use std::{
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, bail};
use clap::{Args, Subcommand};
use serde::{Deserialize, Serialize};

use crate::{
    config::{self, ConfigFile},
    picker::{Picker, PickerItem},
};

#[derive(Debug, Args)]
pub struct WorkspacesArgs {
    /// Write launch script to this file for the profile bridge.
    #[arg(long, hide = true)]
    pub result: Option<PathBuf>,

    #[command(subcommand)]
    pub command: Option<WorkspacesCommand>,
}

#[derive(Debug, Subcommand)]
pub enum WorkspacesCommand {
    /// Add or update a workspace.
    Add(AddWorkspaceArgs),

    /// List saved workspaces.
    List,

    /// Remove a workspace.
    Remove { name: String },

    /// Open a workspace by name.
    Open { name: String },
}

#[derive(Debug, Args)]
pub struct AddWorkspaceArgs {
    /// Workspace name.
    pub name: String,

    /// Main workspace path. Defaults to the current directory.
    #[arg(long)]
    pub path: Option<PathBuf>,

    /// Extra paths to open in additional PowerShell windows.
    #[arg(long = "open")]
    pub open_paths: Vec<PathBuf>,

    /// Commands to run after changing to the workspace path.
    #[arg(long = "command")]
    pub commands: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
struct WorkspacesConfig {
    version: u8,
    workspaces: Vec<SavedWorkspace>,
}

impl Default for WorkspacesConfig {
    fn default() -> Self {
        Self {
            version: 1,
            workspaces: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
struct SavedWorkspace {
    name: String,
    path: String,
    open_paths: Vec<String>,
    commands: Vec<String>,
    last_accessed: u64,
}

pub fn run(args: WorkspacesArgs) -> Result<()> {
    match args.command {
        Some(WorkspacesCommand::Add(add_args)) => add_workspace(add_args),
        Some(WorkspacesCommand::List) => list_workspaces(),
        Some(WorkspacesCommand::Remove { name }) => remove_workspace(&name),
        Some(WorkspacesCommand::Open { name }) => open_workspace(&name, args.result),
        None => open_workspace_menu(args.result),
    }
}

fn add_workspace(args: AddWorkspaceArgs) -> Result<()> {
    validate_workspace_name(&args.name)?;

    let mut config = load_config()?;
    let path = args
        .path
        .unwrap_or(std::env::current_dir().context("failed to resolve current directory")?);
    let path = normalize_path(&path);
    let open_paths = args
        .open_paths
        .iter()
        .map(|path| normalize_path(path).display().to_string())
        .collect::<Vec<_>>();

    let workspace = SavedWorkspace {
        name: args.name.clone(),
        path: path.display().to_string(),
        open_paths,
        commands: args.commands,
        last_accessed: now_secs(),
    };

    if let Some(existing) = config
        .workspaces
        .iter_mut()
        .find(|workspace| workspace.name.eq_ignore_ascii_case(&args.name))
    {
        *existing = workspace;
        println!("Updated workspace '{}'.", args.name);
    } else {
        config.workspaces.push(workspace);
        println!("Added workspace '{}'.", args.name);
    }

    save_config(&config)?;
    Ok(())
}

fn list_workspaces() -> Result<()> {
    let mut config = load_config()?;
    sort_workspaces(&mut config.workspaces);

    println!("{:<24} {:<56} Extras", "Name", "Path");
    for workspace in config.workspaces {
        let extras = workspace.open_paths.len() + workspace.commands.len();
        println!("{:<24} {:<56} {}", workspace.name, workspace.path, extras);
    }

    Ok(())
}

fn remove_workspace(name: &str) -> Result<()> {
    let mut config = load_config()?;
    let original_len = config.workspaces.len();
    config
        .workspaces
        .retain(|workspace| !workspace.name.eq_ignore_ascii_case(name));

    if config.workspaces.len() == original_len {
        bail!("workspace '{name}' was not found");
    }

    save_config(&config)?;
    println!("Removed workspace '{name}'.");
    Ok(())
}

fn open_workspace(name: &str, result: Option<PathBuf>) -> Result<()> {
    let mut config = load_config()?;
    let Some(index) = config
        .workspaces
        .iter()
        .position(|workspace| workspace.name.eq_ignore_ascii_case(name))
    else {
        bail!("workspace '{name}' was not found");
    };

    config.workspaces[index].last_accessed = now_secs();
    let workspace = config.workspaces[index].clone();
    save_config(&config)?;
    write_or_print_script(&workspace_script(&workspace), result)
}

fn open_workspace_menu(result: Option<PathBuf>) -> Result<()> {
    let mut config = load_config()?;
    sort_workspaces(&mut config.workspaces);

    let items = config
        .workspaces
        .iter()
        .map(|workspace| {
            PickerItem::new(
                workspace.name.clone(),
                format!(
                    "{}  extras: {}",
                    workspace.path,
                    workspace.open_paths.len() + workspace.commands.len()
                ),
            )
        })
        .collect::<Vec<_>>();

    let Some(selected) = Picker::new("Workspaces", "Name                     Path", items)
        .help("Use Up/Down. Enter opens. Esc closes.")
        .select()?
    else {
        return Ok(());
    };

    let Some(workspace) = config.workspaces.get_mut(selected) else {
        return Ok(());
    };

    workspace.last_accessed = now_secs();
    let workspace = workspace.clone();
    save_config(&config)?;
    write_or_print_script(&workspace_script(&workspace), result)
}

fn write_or_print_script(script: &str, result: Option<PathBuf>) -> Result<()> {
    if let Some(result) = result {
        fs::write(&result, script)
            .with_context(|| format!("failed to write workspace result {}", result.display()))?;
    } else {
        print!("{script}");
    }

    Ok(())
}

fn workspace_script(workspace: &SavedWorkspace) -> String {
    let mut script = String::new();
    let path = expand_path(&workspace.path);
    script.push_str(&format!(
        "Set-Location -LiteralPath {}\n",
        ps_quote_path(&path)
    ));

    for command in &workspace.commands {
        if !command.trim().is_empty() {
            script.push_str(command);
            if !command.ends_with('\n') {
                script.push('\n');
            }
        }
    }

    if !workspace.open_paths.is_empty() {
        script.push_str("$__psShell = (Get-Command pwsh -ErrorAction SilentlyContinue).Source\n");
        script.push_str("if (-not $__psShell) { $__psShell = (Get-Command powershell -ErrorAction SilentlyContinue).Source }\n");
        script.push_str("if (-not $__psShell) { throw 'Could not find pwsh or powershell to open workspace windows.' }\n");

        for open_path in &workspace.open_paths {
            let open_path = expand_path(open_path);
            let quoted_path = ps_quote_path(&open_path);
            let inner = format!("Set-Location -LiteralPath {quoted_path}");
            script.push_str(&format!(
                "Start-Process -FilePath $__psShell -ArgumentList @('-NoExit', '-Command', {})\n",
                ps_quote(&inner)
            ));
        }
    }

    script
}

fn load_config() -> Result<WorkspacesConfig> {
    let path = config::ensure_workspaces_config()?;
    let raw = fs::read_to_string(&path)
        .with_context(|| format!("failed to read workspaces config at {}", path.display()))?;
    serde_json::from_str(&raw)
        .with_context(|| format!("failed to parse workspaces config at {}", path.display()))
}

fn save_config(config: &WorkspacesConfig) -> Result<()> {
    let path = config::config_path(ConfigFile::Workspaces);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create config directory {}", parent.display()))?;
    }

    let json = serde_json::to_string_pretty(config).context("failed to serialize workspaces")?;
    fs::write(&path, format!("{json}\n"))
        .with_context(|| format!("failed to write workspaces config {}", path.display()))?;
    Ok(())
}

fn sort_workspaces(workspaces: &mut [SavedWorkspace]) {
    workspaces.sort_by(|left, right| {
        right
            .last_accessed
            .cmp(&left.last_accessed)
            .then_with(|| left.name.cmp(&right.name))
    });
}

fn validate_workspace_name(name: &str) -> Result<()> {
    if name.trim().is_empty() {
        bail!("workspace name cannot be empty");
    }

    Ok(())
}

fn normalize_path(path: &Path) -> PathBuf {
    let normalized = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    strip_windows_extended_prefix(&normalized).unwrap_or(normalized)
}

fn strip_windows_extended_prefix(path: &Path) -> Option<PathBuf> {
    let value = path.display().to_string();

    value
        .strip_prefix(r"\\?\UNC\")
        .map(|rest| PathBuf::from(format!(r"\\{rest}")))
        .or_else(|| {
            value
                .strip_prefix(r"\\?\")
                .map(|rest| PathBuf::from(rest.to_string()))
        })
}

fn expand_path(path: &str) -> PathBuf {
    if path == "~" {
        return config::home_dir().unwrap_or_else(|| PathBuf::from(path));
    }

    if let Some(rest) = path.strip_prefix("~/").or_else(|| path.strip_prefix("~\\"))
        && let Some(home) = config::home_dir()
    {
        return home.join(rest);
    }

    PathBuf::from(path)
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn ps_quote_path(path: &Path) -> String {
    ps_quote(&path.display().to_string())
}

fn ps_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sorts_workspaces_by_last_accessed() {
        let mut workspaces = vec![
            SavedWorkspace {
                name: "old".to_string(),
                last_accessed: 1,
                ..SavedWorkspace::default()
            },
            SavedWorkspace {
                name: "new".to_string(),
                last_accessed: 2,
                ..SavedWorkspace::default()
            },
        ];

        sort_workspaces(&mut workspaces);
        assert_eq!(workspaces[0].name, "new");
    }

    #[test]
    fn workspace_script_sets_location() {
        let workspace = SavedWorkspace {
            name: "repo".to_string(),
            path: "C:\\repo".to_string(),
            ..SavedWorkspace::default()
        };

        let script = workspace_script(&workspace);
        assert!(script.contains("Set-Location -LiteralPath 'C:\\repo'"));
    }

    #[test]
    fn quotes_powershell_literals() {
        assert_eq!(ps_quote("Kris's Repo"), "'Kris''s Repo'");
    }

    #[test]
    fn strips_windows_extended_path_prefixes() {
        assert_eq!(
            strip_windows_extended_prefix(Path::new(r"\\?\C:\repo")).unwrap(),
            PathBuf::from(r"C:\repo")
        );
        assert_eq!(
            strip_windows_extended_prefix(Path::new(r"\\?\UNC\server\share")).unwrap(),
            PathBuf::from(r"\\server\share")
        );
    }
}
