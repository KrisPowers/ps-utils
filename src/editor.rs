use std::{env, ffi::OsStr, path::Path, process::Command};

use anyhow::{Context, Result};

pub fn open(path: &Path) -> Result<()> {
    if let Some(editor) = env::var_os("VISUAL").or_else(|| env::var_os("EDITOR")) {
        spawn(editor, path, "editor")
    } else {
        open_default(path)
    }
}

#[cfg(windows)]
fn open_default(path: &Path) -> Result<()> {
    spawn("notepad.exe", path, "notepad")
}

#[cfg(target_os = "macos")]
fn open_default(path: &Path) -> Result<()> {
    spawn("open", path, "open")
}

#[cfg(all(unix, not(target_os = "macos")))]
fn open_default(path: &Path) -> Result<()> {
    spawn("xdg-open", path, "xdg-open")
}

#[cfg(not(any(windows, unix)))]
fn open_default(path: &Path) -> Result<()> {
    anyhow::bail!(
        "no editor or opener was found; config path is {}",
        path.display()
    )
}

fn spawn(program: impl AsRef<OsStr>, path: &Path, label: &str) -> Result<()> {
    Command::new(program)
        .arg(path)
        .spawn()
        .with_context(|| format!("failed to open {label} for {}", path.display()))?;
    Ok(())
}
