use std::{
    env,
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Context, Result, anyhow, bail};

use crate::config::{self, ShortcutCommand};

pub fn run(name: &str, args: &[String]) -> Result<()> {
    if !valid_shortcut_name(name) {
        bail!("'{name}' is not a valid shortcut name");
    }

    let config = config::load_commands()?;
    let Some(command) = config.commands.get(name) else {
        bail!("shortcut '{name}' was not found. Run `ps commands` to edit shortcuts.");
    };

    match command {
        ShortcutCommand::KillPort { .. } => kill_port(args),
        ShortcutCommand::Workspace { .. } | ShortcutCommand::Shell { .. } => {
            println!(
                "`ps run {name}` cannot change the current PowerShell session. Run `{name}` directly after `ps init`, or reload your profile with `. $PROFILE`."
            );
            Ok(())
        }
    }
}

pub fn emit(name: &str, args: &[String]) -> Result<String> {
    if !valid_shortcut_name(name) {
        return Ok(format!(
            "Write-Error {}",
            ps_quote(&format!("'{name}' is not a valid shortcut name"))
        ));
    }

    let config = config::load_commands()?;
    let Some(command) = config.commands.get(name) else {
        return Ok(format!(
            "Write-Error {}",
            ps_quote(&format!("shortcut '{name}' was not found"))
        ));
    };

    let script = match command {
        ShortcutCommand::KillPort { .. } => emit_run_command(name, args)?,
        ShortcutCommand::Workspace {
            path, open_windows, ..
        } => emit_workspace(path, open_windows),
        ShortcutCommand::Shell {
            script,
            script_path,
            dot_source,
            ..
        } => emit_shell(script.as_deref(), script_path.as_deref(), *dot_source, args)?,
    };

    Ok(script)
}

pub fn valid_shortcut_name(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };

    (first.is_ascii_alphabetic() || first == '_')
        && chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
}

fn emit_run_command(name: &str, args: &[String]) -> Result<String> {
    let exe = env::current_exe().context("failed to resolve current executable")?;
    let exe = ps_quote(&exe.display().to_string());
    let name = ps_quote(name);
    let args = args
        .iter()
        .map(|arg| ps_quote(arg))
        .collect::<Vec<_>>()
        .join(" ");

    if args.is_empty() {
        Ok(format!("& {exe} run {name}"))
    } else {
        Ok(format!("& {exe} run {name} {args}"))
    }
}

fn emit_workspace(path: &str, open_windows: &[String]) -> String {
    let path = expand_path(path);
    let mut script = String::new();
    script.push_str(&format!(
        "Set-Location -LiteralPath {}\n",
        ps_quote_path(&path)
    ));

    if !open_windows.is_empty() {
        script.push_str("$__psShell = (Get-Command pwsh -ErrorAction SilentlyContinue).Source\n");
        script.push_str("if (-not $__psShell) { $__psShell = (Get-Command powershell -ErrorAction SilentlyContinue).Source }\n");
        script.push_str("if (-not $__psShell) { throw 'Could not find pwsh or powershell to open workspace windows.' }\n");

        for window in open_windows {
            let window_path = expand_path(window);
            let quoted_path = ps_quote_path(&window_path);
            let inner = format!("Set-Location -LiteralPath {quoted_path}");
            script.push_str(&format!(
                "Start-Process -FilePath $__psShell -ArgumentList @('-NoExit', '-Command', {})\n",
                ps_quote(&inner)
            ));
        }
    }

    script
}

fn emit_shell(
    script: Option<&str>,
    script_path: Option<&str>,
    dot_source: bool,
    args: &[String],
) -> Result<String> {
    if script.is_none() && script_path.is_none() {
        bail!("shell shortcut must define script, script_path, or both");
    }

    let args = args
        .iter()
        .map(|arg| ps_quote(arg))
        .collect::<Vec<_>>()
        .join(", ");

    let mut emitted = String::new();
    emitted.push_str(&format!("$psArgs = @({args})\n"));

    if let Some(script_path) = script_path {
        emitted.push_str(&emit_script_path(script_path, dot_source));
    }

    if let Some(script) = script {
        emitted.push_str(script);
        if !script.ends_with('\n') {
            emitted.push('\n');
        }
    }

    Ok(emitted)
}

fn emit_script_path(script_path: &str, dot_source: bool) -> String {
    let script_path = expand_path(script_path);
    let operator = if dot_source { "." } else { "&" };
    let mut emitted = String::new();

    emitted.push_str(&format!(
        "$__psScriptPath = {}\n",
        ps_quote_path(&script_path)
    ));
    emitted.push_str("if (-not (Test-Path -LiteralPath $__psScriptPath)) {\n");
    emitted.push_str("    throw \"ps script file not found: $__psScriptPath\"\n");
    emitted.push_str("}\n");
    emitted.push_str(&format!("{operator} $__psScriptPath @psArgs\n"));

    emitted
}

fn kill_port(args: &[String]) -> Result<()> {
    let Some(port) = args.first() else {
        bail!("usage: kill <port>");
    };

    let port: u16 = port
        .parse()
        .with_context(|| format!("'{port}' is not a valid TCP port"))?;

    let pids = pids_for_port(port)?;
    if pids.is_empty() {
        println!("No process is listening on port {port}.");
        return Ok(());
    }

    for pid in pids {
        let status = Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/F"])
            .status()
            .with_context(|| format!("failed to run taskkill for PID {pid}"))?;

        if status.success() {
            println!("Stopped PID {pid} on port {port}.");
        } else {
            println!("taskkill exited with {status} for PID {pid}.");
        }
    }

    Ok(())
}

fn pids_for_port(port: u16) -> Result<Vec<u32>> {
    #[cfg(not(windows))]
    {
        let _ = port;
        bail!("kill-port shortcuts are currently implemented for Windows only");
    }

    #[cfg(windows)]
    {
        let script = format!(
            "Get-NetTCPConnection -LocalPort {port} -ErrorAction SilentlyContinue | Select-Object -ExpandProperty OwningProcess -Unique"
        );
        let output = Command::new("powershell.exe")
            .args([
                "-NoProfile",
                "-ExecutionPolicy",
                "Bypass",
                "-Command",
                &script,
            ])
            .output()
            .context("failed to query TCP connections with PowerShell")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            if stderr.is_empty() {
                bail!("PowerShell failed while querying port {port}");
            }
            bail!("PowerShell failed while querying port {port}: {stderr}");
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let mut pids = Vec::new();

        for line in stdout
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
        {
            let pid = line
                .parse::<u32>()
                .map_err(|_| anyhow!("PowerShell returned an invalid PID: {line}"))?;
            if !pids.contains(&pid) {
                pids.push(pid);
            }
        }

        Ok(pids)
    }
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

fn ps_quote_path(path: &Path) -> String {
    ps_quote(&path.display().to_string())
}

fn ps_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_shortcut_names() {
        assert!(valid_shortcut_name("workbench"));
        assert!(valid_shortcut_name("open-api"));
        assert!(valid_shortcut_name("_secret"));
        assert!(!valid_shortcut_name(""));
        assert!(!valid_shortcut_name("123"));
        assert!(!valid_shortcut_name("bad;name"));
    }

    #[test]
    fn quotes_powershell_literals() {
        assert_eq!(ps_quote("C:\\Users\\Kris"), "'C:\\Users\\Kris'");
        assert_eq!(ps_quote("Kris's Repo"), "'Kris''s Repo'");
    }

    #[test]
    fn emits_script_file_shortcuts() {
        let emitted = emit_shell(None, Some("~/tools/start.ps1"), true, &["one".to_string()])
            .expect("script path should emit");

        assert!(emitted.contains("$psArgs = @('one')"));
        assert!(emitted.contains("$__psScriptPath = "));
        assert!(emitted.contains("Test-Path -LiteralPath $__psScriptPath"));
        assert!(emitted.contains(". $__psScriptPath @psArgs"));
    }

    #[test]
    fn can_execute_script_files_without_dot_sourcing() {
        let emitted = emit_shell(None, Some("C:\\tools\\start.ps1"), false, &[])
            .expect("script path should emit");

        assert!(emitted.contains("& $__psScriptPath @psArgs"));
        assert!(!emitted.contains(". $__psScriptPath @psArgs"));
    }
}
