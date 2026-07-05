use std::{
    collections::HashMap,
    io::{IsTerminal, stdin, stdout},
    path::Path,
    process::Command,
};

use anyhow::{Context, Result};
use clap::Args;
use serde::Deserialize;
use sysinfo::System;

use crate::{
    loading,
    picker::{Picker, PickerItem},
};

#[derive(Debug, Args)]
pub struct ProcsArgs {
    /// Filter process names or paths.
    #[arg(short = 'n', long = "name")]
    pub name: Option<String>,

    /// Filter process titles shown in the menu.
    #[arg(short = 't', long = "title")]
    pub title: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ProcessRow {
    pid: u32,
    name: String,
    title: String,
    memory_mb: u64,
    path: String,
}

pub fn run(args: ProcsArgs) -> Result<()> {
    if !stdin().is_terminal() || !stdout().is_terminal() {
        let rows = process_rows(args.name.as_deref(), args.title.as_deref());
        print_plain(&rows);
        return Ok(());
    }

    let name_filter = args.name.clone();
    let title_filter = args.title.clone();
    let rows = loading::load_with_terminal(
        "Processes",
        "Loading processes",
        "Preparing the process list.",
        "process menu canceled",
        move || {
            Ok(process_rows(
                name_filter.as_deref(),
                title_filter.as_deref(),
            ))
        },
    )?;

    let Some(selected) = select_process(&rows)? else {
        return Ok(());
    };

    show_process_actions(&selected)
}

fn process_rows(name_filter: Option<&str>, title_filter: Option<&str>) -> Vec<ProcessRow> {
    let mut system = System::new_all();
    system.refresh_processes();

    let name_filter = name_filter.map(|value| value.to_lowercase());
    let title_filter = title_filter.map(|value| value.to_lowercase());
    let titles = if title_filter.is_some() {
        process_window_titles()
    } else {
        HashMap::new()
    };

    let mut rows = system
        .processes()
        .values()
        .filter_map(|process| {
            let pid = process.pid().as_u32();
            let name = clean_process_name(process.name());
            let title = titles.get(&pid).cloned().unwrap_or_default();
            let path = process
                .exe()
                .map(|path| path.display().to_string())
                .unwrap_or_default();

            if let Some(filter) = &name_filter {
                let haystack = format!("{} {}", name.to_lowercase(), path.to_lowercase());
                if !haystack.contains(filter) {
                    return None;
                }
            }

            if let Some(filter) = &title_filter
                && !process_title_matches(&name, &title, filter)
            {
                return None;
            }

            Some(ProcessRow {
                pid,
                name,
                title,
                memory_mb: process.memory() / 1024 / 1024,
                path,
            })
        })
        .collect::<Vec<_>>();

    rows.sort_by(|left, right| {
        right
            .memory_mb
            .cmp(&left.memory_mb)
            .then_with(|| left.name.cmp(&right.name))
            .then_with(|| left.pid.cmp(&right.pid))
    });

    rows
}

fn select_process(rows: &[ProcessRow]) -> Result<Option<ProcessRow>> {
    let items = rows
        .iter()
        .map(|row| {
            PickerItem::new(
                format!("{:>7} {:>6} MB", row.pid, row.memory_mb),
                process_detail(row),
            )
        })
        .collect();

    let selected = Picker::new("Processes", "    PID    Memory   Process", items)
        .help("Use Up/Down. Enter opens actions. Esc closes.")
        .select()?;

    Ok(selected.and_then(|index| rows.get(index).cloned()))
}

fn show_process_actions(row: &ProcessRow) -> Result<()> {
    println!("PID: {}", row.pid);
    println!("Name: {}", row.name);
    if !row.title.trim().is_empty() {
        println!("Title: {}", row.title);
    }
    println!("Memory: {} MB", row.memory_mb);
    println!(
        "Path: {}",
        if row.path.trim().is_empty() {
            "(unknown)"
        } else {
            &row.path
        }
    );

    let items = vec![
        PickerItem::new("Kill process", format!("Stop PID {}", row.pid)),
        PickerItem::new("Close", "Return without changes"),
    ];

    let selected = Picker::new("Process Actions", "Action", items)
        .help("Use Up/Down. Enter selects. Esc closes.")
        .select()?;

    if selected == Some(0) {
        println!("{}", kill_process(row.pid)?);
    }

    Ok(())
}

fn kill_process(pid: u32) -> Result<String> {
    if cfg!(windows) {
        let status = Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/F"])
            .status()
            .context("failed to run taskkill")?;

        if status.success() {
            Ok(format!("Stopped PID {pid}."))
        } else {
            Ok(format!(
                "Failed to stop PID {pid}: taskkill exited with {status}."
            ))
        }
    } else {
        let status = Command::new("kill")
            .args(["-TERM", &pid.to_string()])
            .status()
            .context("failed to run kill")?;

        if status.success() {
            Ok(format!("Stopped PID {pid}."))
        } else {
            Ok(format!(
                "Failed to stop PID {pid}: kill exited with {status}."
            ))
        }
    }
}

fn print_plain(rows: &[ProcessRow]) {
    println!(
        "{:>7} {:>9}  {:<28} {:<32} Path",
        "PID", "Memory", "Name", "Title"
    );
    for row in rows {
        println!(
            "{:>7} {:>6} MB  {:<28} {:<32} {}",
            row.pid, row.memory_mb, row.name, row.title, row.path
        );
    }
}

fn process_detail(row: &ProcessRow) -> String {
    if row.title.trim().is_empty() {
        format!("{}  {}", row.name, row.path)
    } else {
        format!("{}  {}  {}", row.name, row.title, row.path)
    }
}

fn process_title_matches(name: &str, title: &str, filter: &str) -> bool {
    let filter = filter.to_lowercase();
    name.to_lowercase().contains(&filter) || title.to_lowercase().contains(&filter)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct WindowTitle {
    id: u32,
    main_window_title: String,
}

fn process_window_titles() -> HashMap<u32, String> {
    if !cfg!(windows) {
        return HashMap::new();
    }

    let command = r#"
$items = @(Get-Process | Where-Object {
    -not [string]::IsNullOrWhiteSpace($_.MainWindowTitle)
} | ForEach-Object {
    [pscustomobject]@{
        Id = $_.Id
        MainWindowTitle = $_.MainWindowTitle
    }
})
ConvertTo-Json -InputObject $items -Compress
"#;

    let output = Command::new("powershell.exe")
        .args(["-NoProfile", "-Command", command])
        .output();

    let Ok(output) = output else {
        return HashMap::new();
    };

    if !output.status.success() {
        return HashMap::new();
    }

    let raw = String::from_utf8_lossy(&output.stdout);
    let raw = raw.trim();
    if raw.is_empty() {
        return HashMap::new();
    }

    serde_json::from_str::<Vec<WindowTitle>>(raw)
        .unwrap_or_default()
        .into_iter()
        .filter(|title| !title.main_window_title.trim().is_empty())
        .map(|title| (title.id, title.main_window_title))
        .collect()
}

fn clean_process_name(name: &str) -> String {
    Path::new(name)
        .file_stem()
        .and_then(|value| value.to_str())
        .filter(|value| !value.trim().is_empty())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| name.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_process_name_removes_extension() {
        assert_eq!(clean_process_name("pwsh.exe"), "pwsh");
    }

    #[test]
    fn process_detail_includes_title_when_present() {
        let row = ProcessRow {
            pid: 1,
            name: "app".to_string(),
            title: "Project Window".to_string(),
            memory_mb: 10,
            path: "C:\\app.exe".to_string(),
        };

        assert!(process_detail(&row).contains("Project Window"));
    }

    #[test]
    fn title_filter_matches_visible_process_name_when_window_title_is_empty() {
        assert!(process_title_matches("MusicApp", "", "music"));
        assert!(process_title_matches("helper", "Project Window", "project"));
        assert!(!process_title_matches("helper", "", "project"));
    }
}
