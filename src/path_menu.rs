use std::{
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result};

use crate::{
    config::{self, PathsConfig, SavedPath},
    menu::{Menu, MenuItem},
};

#[derive(Debug, Clone)]
enum PathMenuItem {
    SaveCurrent,
    Saved(SavedPath),
}

pub fn run(current_path: Option<PathBuf>) -> Result<String> {
    let current_path = current_path
        .or_else(|| std::env::current_dir().ok())
        .context("failed to resolve the current path")?;
    let current_path = normalize_path(&current_path);
    let mut paths = config::load_paths_config()?;
    let items = menu_items(&paths);
    let selected = select(&items, &current_path)?;

    match selected {
        PathMenuItem::SaveCurrent => save_current_path(&mut paths, &current_path),
        PathMenuItem::Saved(saved) => open_saved_path(&mut paths, &saved),
    }
}

fn menu_items(paths: &PathsConfig) -> Vec<PathMenuItem> {
    let mut saved = paths.saved.clone();
    saved.sort_by(|left, right| {
        right
            .last_accessed
            .cmp(&left.last_accessed)
            .then_with(|| left.name.cmp(&right.name))
    });

    let mut items = vec![PathMenuItem::SaveCurrent];
    items.extend(saved.into_iter().map(PathMenuItem::Saved));
    items
}

fn select(items: &[PathMenuItem], current_path: &Path) -> Result<PathMenuItem> {
    let menu_items = items
        .iter()
        .map(|item| match item {
            PathMenuItem::SaveCurrent => {
                MenuItem::new("Save current path").detail(current_path.display().to_string())
            }
            PathMenuItem::Saved(saved) => MenuItem::new(&saved.name).description(&saved.path),
        })
        .collect();
    let selected = Menu::new("@ saved paths", menu_items)
        .help("Use Up/Down. Enter selects. Esc cancels.")
        .note(Some(format!("Current: {}", current_path.display())))
        .cancel_message("path menu canceled")
        .select()
        .context("the @ path menu requires an interactive terminal")?;

    Ok(items[selected].clone())
}

fn save_current_path(paths: &mut PathsConfig, current_path: &Path) -> Result<String> {
    let path = current_path.display().to_string();
    let now = now_secs();
    let name = path_name(current_path);

    if let Some(saved) = paths
        .saved
        .iter_mut()
        .find(|saved| same_path(&saved.path, &path))
    {
        saved.last_accessed = now;
    } else {
        paths.saved.push(SavedPath {
            name: unique_name(paths, &name),
            path: path.clone(),
            last_accessed: now,
        });
    }

    config::save_paths_config(paths)?;
    Ok(format!(
        "Write-Host {}\n",
        ps_quote(&format!("Saved path: {path}"))
    ))
}

fn open_saved_path(paths: &mut PathsConfig, selected: &SavedPath) -> Result<String> {
    let now = now_secs();

    if let Some(saved) = paths
        .saved
        .iter_mut()
        .find(|saved| same_path(&saved.path, &selected.path))
    {
        saved.last_accessed = now;
    }

    config::save_paths_config(paths)?;
    Ok(format!(
        "Set-Location -LiteralPath {}\n",
        ps_quote(&selected.path)
    ))
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

fn path_name(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.trim().is_empty())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| path.display().to_string())
}

fn unique_name(paths: &PathsConfig, name: &str) -> String {
    if !paths.saved.iter().any(|saved| saved.name == name) {
        return name.to_string();
    }

    let mut counter = 2;
    loop {
        let candidate = format!("{name}-{counter}");
        if !paths.saved.iter().any(|saved| saved.name == candidate) {
            return candidate;
        }
        counter += 1;
    }
}

fn same_path(left: &str, right: &str) -> bool {
    if cfg!(windows) {
        left.eq_ignore_ascii_case(right)
    } else {
        left == right
    }
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn ps_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_current_is_always_first() {
        let config = PathsConfig {
            version: 1,
            saved: vec![SavedPath {
                name: "repo".to_string(),
                path: "C:\\repo".to_string(),
                last_accessed: 10,
            }],
        };

        assert!(matches!(menu_items(&config)[0], PathMenuItem::SaveCurrent));
    }

    #[test]
    fn saved_paths_sort_by_last_accessed_descending() {
        let config = PathsConfig {
            version: 1,
            saved: vec![
                SavedPath {
                    name: "old".to_string(),
                    path: "C:\\old".to_string(),
                    last_accessed: 1,
                },
                SavedPath {
                    name: "new".to_string(),
                    path: "C:\\new".to_string(),
                    last_accessed: 2,
                },
            ],
        };

        let items = menu_items(&config);
        let PathMenuItem::Saved(saved) = &items[1] else {
            panic!("expected saved path");
        };

        assert_eq!(saved.name, "new");
    }

    #[test]
    fn quotes_powershell_literals() {
        assert_eq!(ps_quote("C:\\Kris's Repo"), "'C:\\Kris''s Repo'");
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
