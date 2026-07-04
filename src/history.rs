use std::{
    fs,
    io::{IsTerminal, stdin, stdout},
    path::PathBuf,
};

use anyhow::{Context, Result};
use clap::Args;
use serde::Deserialize;

use crate::{
    config,
    picker::{Picker, PickerItem},
};

#[derive(Debug, Args)]
pub struct HistoryArgs {
    /// Filter commands or paths.
    #[arg(short = 'q', long = "query")]
    pub query: Option<String>,

    /// Write the selected command to this file for the profile bridge.
    #[arg(long, hide = true)]
    pub result: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct HistoryRow {
    command: String,
    path: String,
    timestamp: u64,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct HistoryFile {
    commands: Vec<HistoryEntry>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct HistoryEntry {
    command: String,
    path: String,
    timestamp: u64,
}

pub fn run(args: HistoryArgs) -> Result<()> {
    let rows = history_rows(args.query.as_deref())?;

    if !stdin().is_terminal() || !stdout().is_terminal() {
        print_plain(&rows);
        return Ok(());
    }

    let Some(selected) = select_history(&rows)? else {
        return Ok(());
    };

    if let Some(result) = args.result {
        fs::write(&result, &selected.command)
            .with_context(|| format!("failed to write history result {}", result.display()))?;
    } else {
        println!("{}", selected.command);
        println!("{}", selected.path);
    }

    Ok(())
}

fn history_rows(filter: Option<&str>) -> Result<Vec<HistoryRow>> {
    let history_dir = config::config_dir().join("history");
    if !history_dir.exists() {
        return Ok(Vec::new());
    }

    let filter = filter.map(|value| value.to_lowercase());
    let mut rows = Vec::new();

    for entry in fs::read_dir(&history_dir)
        .with_context(|| format!("failed to read history directory {}", history_dir.display()))?
    {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            continue;
        }

        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) != Some("json") {
            continue;
        }

        let raw = match fs::read_to_string(&path) {
            Ok(raw) => raw,
            Err(_) => continue,
        };

        let parsed = match serde_json::from_str::<HistoryFile>(&raw) {
            Ok(parsed) => parsed,
            Err(_) => continue,
        };

        for command in parsed.commands {
            if command.command.trim().is_empty() {
                continue;
            }

            if let Some(filter) = &filter {
                let haystack = format!(
                    "{} {}",
                    command.command.to_lowercase(),
                    command.path.to_lowercase()
                );
                if !haystack.contains(filter) {
                    continue;
                }
            }

            rows.push(HistoryRow {
                command: command.command,
                path: command.path,
                timestamp: command.timestamp,
            });
        }
    }

    rows.sort_by(|left, right| {
        right
            .timestamp
            .cmp(&left.timestamp)
            .then_with(|| left.command.cmp(&right.command))
    });
    rows.dedup_by(|left, right| left.command == right.command && left.path == right.path);

    Ok(rows)
}

fn select_history(rows: &[HistoryRow]) -> Result<Option<HistoryRow>> {
    let items = rows
        .iter()
        .map(|row| {
            PickerItem::new(
                row.command.clone(),
                format!("{}  {}", row.timestamp, row.path),
            )
        })
        .collect();

    let selected = Picker::new(
        "Command History",
        "Command                         Time and Path",
        items,
    )
    .help("Use Up/Down. Enter runs the selected command. Esc closes.")
    .select()?;

    Ok(selected.and_then(|index| rows.get(index).cloned()))
}

fn print_plain(rows: &[HistoryRow]) {
    println!("{:<12} {:<48} Path", "Timestamp", "Command");
    for row in rows {
        println!("{:<12} {:<48} {}", row.timestamp, row.command, row.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_history_file_is_empty() {
        assert!(HistoryFile::default().commands.is_empty());
    }
}
