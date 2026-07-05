use std::{
    collections::{HashMap, HashSet},
    io::{IsTerminal, Write, stdin, stdout},
    path::Path,
    process::Command,
    sync::mpsc::{self, TryRecvError},
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail};
use clap::{Args, ValueEnum};
use crossterm::{
    cursor::{Hide, MoveTo, Show, position},
    event::{Event, KeyCode, KeyEventKind, KeyModifiers, poll, read},
    execute, queue,
    style::{Color, Print, ResetColor, SetBackgroundColor, SetForegroundColor},
    terminal::{Clear, ClearType, disable_raw_mode, enable_raw_mode, size},
};
use sysinfo::{Pid, System};

use crate::loading;

const PAGE_SIZE: usize = 20;
const PAGE_SIZE_U16: u16 = 20;
const MENU_HEIGHT: u16 = 29;
const ROW_START: u16 = 3;

#[derive(Debug, Args)]
pub struct PortsArgs {
    /// Show only one local TCP port.
    #[arg(short = 'p', long = "port", value_parser = clap::value_parser!(u16).range(1..=65535))]
    pub port: Option<u16>,

    /// Filter by TCP state, like established, time-wait, or listen.
    #[arg(short = 's', long = "state")]
    pub state: Option<String>,

    /// Filter by process name.
    #[arg(short = 'n', long = "name")]
    pub name: Option<String>,

    /// Keep refreshing the interactive menu while it is open.
    #[arg(long)]
    pub refresh: bool,

    /// Sort rows by port, state, or process.
    #[arg(long, value_enum, default_value_t = PortsSort::Port)]
    pub sort: PortsSort,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum PortsSort {
    Port,
    State,
    Process,
}

#[derive(Debug, Clone)]
struct PortQuery {
    port: Option<u16>,
    state: Option<String>,
    name: Option<String>,
    refresh: bool,
    sort: PortsSort,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PortRow {
    port: u16,
    state: String,
    address: String,
    remote_address: String,
    pid: u32,
    process: String,
    process_path: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Focus {
    Row,
    Kill,
    Prev,
    Next,
}

#[derive(Debug)]
struct PortsMenu {
    rows: Vec<PortRow>,
    page: usize,
    selected_row: usize,
    focus: Focus,
    status: Option<String>,
    query: PortQuery,
}

pub fn run(args: PortsArgs) -> Result<()> {
    let query = PortQuery::from(args);

    if !stdin().is_terminal() || !stdout().is_terminal() {
        let rows = tcp_ports(&query)?;
        print_plain(&rows, &query);
        return Ok(());
    }

    run_interactive(query)
}

fn run_interactive(query: PortQuery) -> Result<()> {
    let mut stdout = stdout();
    let _guard = TerminalGuard::enter(&mut stdout)?;
    queue!(stdout, Print("\n"))?;
    stdout.flush()?;
    let (_, top) = position().context("failed to read cursor position")?;

    render_loading_shell(&mut stdout, top, &query)?;

    let (sender, receiver) = mpsc::channel();
    let load_query = query.clone();
    let _loader = thread::spawn(move || {
        let _ = sender.send(tcp_ports(&load_query));
    });

    let mut frame = 0usize;
    let rows = loop {
        render_loading_frame(&mut stdout, top, frame)?;

        match receiver.try_recv() {
            Ok(Ok(rows)) => break rows,
            Ok(Err(error)) => {
                clear_region(&mut stdout, top)?;
                return Err(error);
            }
            Err(TryRecvError::Disconnected) => {
                clear_region(&mut stdout, top)?;
                bail!("failed to load TCP ports");
            }
            Err(TryRecvError::Empty) => {}
        }

        if poll(Duration::from_millis(140)).context("failed to poll terminal input")? {
            let Event::Key(event) = read().context("failed to read terminal input")? else {
                continue;
            };

            if event.kind == KeyEventKind::Release {
                continue;
            }

            if event.code == KeyCode::Esc
                || (event.code == KeyCode::Char('c')
                    && event.modifiers.contains(KeyModifiers::CONTROL))
            {
                clear_region(&mut stdout, top)?;
                bail!("ports menu canceled");
            }
        }

        frame = frame.wrapping_add(1);
    };

    PortsMenu::new(rows, query).run_at(&mut stdout, top)
}

fn tcp_ports(query: &PortQuery) -> Result<Vec<PortRow>> {
    if !cfg!(windows) {
        bail!("ports is currently implemented for Windows");
    }

    let output = Command::new("netstat")
        .args(["-ano", "-p", "tcp"])
        .output()
        .context("failed to run netstat")?;

    if !output.status.success() {
        bail!("netstat exited with {}", output.status);
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut parsed_rows = Vec::new();
    let mut seen = HashSet::new();
    let mut pids = HashSet::new();

    for line in stdout.lines() {
        let Some(parsed) = parse_netstat_line(line) else {
            continue;
        };

        if let Some(port_filter) = query.port
            && parsed.port != port_filter
        {
            continue;
        }

        if let Some(state_filter) = &query.state
            && !tcp_state_matches(&parsed.state, state_filter)
        {
            continue;
        }

        let key = (
            parsed.address.clone(),
            parsed.port,
            parsed.remote_address.clone(),
            parsed.state.clone(),
            parsed.pid,
        );
        if !seen.insert(key) {
            continue;
        }

        pids.insert(parsed.pid);
        parsed_rows.push(parsed);
    }

    let process_info = process_infos(&pids);
    let mut rows = Vec::new();

    for parsed in parsed_rows {
        let info = process_info
            .get(&parsed.pid)
            .cloned()
            .unwrap_or_else(|| ProcessInfo::fallback(parsed.pid));
        let row = PortRow {
            port: parsed.port,
            state: display_tcp_state(&parsed.state),
            address: parsed.address,
            remote_address: parsed.remote_address,
            pid: parsed.pid,
            process: format!("{} ({})", info.name, parsed.pid),
            process_path: info.path,
        };

        if let Some(name_filter) = &query.name
            && !row
                .process
                .to_lowercase()
                .contains(&name_filter.to_lowercase())
        {
            continue;
        }

        rows.push(row);
    }

    sort_rows(&mut rows, query.sort);

    Ok(rows)
}

impl From<PortsArgs> for PortQuery {
    fn from(args: PortsArgs) -> Self {
        Self {
            port: args.port,
            state: args.state,
            name: args.name,
            refresh: args.refresh,
            sort: args.sort,
        }
    }
}

#[derive(Debug)]
struct ParsedNetstatLine {
    address: String,
    port: u16,
    remote_address: String,
    state: String,
    pid: u32,
}

fn parse_netstat_line(line: &str) -> Option<ParsedNetstatLine> {
    let parts = line.split_whitespace().collect::<Vec<_>>();
    if parts.len() < 5 || parts.first().copied() != Some("TCP") {
        return None;
    }

    let (address, port) = parse_local_address(parts[1])?;
    let remote_address = parse_remote_address(parts[2])?;
    let state = parts[3].to_string();
    let pid = parts[4].parse::<u32>().ok()?;

    Some(ParsedNetstatLine {
        address,
        port,
        remote_address,
        state,
        pid,
    })
}

fn parse_local_address(value: &str) -> Option<(String, u16)> {
    if let Some(rest) = value.strip_prefix('[') {
        let (address, port) = rest.rsplit_once("]:")?;
        return Some((address.to_string(), port.parse().ok()?));
    }

    let (address, port) = value.rsplit_once(':')?;
    Some((address.to_string(), port.parse().ok()?))
}

fn parse_remote_address(value: &str) -> Option<String> {
    if let Some(rest) = value.strip_prefix('[') {
        let (address, port) = rest.rsplit_once("]:")?;
        if port == "0" {
            return Some(address.to_string());
        }

        return Some(format!("[{address}]:{port}"));
    }

    if let Some((address, port)) = value.rsplit_once(':')
        && port == "0"
    {
        return Some(address.to_string());
    }

    Some(value.to_string())
}

fn display_tcp_state(state: &str) -> String {
    match state {
        "LISTENING" => "Listen".to_string(),
        _ => state
            .split('_')
            .filter(|part| !part.is_empty())
            .map(|part| {
                let mut chars = part.chars();
                let Some(first) = chars.next() else {
                    return String::new();
                };

                let first = first.to_uppercase().collect::<String>();
                let rest = chars.as_str().to_lowercase();
                format!("{first}{rest}")
            })
            .collect::<Vec<_>>()
            .join(" "),
    }
}

fn tcp_state_matches(state: &str, filter: &str) -> bool {
    let normalized_filter = normalize_state_token(filter);
    if normalized_filter.is_empty() {
        return true;
    }

    let raw = normalize_state_token(state);
    let display = normalize_state_token(&display_tcp_state(state));

    raw == normalized_filter
        || display == normalized_filter
        || (state == "LISTENING" && normalized_filter == "listen")
}

fn normalize_state_token(value: &str) -> String {
    value
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ProcessInfo {
    name: String,
    path: String,
}

impl ProcessInfo {
    fn fallback(pid: u32) -> Self {
        Self {
            name: fallback_process_name(pid),
            path: String::new(),
        }
    }
}

fn process_infos(pids: &HashSet<u32>) -> HashMap<u32, ProcessInfo> {
    let mut system = System::new_all();
    system.refresh_processes();

    pids.iter()
        .map(|pid| {
            let info = system
                .process(Pid::from_u32(*pid))
                .map(|process| {
                    let name = clean_process_name(process.name());
                    let path = process
                        .exe()
                        .map(|path| path.display().to_string())
                        .unwrap_or_default();

                    ProcessInfo {
                        name: if name.trim().is_empty() {
                            fallback_process_name(*pid)
                        } else {
                            name
                        },
                        path,
                    }
                })
                .unwrap_or_else(|| ProcessInfo::fallback(*pid));

            (*pid, info)
        })
        .collect()
}

fn sort_rows(rows: &mut [PortRow], sort: PortsSort) {
    match sort {
        PortsSort::Port => rows.sort_by(|left, right| {
            left.port
                .cmp(&right.port)
                .then_with(|| left.address.cmp(&right.address))
                .then_with(|| left.remote_address.cmp(&right.remote_address))
                .then_with(|| left.state.cmp(&right.state))
                .then_with(|| left.pid.cmp(&right.pid))
        }),
        PortsSort::State => rows.sort_by(|left, right| {
            left.state
                .cmp(&right.state)
                .then_with(|| left.port.cmp(&right.port))
                .then_with(|| left.process.cmp(&right.process))
                .then_with(|| left.remote_address.cmp(&right.remote_address))
        }),
        PortsSort::Process => rows.sort_by(|left, right| {
            left.process
                .cmp(&right.process)
                .then_with(|| left.port.cmp(&right.port))
                .then_with(|| left.state.cmp(&right.state))
                .then_with(|| left.remote_address.cmp(&right.remote_address))
        }),
    }
}

fn clean_process_name(name: &str) -> String {
    Path::new(name)
        .file_stem()
        .and_then(|value| value.to_str())
        .filter(|value| !value.trim().is_empty())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| name.to_string())
}

fn fallback_process_name(pid: u32) -> String {
    match pid {
        0 => "System Idle Process".to_string(),
        4 => "System".to_string(),
        _ => format!("PID {pid}"),
    }
}

impl PortsMenu {
    fn new(rows: Vec<PortRow>, query: PortQuery) -> Self {
        Self {
            rows,
            page: 0,
            selected_row: 0,
            focus: Focus::Row,
            status: None,
            query,
        }
    }

    fn run_at(mut self, stdout: &mut std::io::Stdout, top: u16) -> Result<()> {
        self.normalize_selection();
        render_full(stdout, top, &self)?;
        let mut last_auto_refresh = Instant::now();

        loop {
            if !poll(Duration::from_millis(250)).context("failed to poll terminal input")? {
                if self.query.refresh && last_auto_refresh.elapsed() >= Duration::from_secs(2) {
                    self.refresh_rows()?;
                    render_full(stdout, top, &self)?;
                    last_auto_refresh = Instant::now();
                }

                continue;
            }

            let Event::Key(event) = read().context("failed to read terminal input")? else {
                continue;
            };

            if event.kind == KeyEventKind::Release {
                continue;
            }

            let old_focus = self.focus;
            let old_row = self.selected_row;
            let mut full_render = false;

            match event.code {
                KeyCode::Up => self.move_up(),
                KeyCode::Down => self.move_down(),
                KeyCode::Left => self.move_left(),
                KeyCode::Right => self.move_right(),
                KeyCode::Home => {
                    self.focus = Focus::Row;
                    self.selected_row = 0;
                }
                KeyCode::End => {
                    self.focus = Focus::Row;
                    self.selected_row = self.visible_count().saturating_sub(1);
                }
                KeyCode::PageUp | KeyCode::Char('p') => {
                    full_render = self.previous_page();
                }
                KeyCode::PageDown | KeyCode::Char('n') => {
                    full_render = self.next_page();
                }
                KeyCode::Enter => {
                    full_render = self.accept(stdout, top)?;
                    last_auto_refresh = Instant::now();
                }
                KeyCode::Char('r') => {
                    self.refresh_rows()?;
                    full_render = true;
                    last_auto_refresh = Instant::now();
                }
                KeyCode::Esc => {
                    clear_region(stdout, top)?;
                    break;
                }
                KeyCode::Char('c') if event.modifiers.contains(KeyModifiers::CONTROL) => {
                    clear_region(stdout, top)?;
                    break;
                }
                _ => {}
            }

            self.normalize_selection();

            if full_render {
                render_full(stdout, top, &self)?;
            } else {
                render_selection_change(
                    stdout,
                    top,
                    &self,
                    old_focus,
                    old_row,
                    self.focus,
                    self.selected_row,
                )?;
            }
        }

        Ok(())
    }

    fn visible_rows(&self) -> &[PortRow] {
        let start = self.page * PAGE_SIZE;
        let end = (start + PAGE_SIZE).min(self.rows.len());
        &self.rows[start..end]
    }

    fn visible_count(&self) -> usize {
        self.visible_rows().len()
    }

    fn page_count(&self) -> usize {
        self.rows.len().max(1).div_ceil(PAGE_SIZE)
    }

    fn normalize_selection(&mut self) {
        let visible_count = self.visible_count();
        if visible_count == 0 {
            self.selected_row = 0;
            if matches!(self.focus, Focus::Row | Focus::Kill) {
                self.focus = Focus::Next;
            }
            return;
        }

        if self.selected_row >= visible_count {
            self.selected_row = visible_count - 1;
        }
    }

    fn move_up(&mut self) {
        match self.focus {
            Focus::Row | Focus::Kill => {
                if self.selected_row > 0 {
                    self.selected_row -= 1;
                }
            }
            Focus::Prev | Focus::Next => {
                if self.visible_count() > 0 {
                    self.focus = Focus::Row;
                    self.selected_row = self.visible_count() - 1;
                }
            }
        }
    }

    fn move_down(&mut self) {
        match self.focus {
            Focus::Row | Focus::Kill => {
                if self.selected_row + 1 < self.visible_count() {
                    self.selected_row += 1;
                } else {
                    self.focus = Focus::Next;
                }
            }
            Focus::Prev | Focus::Next => {}
        }
    }

    fn move_left(&mut self) {
        self.focus = match self.focus {
            Focus::Kill => Focus::Row,
            Focus::Next => Focus::Prev,
            other => other,
        };
    }

    fn move_right(&mut self) {
        self.focus = match self.focus {
            Focus::Row if self.visible_count() > 0 => Focus::Kill,
            Focus::Prev => Focus::Next,
            other => other,
        };
    }

    fn previous_page(&mut self) -> bool {
        if self.page == 0 {
            return false;
        }

        self.page -= 1;
        self.selected_row = 0;
        self.focus = Focus::Row;
        true
    }

    fn next_page(&mut self) -> bool {
        if self.page + 1 >= self.page_count() {
            return false;
        }

        self.page += 1;
        self.selected_row = 0;
        self.focus = Focus::Row;
        true
    }

    fn accept(&mut self, stdout: &mut std::io::Stdout, top: u16) -> Result<bool> {
        match self.focus {
            Focus::Row => {
                self.show_detail(stdout, top)?;
                Ok(true)
            }
            Focus::Kill => {
                let Some(row) = self.visible_rows().get(self.selected_row).cloned() else {
                    return Ok(false);
                };

                self.status = Some(kill_process(&row));
                self.refresh_rows()?;
                Ok(true)
            }
            Focus::Prev => Ok(self.previous_page()),
            Focus::Next => Ok(self.next_page()),
        }
    }

    fn refresh_rows(&mut self) -> Result<()> {
        self.rows = tcp_ports(&self.query)?;

        if self.page >= self.page_count() {
            self.page = self.page_count() - 1;
        }

        self.selected_row = self
            .selected_row
            .min(self.visible_count().saturating_sub(1));
        self.focus = if self.visible_count() > 0 {
            Focus::Row
        } else {
            Focus::Next
        };

        Ok(())
    }

    fn show_detail(&mut self, stdout: &mut std::io::Stdout, top: u16) -> Result<()> {
        let Some(row) = self.visible_rows().get(self.selected_row).cloned() else {
            return Ok(());
        };

        let mut status = None;

        loop {
            render_detail(stdout, top, &row, status.as_deref())?;

            if !poll(Duration::from_millis(250)).context("failed to poll terminal input")? {
                continue;
            }

            let Event::Key(event) = read().context("failed to read terminal input")? else {
                continue;
            };

            if event.kind == KeyEventKind::Release {
                continue;
            }

            match event.code {
                KeyCode::Enter | KeyCode::Char('k') => {
                    let message = kill_process(&row);
                    self.status = Some(message.clone());
                    status = Some(message);
                    self.refresh_rows()?;
                }
                KeyCode::Esc | KeyCode::Backspace => break,
                KeyCode::Char('c') if event.modifiers.contains(KeyModifiers::CONTROL) => break,
                _ => {}
            }
        }

        Ok(())
    }
}

fn kill_process(row: &PortRow) -> String {
    let status = Command::new("taskkill")
        .args(["/PID", &row.pid.to_string(), "/F"])
        .status();

    match status {
        Ok(status) if status.success() => {
            format!("Stopped {} on port {}.", row.process, row.port)
        }
        Ok(status) => {
            format!(
                "Failed to stop PID {} on port {}: taskkill exited with {status}.",
                row.pid, row.port
            )
        }
        Err(error) => {
            format!(
                "Failed to stop PID {} on port {}: {error}.",
                row.pid, row.port
            )
        }
    }
}

fn render_loading_shell(stdout: &mut std::io::Stdout, top: u16, query: &PortQuery) -> Result<()> {
    clear_region(stdout, top)?;

    let title = ports_title(query);
    loading::render_shell(stdout, top, &title, "Preparing the first page.")
}

fn render_loading_frame(stdout: &mut std::io::Stdout, top: u16, frame: usize) -> Result<()> {
    loading::render_frame(stdout, top, "Loading TCP ports", frame)
}

fn render_full(stdout: &mut std::io::Stdout, top: u16, menu: &PortsMenu) -> Result<()> {
    clear_region(stdout, top)?;

    let page_count = menu.page_count();
    let title = ports_title(&menu.query);
    let (width, _) = size().unwrap_or((100, 30));
    let (local_width, remote_width, _) = column_widths(width);

    queue!(
        stdout,
        MoveTo(0, top),
        SetForegroundColor(Color::DarkGrey),
        Print(title),
        ResetColor,
        MoveTo(0, top + 2),
        SetForegroundColor(Color::DarkGrey),
        Print(format!(
            "{:>6} {:<13} {:<local_width$} {:<remote_width$} {}",
            "Port",
            "State",
            "Local",
            "Remote",
            "Process",
            local_width = local_width,
            remote_width = remote_width
        )),
        ResetColor
    )?;

    for index in 0..PAGE_SIZE {
        render_row(stdout, top, menu, index)?;
    }

    render_nav(stdout, top, menu)?;
    render_status(stdout, top, menu)?;
    render_help(stdout, top)?;
    queue!(
        stdout,
        MoveTo(0, top + MENU_HEIGHT - 1),
        SetForegroundColor(Color::DarkGrey),
        Print(format!("Page {}/{}", menu.page + 1, page_count)),
        ResetColor
    )?;

    stdout.flush().context("failed to render ports menu")
}

fn render_selection_change(
    stdout: &mut std::io::Stdout,
    top: u16,
    menu: &PortsMenu,
    old_focus: Focus,
    old_row: usize,
    new_focus: Focus,
    new_row: usize,
) -> Result<()> {
    if matches!(old_focus, Focus::Row | Focus::Kill) {
        render_row(stdout, top, menu, old_row)?;
    }

    if matches!(new_focus, Focus::Row | Focus::Kill)
        && (new_row != old_row || old_focus != new_focus)
    {
        render_row(stdout, top, menu, new_row)?;
    }

    if matches!(old_focus, Focus::Prev | Focus::Next)
        || matches!(new_focus, Focus::Prev | Focus::Next)
    {
        render_nav(stdout, top, menu)?;
    }

    stdout.flush().context("failed to update ports menu")
}

fn render_row(
    stdout: &mut std::io::Stdout,
    top: u16,
    menu: &PortsMenu,
    index: usize,
) -> Result<()> {
    let y = top + ROW_START + index as u16;
    let (width, _) = size().unwrap_or((100, 30));
    queue!(stdout, MoveTo(0, y), Clear(ClearType::CurrentLine))?;

    let Some(row) = menu.visible_rows().get(index) else {
        return Ok(());
    };

    let (local_width, remote_width, process_width) = column_widths(width);
    let address = clamp(&row.address, local_width);
    let remote_address = clamp(&row.remote_address, remote_width);
    let process = clamp(&row.process, process_width);
    let line = format!(
        "{:>6} {:<13} {:<local_width$} {:<remote_width$} {:<process_width$}",
        row.port,
        row.state,
        address,
        remote_address,
        process,
        local_width = local_width,
        remote_width = remote_width,
        process_width = process_width
    );

    if menu.focus == Focus::Row && menu.selected_row == index {
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

    queue!(stdout, Print("  "))?;
    if menu.focus == Focus::Kill && menu.selected_row == index {
        queue!(
            stdout,
            SetForegroundColor(Color::White),
            SetBackgroundColor(Color::DarkRed),
            Print("[ Kill ]"),
            ResetColor
        )?;
    } else {
        queue!(
            stdout,
            SetForegroundColor(Color::Red),
            Print("[ Kill ]"),
            ResetColor
        )?;
    }

    Ok(())
}

fn render_nav(stdout: &mut std::io::Stdout, top: u16, menu: &PortsMenu) -> Result<()> {
    let y = top + ROW_START + PAGE_SIZE_U16 + 1;
    queue!(stdout, MoveTo(0, y), Clear(ClearType::CurrentLine))?;
    render_nav_button(stdout, "Prev", menu.focus == Focus::Prev, menu.page > 0)?;
    queue!(
        stdout,
        SetForegroundColor(Color::DarkGrey),
        Print(format!("  Page {}/{}  ", menu.page + 1, menu.page_count())),
        ResetColor
    )?;
    render_nav_button(
        stdout,
        "Next",
        menu.focus == Focus::Next,
        menu.page + 1 < menu.page_count(),
    )
}

fn render_nav_button(
    stdout: &mut std::io::Stdout,
    label: &str,
    selected: bool,
    enabled: bool,
) -> Result<()> {
    let text = format!("[ {label} ]");

    if selected && enabled {
        queue!(
            stdout,
            SetForegroundColor(Color::Black),
            SetBackgroundColor(Color::Yellow),
            Print(text),
            ResetColor
        )?;
    } else if selected {
        queue!(
            stdout,
            SetForegroundColor(Color::White),
            SetBackgroundColor(Color::DarkGrey),
            Print(text),
            ResetColor
        )?;
    } else if enabled {
        queue!(stdout, Print(text))?;
    } else {
        queue!(
            stdout,
            SetForegroundColor(Color::DarkGrey),
            Print(text),
            ResetColor
        )?;
    }

    Ok(())
}

fn render_status(stdout: &mut std::io::Stdout, top: u16, menu: &PortsMenu) -> Result<()> {
    let y = top + ROW_START + PAGE_SIZE_U16 + 2;
    queue!(stdout, MoveTo(0, y), Clear(ClearType::CurrentLine))?;

    let Some(status) = &menu.status else {
        return Ok(());
    };

    if status.starts_with("Failed") {
        queue!(
            stdout,
            SetForegroundColor(Color::Red),
            Print(status),
            ResetColor
        )?;
    } else {
        queue!(
            stdout,
            SetForegroundColor(Color::Green),
            Print(status),
            ResetColor
        )?;
    }

    Ok(())
}

fn render_help(stdout: &mut std::io::Stdout, top: u16) -> Result<()> {
    queue!(
        stdout,
        MoveTo(0, top + ROW_START + PAGE_SIZE_U16 + 4),
        Clear(ClearType::CurrentLine),
        SetForegroundColor(Color::DarkGrey),
        Print(
            "Use Up/Down. Enter opens details. Right selects Kill. R refreshes. PageUp/PageDown changes pages. Esc closes."
        ),
        ResetColor
    )?;
    Ok(())
}

fn render_detail(
    stdout: &mut std::io::Stdout,
    top: u16,
    row: &PortRow,
    status: Option<&str>,
) -> Result<()> {
    clear_region(stdout, top)?;

    queue!(
        stdout,
        MoveTo(0, top),
        SetForegroundColor(Color::DarkGrey),
        Print("TCP Port Detail"),
        ResetColor,
        MoveTo(0, top + 2),
        Print(format!("Port: {}", row.port)),
        MoveTo(0, top + 3),
        Print(format!("State: {}", row.state)),
        MoveTo(0, top + 4),
        Print(format!("Local: {}", row.address)),
        MoveTo(0, top + 5),
        Print(format!("Remote: {}", row.remote_address)),
        MoveTo(0, top + 6),
        Print(format!("PID: {}", row.pid)),
        MoveTo(0, top + 7),
        Print(format!("Process: {}", row.process)),
        MoveTo(0, top + 8),
        Print(format!(
            "Path: {}",
            if row.process_path.trim().is_empty() {
                "(unknown)"
            } else {
                &row.process_path
            }
        )),
        MoveTo(0, top + 10),
        SetForegroundColor(Color::White),
        SetBackgroundColor(Color::DarkRed),
        Print("[ Kill ]"),
        ResetColor,
        MoveTo(0, top + 12),
        SetForegroundColor(Color::DarkGrey),
        Print("Enter or K kills. Esc returns."),
        ResetColor
    )?;

    if let Some(status) = status {
        queue!(
            stdout,
            MoveTo(0, top + 14),
            SetForegroundColor(if status.starts_with("Failed") {
                Color::Red
            } else {
                Color::Green
            }),
            Print(status),
            ResetColor
        )?;
    }

    stdout.flush().context("failed to render port detail")
}

fn clear_region(stdout: &mut std::io::Stdout, top: u16) -> Result<()> {
    for offset in 0..MENU_HEIGHT {
        queue!(
            stdout,
            MoveTo(0, top + offset),
            Clear(ClearType::CurrentLine)
        )?;
    }
    queue!(stdout, MoveTo(0, top))?;
    stdout.flush().context("failed to clear ports menu")
}

fn print_plain(rows: &[PortRow], query: &PortQuery) {
    println!("{}", ports_title(query));

    println!(
        "{:>6} {:<13} {:<22} {:<26} Process",
        "Port", "State", "Local", "Remote"
    );
    for row in rows {
        println!(
            "{:>6} {:<13} {:<22} {:<26} {}",
            row.port, row.state, row.address, row.remote_address, row.process
        );
    }
}

fn column_widths(terminal_width: u16) -> (usize, usize, usize) {
    let available = usize::from(terminal_width).saturating_sub(33).max(32);
    let local_target = (available * 30 / 100).clamp(10, 22);
    let local_width = local_target.min(available.saturating_sub(20));

    let remaining = available.saturating_sub(local_width);
    let remote_target = (remaining * 45 / 100).clamp(10, 24);
    let remote_width = remote_target.min(remaining.saturating_sub(10));
    let process_width = remaining.saturating_sub(remote_width).max(10);

    (local_width, remote_width, process_width)
}

fn ports_title(query: &PortQuery) -> String {
    let mut title = if let Some(port) = query.port {
        format!("TCP Ports: {port}")
    } else {
        "TCP Ports".to_string()
    };

    if let Some(state) = &query.state
        && !state.trim().is_empty()
    {
        title.push_str(&format!(" | state: {state}"));
    }

    if let Some(name) = &query.name
        && !name.trim().is_empty()
    {
        title.push_str(&format!(" | process: {name}"));
    }

    title.push_str(&format!(" | sort: {}", sort_label(query.sort)));
    if query.refresh {
        title.push_str(" | live refresh");
    }

    title
}

fn sort_label(sort: PortsSort) -> &'static str {
    match sort {
        PortsSort::Port => "port",
        PortsSort::State => "state",
        PortsSort::Process => "process",
    }
}

fn clamp(value: &str, max: usize) -> String {
    if max < 4 || value.chars().count() <= max {
        return value.to_string();
    }

    let mut text = value.chars().take(max - 3).collect::<String>();
    text.push_str("...");
    text
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
    fn parses_ipv4_netstat_line() {
        let parsed = parse_netstat_line("  TCP    0.0.0.0:135    0.0.0.0:0    LISTENING    1632")
            .expect("line should parse");

        assert_eq!(parsed.address, "0.0.0.0");
        assert_eq!(parsed.port, 135);
        assert_eq!(parsed.remote_address, "0.0.0.0");
        assert_eq!(parsed.state, "LISTENING");
        assert_eq!(parsed.pid, 1632);
    }

    #[test]
    fn parses_ipv6_netstat_line() {
        let parsed = parse_netstat_line("  TCP    [::]:445    [::]:0    LISTENING    4")
            .expect("line should parse");

        assert_eq!(parsed.address, "::");
        assert_eq!(parsed.port, 445);
        assert_eq!(parsed.remote_address, "::");
        assert_eq!(parsed.state, "LISTENING");
        assert_eq!(parsed.pid, 4);
    }

    #[test]
    fn parses_established_netstat_line() {
        let parsed =
            parse_netstat_line("  TCP    127.0.0.1:5173    127.0.0.1:64120    ESTABLISHED    9480")
                .expect("line should parse");

        assert_eq!(parsed.address, "127.0.0.1");
        assert_eq!(parsed.port, 5173);
        assert_eq!(parsed.remote_address, "127.0.0.1:64120");
        assert_eq!(parsed.state, "ESTABLISHED");
        assert_eq!(parsed.pid, 9480);
    }

    #[test]
    fn displays_common_tcp_states() {
        assert_eq!(display_tcp_state("LISTENING"), "Listen");
        assert_eq!(display_tcp_state("ESTABLISHED"), "Established");
        assert_eq!(display_tcp_state("TIME_WAIT"), "Time Wait");
        assert_eq!(display_tcp_state("CLOSE_WAIT"), "Close Wait");
    }

    #[test]
    fn matches_state_filters_flexibly() {
        assert!(tcp_state_matches("ESTABLISHED", "established"));
        assert!(tcp_state_matches("TIME_WAIT", "time-wait"));
        assert!(tcp_state_matches("TIME_WAIT", "time wait"));
        assert!(tcp_state_matches("LISTENING", "listen"));
        assert!(tcp_state_matches("LISTENING", "listening"));
        assert!(!tcp_state_matches("CLOSE_WAIT", "established"));
    }

    #[test]
    fn sorts_rows_by_requested_field() {
        let mut rows = vec![
            PortRow {
                port: 3000,
                state: "Established".to_string(),
                address: "127.0.0.1".to_string(),
                remote_address: "127.0.0.1:1".to_string(),
                pid: 2,
                process: "zeta (2)".to_string(),
                process_path: String::new(),
            },
            PortRow {
                port: 1000,
                state: "Listen".to_string(),
                address: "127.0.0.1".to_string(),
                remote_address: "0.0.0.0".to_string(),
                pid: 1,
                process: "alpha (1)".to_string(),
                process_path: String::new(),
            },
        ];

        sort_rows(&mut rows, PortsSort::Process);
        assert_eq!(rows[0].process, "alpha (1)");

        sort_rows(&mut rows, PortsSort::Port);
        assert_eq!(rows[0].port, 1000);
    }

    #[test]
    fn computes_page_count() {
        let rows = vec![
            PortRow {
                port: 1,
                state: "Listen".to_string(),
                address: "127.0.0.1".to_string(),
                remote_address: "0.0.0.0".to_string(),
                pid: 1,
                process: "test (1)".to_string(),
                process_path: String::new(),
            };
            45
        ];

        let menu = PortsMenu::new(
            rows,
            PortQuery {
                port: None,
                state: None,
                name: None,
                refresh: false,
                sort: PortsSort::Port,
            },
        );
        assert_eq!(menu.page_count(), 3);
    }

    #[test]
    fn loading_frame_cycles() {
        assert_eq!(loading::frame(0), "[.  ]");
        assert_eq!(loading::frame(1), "[.. ]");
        assert_eq!(loading::frame(2), "[...]");
        assert_eq!(loading::frame(6), "[.  ]");
    }
}
