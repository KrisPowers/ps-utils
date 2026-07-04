use std::{
    env,
    ffi::OsString,
    io::{IsTerminal, stdin, stdout},
};

use anyhow::Result;
use clap::Args;

use crate::picker::{Picker, PickerItem};

#[derive(Debug, Args)]
pub struct EnvsArgs {
    /// Filter variable names or values.
    #[arg(short = 'q', long = "query")]
    pub query: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct EnvRow {
    key: String,
    value: String,
}

pub fn run(args: EnvsArgs) -> Result<()> {
    let rows = env_rows(args.query.as_deref());

    if !stdin().is_terminal() || !stdout().is_terminal() {
        print_plain(&rows);
        return Ok(());
    }

    let Some(selected) = select_env(&rows)? else {
        return Ok(());
    };

    println!("{}={}", selected.key, selected.value);
    Ok(())
}

fn env_rows(filter: Option<&str>) -> Vec<EnvRow> {
    let filter = filter.map(|value| value.to_lowercase());
    let mut rows = env::vars_os()
        .filter_map(|(key, value)| {
            let key = os_to_string(key);
            let value = os_to_string(value);

            if let Some(filter) = &filter {
                let haystack = format!("{} {}", key.to_lowercase(), value.to_lowercase());
                if !haystack.contains(filter) {
                    return None;
                }
            }

            Some(EnvRow { key, value })
        })
        .collect::<Vec<_>>();

    rows.sort_by(|left, right| left.key.cmp(&right.key));
    rows
}

fn select_env(rows: &[EnvRow]) -> Result<Option<EnvRow>> {
    let items = rows
        .iter()
        .map(|row| PickerItem::new(row.key.clone(), row.value.clone()))
        .collect();

    let selected = Picker::new(
        "Environment Variables",
        "Name                              Value",
        items,
    )
    .help("Use Up/Down. Enter prints the selected variable. Esc closes.")
    .select()?;

    Ok(selected.and_then(|index| rows.get(index).cloned()))
}

fn print_plain(rows: &[EnvRow]) {
    println!("{:<32} Value", "Name");
    for row in rows {
        println!("{:<32} {}", row.key, row.value);
    }
}

fn os_to_string(value: OsString) -> String {
    value.to_string_lossy().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn os_values_convert_lossily() {
        assert_eq!(os_to_string(OsString::from("PATH")), "PATH");
    }
}
