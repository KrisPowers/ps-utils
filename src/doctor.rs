use std::{env, fs, path::Path, path::PathBuf};

use anyhow::Result;

use crate::{
    config,
    profile::{self, ShellTarget},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CheckLevel {
    Ok,
    Warn,
    Error,
}

#[derive(Debug)]
struct Check {
    level: CheckLevel,
    name: String,
    message: String,
}

pub fn run() -> Result<()> {
    let mut checks = Vec::new();

    checks.push(check_config_dir());
    checks.extend(check_configs());
    checks.extend(check_profiles());
    checks.push(check_path());
    checks.extend(check_saved_paths());
    checks.push(check_history_dir());
    checks.push(check_sessions_file());

    println!("ps doctor");
    for check in &checks {
        println!(
            "[{}] {:<22} {}",
            level_label(check.level),
            check.name,
            check.message
        );
    }

    let errors = checks
        .iter()
        .filter(|check| check.level == CheckLevel::Error)
        .count();
    let warnings = checks
        .iter()
        .filter(|check| check.level == CheckLevel::Warn)
        .count();

    println!();
    println!("{errors} errors, {warnings} warnings");

    Ok(())
}

fn check_config_dir() -> Check {
    let dir = config::config_dir();
    match fs::create_dir_all(&dir) {
        Ok(()) => ok("config directory", dir.display().to_string()),
        Err(error) => check_error("config directory", format!("{}: {error}", dir.display())),
    }
}

fn check_configs() -> Vec<Check> {
    let mut checks = Vec::new();

    checks.push(match config::load_commands() {
        Ok(config) => ok(
            "commands.json",
            format!("{} commands", config.commands.len()),
        ),
        Err(error) => check_error("commands.json", error.to_string()),
    });

    checks.push(match config::load_paths_config() {
        Ok(config) => ok("paths.json", format!("{} saved paths", config.saved.len())),
        Err(error) => check_error("paths.json", error.to_string()),
    });

    checks.push(match config::load_settings_config() {
        Ok(_) => ok("settings.json", "valid"),
        Err(error) => check_error("settings.json", error.to_string()),
    });

    checks
}

fn check_profiles() -> Vec<Check> {
    let profiles = match profile::profile_paths(ShellTarget::All) {
        Ok(profiles) => profiles,
        Err(error) => return vec![check_error("profiles", error.to_string())],
    };

    profiles
        .into_iter()
        .map(|path| match fs::read_to_string(&path) {
            Ok(contents) => {
                if contents.contains("# >>> ps cli >>>") && contents.contains("# <<< ps cli <<<") {
                    ok("profile bridge", path.display().to_string())
                } else {
                    warn(
                        "profile bridge",
                        format!("missing ps block in {}", path.display()),
                    )
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                warn("profile bridge", format!("missing {}", path.display()))
            }
            Err(error) => check_error("profile bridge", format!("{}: {error}", path.display())),
        })
        .collect()
}

fn check_path() -> Check {
    let exe = match env::current_exe() {
        Ok(exe) => exe,
        Err(error) => return warn("PATH", format!("failed to read current exe: {error}")),
    };

    let Some(parent) = exe.parent() else {
        return warn("PATH", "current exe has no parent directory");
    };

    let in_path = env::var_os("PATH")
        .map(|paths| env::split_paths(&paths).any(|path| same_path(&path, parent)))
        .unwrap_or(false);

    if in_path {
        ok("PATH", format!("{} is available", parent.display()))
    } else {
        warn("PATH", format!("{} is not in PATH", parent.display()))
    }
}

fn check_saved_paths() -> Vec<Check> {
    let paths = match config::load_paths_config() {
        Ok(paths) => paths,
        Err(_) => return Vec::new(),
    };

    let missing = paths
        .saved
        .iter()
        .filter(|saved| !Path::new(&saved.path).exists())
        .count();

    if missing == 0 {
        vec![ok("saved paths", "all saved paths exist")]
    } else {
        vec![warn(
            "saved paths",
            format!("{missing} saved paths are missing"),
        )]
    }
}

fn check_history_dir() -> Check {
    let path = config::config_dir().join("history");
    if !path.exists() {
        return warn("history", "history directory does not exist yet");
    }

    match fs::read_dir(&path) {
        Ok(entries) => ok("history", format!("{} files", entries.count())),
        Err(error) => warn("history", format!("{}: {error}", path.display())),
    }
}

fn check_sessions_file() -> Check {
    let path = config::config_dir().join("sessions.json");
    if !path.exists() {
        return warn("sessions.json", "session history file does not exist yet");
    }

    match fs::read_to_string(&path) {
        Ok(_) => ok("sessions.json", "readable"),
        Err(error) => warn("sessions.json", format!("{}: {error}", path.display())),
    }
}

fn same_path(left: &Path, right: &Path) -> bool {
    let left = normalize_path(left);
    let right = normalize_path(right);

    if cfg!(windows) {
        left.to_string_lossy()
            .eq_ignore_ascii_case(&right.to_string_lossy())
    } else {
        left == right
    }
}

fn normalize_path(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

fn ok(name: impl Into<String>, message: impl Into<String>) -> Check {
    Check {
        level: CheckLevel::Ok,
        name: name.into(),
        message: message.into(),
    }
}

fn warn(name: impl Into<String>, message: impl Into<String>) -> Check {
    Check {
        level: CheckLevel::Warn,
        name: name.into(),
        message: message.into(),
    }
}

fn check_error(name: impl Into<String>, message: impl Into<String>) -> Check {
    Check {
        level: CheckLevel::Error,
        name: name.into(),
        message: message.into(),
    }
}

fn level_label(level: CheckLevel) -> &'static str {
    match level {
        CheckLevel::Ok => "OK",
        CheckLevel::Warn => "WARN",
        CheckLevel::Error => "ERROR",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_check_levels() {
        assert_eq!(level_label(CheckLevel::Ok), "OK");
        assert_eq!(level_label(CheckLevel::Warn), "WARN");
        assert_eq!(level_label(CheckLevel::Error), "ERROR");
    }
}
