mod config;
mod doctor;
mod editor;
mod envs;
mod history;
mod loading;
mod menu;
mod path_menu;
mod picker;
mod ports;
mod procs;
mod profile;
mod shortcut;
mod workspaces;

use std::{fs, path::PathBuf};

use anyhow::{Context, Result};
use clap::{Args, Parser, Subcommand, ValueEnum};

use crate::{
    config::{ConfigFile, config_path},
    profile::ShellTarget,
};

#[derive(Debug, Parser)]
#[command(name = "ps")]
#[command(
    version,
    about = "PowerShell utilities for saved paths and prefixless shortcuts."
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Install or refresh the PowerShell profile bridge.
    Init(InitArgs),

    /// Open the shortcut commands JSON file.
    Commands(OpenConfigArgs),

    /// Print a managed config path.
    ConfigPath {
        #[arg(value_enum)]
        file: ConfigPathArg,
    },

    /// Open the interactive TCP ports menu.
    Ports(ports::PortsArgs),

    /// Run health checks for the ps installation.
    Doctor,

    /// Open the interactive process menu.
    #[command(alias = "processes")]
    Procs(procs::ProcsArgs),

    /// Open the command history menu.
    History(history::HistoryArgs),

    /// Open the environment variables menu.
    Envs(envs::EnvsArgs),

    /// Manage and launch saved workspaces.
    Workspaces(workspaces::WorkspacesArgs),

    /// Run a shortcut in a child process.
    Run {
        name: String,
        #[arg(trailing_var_arg = true)]
        args: Vec<String>,
    },

    /// Emit PowerShell for profile-created shortcut functions.
    #[command(hide = true)]
    Emit {
        name: String,
        #[arg(trailing_var_arg = true)]
        args: Vec<String>,
    },

    /// Open the interactive saved-path menu for the profile bridge.
    #[command(hide = true)]
    PathMenu {
        #[arg(long)]
        current: Option<PathBuf>,

        #[arg(long)]
        result: Option<PathBuf>,
    },
}

#[derive(Debug, Args)]
struct InitArgs {
    /// Install to pwsh, Windows PowerShell, or both.
    #[arg(long, value_enum, default_value_t = ShellTarget::All)]
    shell: ShellTarget,

    /// Accepted for install scripts; init is non-interactive today.
    #[arg(long)]
    yes: bool,
}

#[derive(Debug, Args)]
struct OpenConfigArgs {
    /// Print the file path instead of opening it.
    #[arg(long)]
    path: bool,

    /// Print the file contents instead of opening it.
    #[arg(long)]
    print: bool,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum ConfigPathArg {
    Commands,
    Paths,
    Settings,
    Workspaces,
    Dir,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Command::Init(args) => init(args),
        Command::Commands(args) => open_config(ConfigFile::Commands, args),
        Command::ConfigPath { file } => {
            println!("{}", config_path(file.into()).display());
            Ok(())
        }
        Command::Ports(args) => ports::run(args),
        Command::Doctor => doctor::run(),
        Command::Procs(args) => procs::run(args),
        Command::History(args) => history::run(args),
        Command::Envs(args) => envs::run(args),
        Command::Workspaces(args) => workspaces::run(args),
        Command::Run { name, args } => shortcut::run(&name, &args),
        Command::Emit { name, args } => {
            println!("{}", shortcut::emit(&name, &args)?);
            Ok(())
        }
        Command::PathMenu { current, result } => {
            let script = path_menu::run(current)?;
            if let Some(result) = result {
                fs::write(&result, script).with_context(|| {
                    format!("failed to write path menu result {}", result.display())
                })?;
            } else {
                print!("{script}");
            }
            Ok(())
        }
    }
}

fn init(args: InitArgs) -> Result<()> {
    let _ = args.yes;
    let exe = std::env::current_exe().context("failed to resolve current executable")?;
    let results = profile::install(args.shell, &exe)?;

    println!("ps profile bridge installed.");
    println!(
        "Config directory: {}",
        config_path(ConfigFile::Dir).display()
    );

    for result in results {
        let status = if result.changed {
            "updated"
        } else {
            "already current"
        };
        println!("{status}: {}", result.path.display());
    }

    println!("Restart PowerShell or run `. $PROFILE` to load ps shortcuts.");
    Ok(())
}

fn open_config(file: ConfigFile, args: OpenConfigArgs) -> Result<()> {
    let path = match file {
        ConfigFile::Commands => config::ensure_commands_config()?,
        ConfigFile::Paths => config::ensure_paths_config()?,
        ConfigFile::Settings => config::ensure_settings_config()?,
        ConfigFile::Workspaces => config::ensure_workspaces_config()?,
        ConfigFile::Dir => config_path(ConfigFile::Dir),
    };

    if args.path {
        println!("{}", path.display());
        return Ok(());
    }

    if args.print {
        let contents = fs::read_to_string(&path)
            .with_context(|| format!("failed to read config file {}", path.display()))?;
        print!("{contents}");
        return Ok(());
    }

    editor::open(&path)
}

impl From<ConfigPathArg> for ConfigFile {
    fn from(value: ConfigPathArg) -> Self {
        match value {
            ConfigPathArg::Commands => ConfigFile::Commands,
            ConfigPathArg::Paths => ConfigFile::Paths,
            ConfigPathArg::Settings => ConfigFile::Settings,
            ConfigPathArg::Workspaces => ConfigFile::Workspaces,
            ConfigPathArg::Dir => ConfigFile::Dir,
        }
    }
}
