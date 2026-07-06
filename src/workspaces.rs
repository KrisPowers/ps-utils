use std::{
    collections::BTreeSet,
    fs,
    io::{IsTerminal, Write, stdin, stdout},
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, bail};
use clap::{Args, Subcommand};
use crossterm::{
    cursor::{Hide, MoveTo, Show, position},
    event::{Event, KeyCode, KeyEventKind, KeyModifiers, poll, read},
    execute, queue,
    style::{Color, Print, ResetColor, SetBackgroundColor, SetForegroundColor},
    terminal::{Clear, ClearType, disable_raw_mode, enable_raw_mode, size},
};
use serde::{Deserialize, Serialize};
use sysinfo::System;

use crate::config::{self, ConfigFile};

const MENU_HEIGHT: u16 = 28;
const TAB_PAGE_SIZE: usize = 16;
const TAB_PAGE_SIZE_U16: u16 = 16;

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

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
struct SessionsConfig {
    sessions: Vec<SessionSnapshot>,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(default)]
struct SessionSnapshot {
    pid: u32,
    path: String,
    updated_at: u64,
    status: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TabCandidate {
    label: String,
    path: String,
    updated_at: u64,
    current: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MainSelection {
    SaveNew,
    Open(usize),
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

    let updated = upsert_workspace(&mut config, workspace);
    save_config(&config)?;

    if updated {
        println!("Updated workspace '{}'.", args.name);
    } else {
        println!("Added workspace '{}'.", args.name);
    }

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
    if !stdin().is_terminal() || !stdout().is_terminal() {
        return list_workspaces();
    }

    let mut config = load_config()?;
    sort_workspaces(&mut config.workspaces);

    let Some(selection) = select_workspace_main_menu(&config.workspaces)? else {
        return Ok(());
    };

    match selection {
        MainSelection::SaveNew => save_workspace_from_menu(&mut config),
        MainSelection::Open(index) => {
            let Some(workspace) = config.workspaces.get_mut(index) else {
                return Ok(());
            };

            workspace.last_accessed = now_secs();
            let workspace = workspace.clone();
            save_config(&config)?;
            write_or_print_script(&workspace_script(&workspace), result)
        }
    }
}

fn save_workspace_from_menu(config: &mut WorkspacesConfig) -> Result<()> {
    let tabs = workspace_tab_candidates()?;
    let Some(paths) = select_workspace_tabs(&tabs)? else {
        return Ok(());
    };

    let default_name = default_workspace_name(&paths);
    let Some(name) = read_workspace_name(&default_name)? else {
        return Ok(());
    };

    validate_workspace_name(&name)?;

    let workspace = SavedWorkspace {
        name: name.clone(),
        path: paths[0].clone(),
        open_paths: paths.iter().skip(1).cloned().collect(),
        commands: Vec::new(),
        last_accessed: now_secs(),
    };

    let updated = upsert_workspace(config, workspace);
    save_config(config)?;

    if updated {
        println!("Updated workspace '{name}'.");
    } else {
        println!("Saved workspace '{name}'.");
    }

    Ok(())
}

fn select_workspace_main_menu(workspaces: &[SavedWorkspace]) -> Result<Option<MainSelection>> {
    let mut stdout = stdout();
    let _guard = TerminalGuard::enter(&mut stdout)?;
    queue!(stdout, Print("\n"))?;
    stdout.flush()?;
    let (_, top) = position().context("failed to read cursor position")?;

    let mut selected = 0usize;
    render_main_menu(&mut stdout, top, workspaces, selected)?;

    loop {
        let Event::Key(event) = read_key()? else {
            continue;
        };

        let old_selected = selected;
        match event.code {
            KeyCode::Up => {
                selected = selected.saturating_sub(1);
            }
            KeyCode::Down if selected < workspaces.len() => {
                selected += 1;
            }
            KeyCode::Home => selected = 0,
            KeyCode::End => selected = workspaces.len(),
            KeyCode::Enter => {
                clear_region(&mut stdout, top, MENU_HEIGHT)?;
                return Ok(Some(if selected == 0 {
                    MainSelection::SaveNew
                } else {
                    MainSelection::Open(selected - 1)
                }));
            }
            KeyCode::Esc => {
                clear_region(&mut stdout, top, MENU_HEIGHT)?;
                return Ok(None);
            }
            KeyCode::Char('c') if event.modifiers.contains(KeyModifiers::CONTROL) => {
                clear_region(&mut stdout, top, MENU_HEIGHT)?;
                return Ok(None);
            }
            _ => {}
        }

        if old_selected != selected {
            render_main_entry_line(&mut stdout, top, workspaces, old_selected, selected)?;
            render_main_entry_line(&mut stdout, top, workspaces, selected, selected)?;
            stdout.flush().context("failed to update workspace menu")?;
        }
    }
}

fn render_main_menu(
    stdout: &mut std::io::Stdout,
    top: u16,
    workspaces: &[SavedWorkspace],
    selected: usize,
) -> Result<()> {
    clear_region(stdout, top, MENU_HEIGHT)?;
    queue!(
        stdout,
        MoveTo(0, top),
        SetForegroundColor(Color::DarkGrey),
        Print("Workspaces"),
        ResetColor,
        MoveTo(0, top + 2),
        SetForegroundColor(Color::DarkGrey),
        Print("Actions"),
        ResetColor
    )?;

    render_main_entry_line(stdout, top, workspaces, 0, selected)?;

    queue!(
        stdout,
        MoveTo(0, top + 5),
        SetForegroundColor(Color::DarkGrey),
        Print("Saved Workspaces"),
        ResetColor
    )?;

    if workspaces.is_empty() {
        queue!(
            stdout,
            MoveTo(0, top + 6),
            SetForegroundColor(Color::DarkGrey),
            Print("No workspaces saved yet."),
            ResetColor
        )?;
    } else {
        for index in 0..workspaces.len() {
            render_main_entry_line(stdout, top, workspaces, index + 1, selected)?;
        }
    }

    queue!(
        stdout,
        MoveTo(0, top + 8 + workspaces.len() as u16),
        SetForegroundColor(Color::DarkGrey),
        Print("Use Up/Down. Enter selects. Esc closes."),
        ResetColor
    )?;

    stdout.flush().context("failed to render workspace menu")
}

fn render_main_entry_line(
    stdout: &mut std::io::Stdout,
    top: u16,
    workspaces: &[SavedWorkspace],
    entry_index: usize,
    selected: usize,
) -> Result<()> {
    let y = if entry_index == 0 {
        top + 3
    } else {
        top + 5 + entry_index as u16
    };

    queue!(stdout, MoveTo(0, y), Clear(ClearType::CurrentLine))?;

    if entry_index == 0 {
        write_button(stdout, "Save new workspace", entry_index == selected, false)?;
        return Ok(());
    }

    let Some(workspace) = workspaces.get(entry_index - 1) else {
        return Ok(());
    };

    let (width, _) = size().unwrap_or((100, 30));
    let name_width = 24usize;
    let path_width = usize::from(width).saturating_sub(name_width + 15).max(20);
    let extras = workspace.open_paths.len() + workspace.commands.len();
    let line = format!(
        "{:<name_width$} {}  tabs: {}",
        clamp(&workspace.name, name_width),
        clamp(&workspace.path, path_width),
        extras + 1,
        name_width = name_width
    );

    if entry_index == selected {
        queue!(
            stdout,
            SetForegroundColor(Color::Black),
            SetBackgroundColor(Color::Yellow),
            Print(line),
            ResetColor
        )?;
    } else {
        queue!(stdout, Print(line))?;
    }

    Ok(())
}

fn select_workspace_tabs(tabs: &[TabCandidate]) -> Result<Option<Vec<String>>> {
    if tabs.is_empty() {
        return Ok(None);
    }

    let mut stdout = stdout();
    let _guard = TerminalGuard::enter(&mut stdout)?;
    queue!(stdout, Print("\n"))?;
    stdout.flush()?;
    let (_, top) = position().context("failed to read cursor position")?;

    let mut selected = 0usize;
    let mut enabled = vec![true; tabs.len()];
    let mut page = 0usize;
    render_tab_menu(&mut stdout, top, tabs, &enabled, selected, page)?;

    loop {
        let Event::Key(event) = read_key()? else {
            continue;
        };

        let old_selected = selected;
        let old_page = page;
        let mut render_selected = false;

        match event.code {
            KeyCode::Up => selected = selected.saturating_sub(1),
            KeyCode::Down if selected < tabs.len() => selected += 1,
            KeyCode::PageUp => selected = selected.saturating_sub(TAB_PAGE_SIZE),
            KeyCode::PageDown => selected = (selected + TAB_PAGE_SIZE).min(tabs.len()),
            KeyCode::Home => selected = 0,
            KeyCode::End => selected = tabs.len(),
            KeyCode::Char(' ') if selected > 0 => {
                enabled[selected - 1] = !enabled[selected - 1];
                render_selected = true;
            }
            KeyCode::Enter => {
                if selected == 0 {
                    let paths = selected_tab_paths(tabs, &enabled);
                    if !paths.is_empty() {
                        clear_region(&mut stdout, top, MENU_HEIGHT)?;
                        return Ok(Some(paths));
                    }
                } else {
                    enabled[selected - 1] = !enabled[selected - 1];
                    render_selected = true;
                }
            }
            KeyCode::Esc => {
                clear_region(&mut stdout, top, MENU_HEIGHT)?;
                return Ok(None);
            }
            KeyCode::Char('c') if event.modifiers.contains(KeyModifiers::CONTROL) => {
                clear_region(&mut stdout, top, MENU_HEIGHT)?;
                return Ok(None);
            }
            _ => {}
        }

        page = selected.saturating_sub(1) / TAB_PAGE_SIZE;
        if selected == 0 {
            page = old_page;
        }

        if page != old_page {
            render_tab_menu(&mut stdout, top, tabs, &enabled, selected, page)?;
        } else if old_selected != selected {
            render_tab_entry_line(
                &mut stdout,
                top,
                tabs,
                &enabled,
                old_selected,
                selected,
                page,
            )?;
            render_tab_entry_line(&mut stdout, top, tabs, &enabled, selected, selected, page)?;
            stdout.flush().context("failed to update tab menu")?;
        } else if render_selected {
            render_tab_entry_line(&mut stdout, top, tabs, &enabled, selected, selected, page)?;
            stdout.flush().context("failed to update tab menu")?;
        }
    }
}

fn render_tab_menu(
    stdout: &mut std::io::Stdout,
    top: u16,
    tabs: &[TabCandidate],
    enabled: &[bool],
    selected: usize,
    page: usize,
) -> Result<()> {
    clear_region(stdout, top, MENU_HEIGHT)?;
    queue!(
        stdout,
        MoveTo(0, top),
        SetForegroundColor(Color::DarkGrey),
        Print("Save Workspace"),
        ResetColor,
        MoveTo(0, top + 2),
        SetForegroundColor(Color::DarkGrey),
        Print("Actions"),
        ResetColor
    )?;

    render_tab_entry_line(stdout, top, tabs, enabled, 0, selected, page)?;

    queue!(
        stdout,
        MoveTo(0, top + 5),
        SetForegroundColor(Color::DarkGrey),
        Print("Open Tabs"),
        ResetColor
    )?;

    for row in 0..TAB_PAGE_SIZE {
        render_tab_entry_line(
            stdout,
            top,
            tabs,
            enabled,
            page * TAB_PAGE_SIZE + row + 1,
            selected,
            page,
        )?;
    }

    queue!(
        stdout,
        MoveTo(0, top + 6 + TAB_PAGE_SIZE_U16 + 1),
        SetForegroundColor(Color::DarkGrey),
        Print(format!(
            "Page {}/{}",
            page + 1,
            tabs.len().max(1).div_ceil(TAB_PAGE_SIZE)
        )),
        MoveTo(0, top + 6 + TAB_PAGE_SIZE_U16 + 3),
        Print("Use Up/Down. Enter toggles tabs or continues. Esc cancels."),
        ResetColor
    )?;

    stdout.flush().context("failed to render tab menu")
}

fn render_tab_entry_line(
    stdout: &mut std::io::Stdout,
    top: u16,
    tabs: &[TabCandidate],
    enabled: &[bool],
    entry_index: usize,
    selected: usize,
    page: usize,
) -> Result<()> {
    if entry_index == 0 {
        queue!(stdout, MoveTo(0, top + 3), Clear(ClearType::CurrentLine))?;
        write_button(stdout, "Continue", selected == 0, false)?;
        return Ok(());
    }

    let tab_index = entry_index - 1;
    let row = tab_index.saturating_sub(page * TAB_PAGE_SIZE);
    if row >= TAB_PAGE_SIZE {
        return Ok(());
    }

    let y = top + 6 + row as u16;
    queue!(stdout, MoveTo(0, y), Clear(ClearType::CurrentLine))?;

    let Some(tab) = tabs.get(tab_index) else {
        return Ok(());
    };

    let is_selected = entry_index == selected;
    let is_enabled = enabled.get(tab_index).copied().unwrap_or(false);
    let (width, _) = size().unwrap_or((100, 30));
    let label_width = 16usize;
    let button_width = 14usize;
    let path_width = usize::from(width)
        .saturating_sub(label_width + button_width + 5)
        .max(20);

    queue!(
        stdout,
        Print(format!(
            "{:<label_width$} {}  ",
            clamp(&tab.label, label_width),
            clamp(&tab.path, path_width),
            label_width = label_width
        ))
    )?;
    write_toggle_button(stdout, is_enabled, is_selected)?;

    Ok(())
}

fn read_workspace_name(default_value: &str) -> Result<Option<String>> {
    let mut stdout = stdout();
    let _guard = TerminalGuard::enter(&mut stdout)?;
    queue!(stdout, Print("\n"))?;
    stdout.flush()?;
    let (_, top) = position().context("failed to read cursor position")?;

    let mut value = default_value.to_string();
    render_workspace_name_input(&mut stdout, top, &value)?;

    loop {
        let Event::Key(event) = read_key()? else {
            continue;
        };

        match event.code {
            KeyCode::Esc => {
                clear_region(&mut stdout, top, MENU_HEIGHT)?;
                return Ok(None);
            }
            KeyCode::Enter => {
                let value = value.trim().to_string();
                if !value.is_empty() {
                    clear_region(&mut stdout, top, MENU_HEIGHT)?;
                    return Ok(Some(value));
                }
            }
            KeyCode::Backspace => {
                value.pop();
            }
            KeyCode::Char('u') if event.modifiers.contains(KeyModifiers::CONTROL) => {
                value.clear();
            }
            KeyCode::Char(c)
                if !event.modifiers.contains(KeyModifiers::CONTROL)
                    && !c.is_control()
                    && value.chars().count() < 64 =>
            {
                value.push(c);
            }
            _ => {}
        }

        render_workspace_name_input(&mut stdout, top, &value)?;
    }
}

fn render_workspace_name_input(stdout: &mut std::io::Stdout, top: u16, value: &str) -> Result<()> {
    clear_region(stdout, top, MENU_HEIGHT)?;
    queue!(
        stdout,
        MoveTo(0, top),
        SetForegroundColor(Color::DarkGrey),
        Print("Workspace Name"),
        ResetColor,
        MoveTo(0, top + 2),
        Print("[ "),
        SetForegroundColor(Color::Black),
        SetBackgroundColor(Color::Yellow),
        Print(if value.is_empty() { " " } else { value }),
        ResetColor,
        Print(" ]"),
        MoveTo(0, top + 4),
        SetForegroundColor(Color::DarkGrey),
        Print("Type a name. Enter saves. Esc cancels."),
        ResetColor
    )?;
    stdout.flush().context("failed to render name input")
}

fn write_button(
    stdout: &mut std::io::Stdout,
    label: &str,
    selected: bool,
    destructive: bool,
) -> Result<()> {
    let button = format!("[ {label} ]");

    if selected {
        if destructive {
            queue!(
                stdout,
                SetForegroundColor(Color::White),
                SetBackgroundColor(Color::DarkRed),
                Print(button),
                ResetColor
            )?;
        } else {
            queue!(
                stdout,
                SetForegroundColor(Color::Black),
                SetBackgroundColor(Color::Yellow),
                Print(button),
                ResetColor
            )?;
        }
    } else if destructive {
        queue!(
            stdout,
            SetForegroundColor(Color::Red),
            Print(button),
            ResetColor
        )?;
    } else {
        queue!(stdout, Print(button))?;
    }

    Ok(())
}

fn write_toggle_button(stdout: &mut std::io::Stdout, enabled: bool, selected: bool) -> Result<()> {
    let label = if enabled { "Enabled" } else { "Disabled" };
    let button = format!("[ {label} ]");

    if selected {
        queue!(
            stdout,
            SetForegroundColor(Color::Black),
            SetBackgroundColor(Color::Yellow),
            Print(button),
            ResetColor
        )?;
    } else if enabled {
        queue!(
            stdout,
            SetForegroundColor(Color::Green),
            Print(button),
            ResetColor
        )?;
    } else {
        queue!(
            stdout,
            SetForegroundColor(Color::DarkGrey),
            Print(button),
            ResetColor
        )?;
    }

    Ok(())
}

fn workspace_tab_candidates() -> Result<Vec<TabCandidate>> {
    let current_path = std::env::current_dir().context("failed to resolve current directory")?;
    let current_path = normalize_path(&current_path).display().to_string();
    let sessions = load_sessions_config().sessions;

    let mut system = System::new_all();
    system.refresh_processes();

    let open_pids = system
        .processes()
        .keys()
        .map(|pid| pid.as_u32())
        .collect::<BTreeSet<_>>();

    Ok(tab_candidates_from_sessions(
        &current_path,
        &sessions,
        &open_pids,
    ))
}

fn tab_candidates_from_sessions(
    current_path: &str,
    sessions: &[SessionSnapshot],
    open_pids: &BTreeSet<u32>,
) -> Vec<TabCandidate> {
    let mut tabs = vec![TabCandidate {
        label: "Current Tab".to_string(),
        path: normalize_path_text(current_path),
        updated_at: now_secs(),
        current: true,
    }];

    let mut seen = vec![normalize_for_compare(current_path)];
    let mut session_tabs = sessions
        .iter()
        .filter(|session| session_is_open(session, open_pids))
        .filter_map(|session| {
            let path = normalize_path_text(&session.path);
            let comparable = normalize_for_compare(&path);

            if path.trim().is_empty() || seen.iter().any(|seen_path| seen_path == &comparable) {
                return None;
            }

            seen.push(comparable);
            Some(TabCandidate {
                label: path_name(Path::new(&path)),
                path,
                updated_at: session.updated_at,
                current: false,
            })
        })
        .collect::<Vec<_>>();

    session_tabs.sort_by(|left, right| {
        right
            .updated_at
            .cmp(&left.updated_at)
            .then_with(|| left.path.cmp(&right.path))
    });

    tabs.extend(session_tabs);
    tabs
}

fn session_is_open(session: &SessionSnapshot, open_pids: &BTreeSet<u32>) -> bool {
    if session.path.trim().is_empty() {
        return false;
    }

    if !session.status.eq_ignore_ascii_case("open") {
        return false;
    }

    session.pid == 0 || open_pids.contains(&session.pid)
}

fn selected_tab_paths(tabs: &[TabCandidate], enabled: &[bool]) -> Vec<String> {
    tabs.iter()
        .zip(enabled)
        .filter(|(_, enabled)| **enabled)
        .map(|(tab, _)| tab.path.clone())
        .collect()
}

fn default_workspace_name(paths: &[String]) -> String {
    paths
        .first()
        .map(|path| path_name(Path::new(path)))
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| "workspace".to_string())
}

fn upsert_workspace(config: &mut WorkspacesConfig, workspace: SavedWorkspace) -> bool {
    if let Some(existing) = config
        .workspaces
        .iter_mut()
        .find(|existing| existing.name.eq_ignore_ascii_case(&workspace.name))
    {
        *existing = workspace;
        return true;
    }

    config.workspaces.push(workspace);
    false
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

fn load_sessions_config() -> SessionsConfig {
    let path = config::config_dir().join("sessions.json");
    let Ok(raw) = fs::read_to_string(path) else {
        return SessionsConfig::default();
    };

    serde_json::from_str(&raw).unwrap_or_default()
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

fn normalize_path_text(path: &str) -> String {
    strip_windows_extended_prefix(Path::new(path))
        .unwrap_or_else(|| PathBuf::from(path))
        .display()
        .to_string()
}

fn normalize_for_compare(path: &str) -> String {
    let normalized = normalize_path_text(path);
    if cfg!(windows) {
        normalized.to_lowercase()
    } else {
        normalized
    }
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

fn path_name(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.trim().is_empty())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| path.display().to_string())
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

fn clamp(value: &str, max: usize) -> String {
    if max < 4 || value.chars().count() <= max {
        return value.to_string();
    }

    let mut text = value.chars().take(max - 3).collect::<String>();
    text.push_str("...");
    text
}

fn read_key() -> Result<Event> {
    loop {
        if !poll(Duration::from_millis(250)).context("failed to poll terminal input")? {
            continue;
        }

        let event = read().context("failed to read terminal input")?;
        if let Event::Key(key) = event
            && key.kind == KeyEventKind::Release
        {
            continue;
        }

        return Ok(event);
    }
}

fn clear_region(stdout: &mut std::io::Stdout, top: u16, height: u16) -> Result<()> {
    for offset in 0..height {
        queue!(
            stdout,
            MoveTo(0, top + offset),
            Clear(ClearType::CurrentLine)
        )?;
    }

    queue!(stdout, MoveTo(0, top))?;
    stdout.flush().context("failed to clear workspace menu")
}

struct TerminalGuard;

impl TerminalGuard {
    fn enter(stdout: &mut std::io::Stdout) -> Result<Self> {
        enable_raw_mode().context("failed to enable terminal raw mode")?;
        execute!(stdout, Hide).context("failed to prepare terminal")?;
        Ok(Self)
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(stdout(), Show);
    }
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
    fn upserts_workspaces_by_name() {
        let mut config = WorkspacesConfig::default();
        assert!(!upsert_workspace(
            &mut config,
            SavedWorkspace {
                name: "api".to_string(),
                path: "C:\\old".to_string(),
                ..SavedWorkspace::default()
            },
        ));
        assert!(upsert_workspace(
            &mut config,
            SavedWorkspace {
                name: "API".to_string(),
                path: "C:\\new".to_string(),
                ..SavedWorkspace::default()
            },
        ));

        assert_eq!(config.workspaces.len(), 1);
        assert_eq!(config.workspaces[0].path, "C:\\new");
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
    fn tab_candidates_keep_current_first_and_sort_open_sessions() {
        let sessions = vec![
            SessionSnapshot {
                pid: 22,
                path: "C:\\web".to_string(),
                updated_at: 2,
                status: "open".to_string(),
            },
            SessionSnapshot {
                pid: 33,
                path: "C:\\api".to_string(),
                updated_at: 3,
                status: "open".to_string(),
            },
            SessionSnapshot {
                pid: 44,
                path: "C:\\closed".to_string(),
                updated_at: 4,
                status: "closed".to_string(),
            },
        ];
        let open_pids = BTreeSet::from([22, 33]);

        let tabs = tab_candidates_from_sessions("C:\\repo", &sessions, &open_pids);

        assert_eq!(tabs[0].path, "C:\\repo");
        assert!(tabs[0].current);
        assert_eq!(tabs[1].path, "C:\\api");
        assert_eq!(tabs[2].path, "C:\\web");
        assert_eq!(tabs.len(), 3);
    }

    #[test]
    fn selected_tab_paths_preserve_order() {
        let tabs = vec![
            TabCandidate {
                label: "Current Tab".to_string(),
                path: "C:\\repo".to_string(),
                updated_at: 0,
                current: true,
            },
            TabCandidate {
                label: "web".to_string(),
                path: "C:\\web".to_string(),
                updated_at: 0,
                current: false,
            },
        ];

        assert_eq!(
            selected_tab_paths(&tabs, &[true, false]),
            vec!["C:\\repo".to_string()]
        );
        assert_eq!(
            selected_tab_paths(&tabs, &[false, true]),
            vec!["C:\\web".to_string()]
        );
    }

    #[test]
    fn default_workspace_name_uses_first_path_leaf() {
        assert_eq!(
            default_workspace_name(&["C:\\Users\\krisp\\repo".to_string()]),
            "repo"
        );
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
