use std::{
    collections::BTreeMap,
    env, fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

pub const APP_NAME: &str = "ps";

#[derive(Debug, Clone, Copy)]
pub enum ConfigFile {
    Commands,
    Paths,
    Settings,
    Workspaces,
    Dir,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct CommandsConfig {
    pub version: u8,
    pub commands: BTreeMap<String, ShortcutCommand>,
}

impl CommandsConfig {
    pub fn sample() -> Self {
        let mut commands = BTreeMap::new();

        commands.insert(
            "kill-port".to_string(),
            ShortcutCommand::KillPort {
                description: Some("Stop the process listening on a TCP port.".to_string()),
            },
        );

        commands.insert(
            "repo".to_string(),
            ShortcutCommand::Shell {
                description: Some("Example custom script. Replace this with your own.".to_string()),
                script: Some("Set-Location -LiteralPath $HOME".to_string()),
                script_path: None,
                dot_source: default_dot_source(),
            },
        );

        Self {
            version: 1,
            commands,
        }
    }
}

impl Default for CommandsConfig {
    fn default() -> Self {
        Self {
            version: 1,
            commands: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum ShortcutCommand {
    KillPort {
        #[serde(default)]
        description: Option<String>,
    },
    Workspace {
        #[serde(default)]
        description: Option<String>,
        path: String,
        #[serde(default)]
        open_windows: Vec<String>,
    },
    Shell {
        #[serde(default)]
        description: Option<String>,
        #[serde(default)]
        script: Option<String>,
        #[serde(default)]
        script_path: Option<String>,
        #[serde(default = "default_dot_source")]
        dot_source: bool,
    },
}

pub fn default_dot_source() -> bool {
    true
}

pub fn config_path(file: ConfigFile) -> PathBuf {
    match file {
        ConfigFile::Commands => config_dir().join("commands.json"),
        ConfigFile::Paths => config_dir().join("paths.json"),
        ConfigFile::Settings => config_dir().join("settings.json"),
        ConfigFile::Workspaces => config_dir().join("workspaces.json"),
        ConfigFile::Dir => config_dir(),
    }
}

pub fn ensure_all() -> Result<()> {
    ensure_commands_config()?;
    ensure_paths_config()?;
    ensure_settings_config()?;
    ensure_workspaces_config()?;
    Ok(())
}

pub fn ensure_commands_config() -> Result<PathBuf> {
    let path = config_path(ConfigFile::Commands);
    ensure_json_file(&path, &CommandsConfig::sample())?;
    Ok(path)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PathsConfig {
    pub version: u8,
    pub saved: Vec<SavedPath>,
}

impl Default for PathsConfig {
    fn default() -> Self {
        Self {
            version: 1,
            saved: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedPath {
    pub name: String,
    pub path: String,
    pub last_accessed: u64,
}

pub fn ensure_paths_config() -> Result<PathBuf> {
    let path = config_path(ConfigFile::Paths);
    ensure_json_file(&path, &PathsConfig::default())?;
    Ok(path)
}

pub fn load_paths_config() -> Result<PathsConfig> {
    let path = ensure_paths_config()?;
    let raw = fs::read_to_string(&path)
        .with_context(|| format!("failed to read paths config at {}", path.display()))?;
    serde_json::from_str(strip_utf8_bom(&raw))
        .with_context(|| format!("failed to parse paths config at {}", path.display()))
}

pub fn save_paths_config(paths: &PathsConfig) -> Result<PathBuf> {
    let path = config_path(ConfigFile::Paths);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create config directory {}", parent.display()))?;
    }

    let json = serde_json::to_string_pretty(paths).context("failed to serialize paths config")?;
    fs::write(&path, format!("{json}\n"))
        .with_context(|| format!("failed to write paths config {}", path.display()))?;
    Ok(path)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SettingsConfig {
    pub version: u8,
    pub short_pwd: bool,
    pub display_timestamps: bool,
    pub terminal_history: bool,
    pub terminal_history_max_length: u16,
    pub session_history: bool,
}

impl Default for SettingsConfig {
    fn default() -> Self {
        Self {
            version: 1,
            short_pwd: false,
            display_timestamps: false,
            terminal_history: false,
            terminal_history_max_length: 200,
            session_history: false,
        }
    }
}

pub fn ensure_settings_config() -> Result<PathBuf> {
    let path = config_path(ConfigFile::Settings);
    ensure_json_file(&path, &SettingsConfig::default())?;
    Ok(path)
}

pub fn load_settings_config() -> Result<SettingsConfig> {
    let path = ensure_settings_config()?;
    let raw = fs::read_to_string(&path)
        .with_context(|| format!("failed to read settings config at {}", path.display()))?;
    serde_json::from_str(strip_utf8_bom(&raw))
        .with_context(|| format!("failed to parse settings config at {}", path.display()))
}

pub fn ensure_workspaces_config() -> Result<PathBuf> {
    let path = config_path(ConfigFile::Workspaces);
    ensure_json_file(
        &path,
        &serde_json::json!({
            "version": 1,
            "workspaces": []
        }),
    )?;
    Ok(path)
}

pub fn load_commands() -> Result<CommandsConfig> {
    let path = ensure_commands_config()?;
    let raw = fs::read_to_string(&path)
        .with_context(|| format!("failed to read commands config at {}", path.display()))?;
    serde_json::from_str(strip_utf8_bom(&raw))
        .with_context(|| format!("failed to parse commands config at {}", path.display()))
}

fn strip_utf8_bom(value: &str) -> &str {
    value.strip_prefix('\u{feff}').unwrap_or(value)
}

fn ensure_json_file<T>(path: &Path, value: &T) -> Result<()>
where
    T: Serialize,
{
    if path.exists() {
        return Ok(());
    }

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create config directory {}", parent.display()))?;
    }

    let json = serde_json::to_string_pretty(value).context("failed to serialize default config")?;
    fs::write(path, format!("{json}\n"))
        .with_context(|| format!("failed to write config file {}", path.display()))?;
    Ok(())
}

pub fn config_dir() -> PathBuf {
    if let Some(appdata) = env::var_os("APPDATA") {
        return PathBuf::from(appdata).join(APP_NAME);
    }

    if let Some(xdg) = env::var_os("XDG_CONFIG_HOME") {
        return PathBuf::from(xdg).join(APP_NAME);
    }

    home_dir()
        .unwrap_or_else(|| env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
        .join(".config")
        .join(APP_NAME)
}

pub fn home_dir() -> Option<PathBuf> {
    env::var_os("USERPROFILE")
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(PathBuf::from))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_utf8_bom_before_json_parse() {
        let raw = "\u{feff}{\"version\":1,\"commands\":{}}";
        let parsed: CommandsConfig = serde_json::from_str(strip_utf8_bom(raw)).unwrap();

        assert_eq!(parsed.version, 1);
        assert!(parsed.commands.is_empty());
    }
}
