use std::{
    io::{IsTerminal, stdin, stdout},
    path::Path,
    process::Command,
};

use anyhow::{Context, Result};
use clap::Args;
use sysinfo::System;

use crate::picker::{Picker, PickerItem};

#[derive(Debug, Args)]
pub struct ProcsArgs {
    /// Filter process names or paths.
    #[arg(short = 'n', long = "name")]
    pub name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ProcessRow {
    pid: u32,
    name: String,
    memory_mb: u64,
    path: String,
}

pub fn run(args: ProcsArgs) -> Result<()> {
    let rows = process_rows(args.name.as_deref());

    if !stdin().is_terminal() || !stdout().is_terminal() {
        print_plain(&rows);
        return Ok(());
    }

    let Some(selected) = select_process(&rows)? else {
        return Ok(());
    };

    show_process_actions(&selected)
}

fn process_rows(filter: Option<&str>) -> Vec<ProcessRow> {
    let mut system = System::new_all();
    system.refresh_processes();

    let filter = filter.map(|value| value.to_lowercase());
    let mut rows = system
        .processes()
        .values()
        .filter_map(|process| {
            let pid = process.pid().as_u32();
            let name = clean_process_name(process.name());
            let path = process
                .exe()
                .map(|path| path.display().to_string())
                .unwrap_or_default();

            if let Some(filter) = &filter {
                let haystack = format!("{} {}", name.to_lowercase(), path.to_lowercase());
                if !haystack.contains(filter) {
                    return None;
                }
            }

            Some(ProcessRow {
                pid,
                name,
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
                format!("{}  {}", row.name, row.path),
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
    println!("{:>7} {:>9}  {:<28} Path", "PID", "Memory", "Name");
    for row in rows {
        println!(
            "{:>7} {:>6} MB  {:<28} {}",
            row.pid, row.memory_mb, row.name, row.path
        );
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_process_name_removes_extension() {
        assert_eq!(clean_process_name("pwsh.exe"), "pwsh");
    }
}
