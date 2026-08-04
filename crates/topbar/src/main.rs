//! gnome-topbar — GNOME Shell-inspired GTK4 top bar for niri.
//!
//! M0 is the scaffold: the CLI surface, configuration loading/validation, and
//! logging are real; the panel UI itself lands from M1 onward.

#![warn(missing_docs)]

mod anim;
mod cli;
mod ipc_client;
mod panel;
mod style;

use std::process::ExitCode;

use clap::Parser;
use topbar_core::config::{Config, ConfigLoad, EXAMPLE_CONFIG_TOML, Warning};
use topbar_core::ipc::{self, IpcRequest, IpcResponse};
use topbar_core::logging;
use tracing::{debug, info, warn};

use crate::cli::{Cli, Command, DumpAction, PopoverAction, VisibilityAction};

fn main() -> ExitCode {
    let cli = Cli::parse();
    logging::init(cli.verbose);

    // --print-example-config is answered before any file is touched so it works
    // even when the user's config is broken.
    if cli.print_example_config {
        print!("{EXAMPLE_CONFIG_TOML}");
        return ExitCode::SUCCESS;
    }

    if let Some(command) = cli.command {
        return run_command(command);
    }

    let load = match Config::find_and_load(cli.config.as_deref()) {
        Ok(load) => load,
        Err(err) => {
            eprintln!("Error: {err}");
            return ExitCode::FAILURE;
        }
    };

    // `--check-config` reports the same warnings on stderr, so logging them too
    // would print every one twice.
    if !cli.check_config {
        for warning in &load.warnings {
            warn!("{warning}");
        }
    }

    if cli.strict && !load.warnings.is_empty() {
        eprintln!(
            "Error: {}",
            topbar_core::Error::StrictWarnings(
                load.warnings.iter().map(Warning::to_string).collect()
            )
        );
        return ExitCode::FAILURE;
    }

    if cli.check_config {
        return report_check(&load);
    }

    describe(&load);
    debug!("linked against {}", panel::linked_stack());
    info!("gnome-topbar v2 scaffold: UI not implemented yet (milestone M0)");
    println!("gnome-topbar v2 scaffold: UI not implemented yet (milestone M0)");
    ExitCode::SUCCESS
}

/// `--check-config` output: one status line, then every warning on stderr.
fn report_check(load: &ConfigLoad) -> ExitCode {
    match &load.source {
        Some(source) => println!("Configuration valid: {}", source.display()),
        None => println!("Configuration valid (using defaults)"),
    }
    for warning in &load.warnings {
        eprintln!("Warning: {warning}");
    }
    ExitCode::SUCCESS
}

fn describe(load: &ConfigLoad) {
    match &load.source {
        Some(source) => info!("Loaded configuration from {}", source.display()),
        None => warn!("No config file found; using built-in defaults"),
    }
    let widgets = &load.config.widgets;
    info!(
        "Bar {}px; widgets: {} left, {} center, {} right",
        load.config.bar.size,
        widgets.left.len(),
        widgets.center.len(),
        widgets.right.len()
    );
}

/// Dispatch a subcommand to the running panel.
///
/// Everything routes through the IPC socket in M0. The direct PulseAudio and
/// logind paths that make `volume`/`brightness` work without a running panel
/// land with M8.
fn run_command(command: Command) -> ExitCode {
    let request = match command {
        Command::Brightness { action } => {
            return unimplemented_locally("brightness", action_name_brightness(&action));
        }
        Command::Volume { action } => {
            return unimplemented_locally("volume", action_name_volume(&action));
        }
        Command::Inhibit { .. } => IpcRequest::ToggleInhibitor,
        Command::Media { action } => IpcRequest::Media {
            action: media_action(&action),
        },
        Command::Bar { action } => IpcRequest::Bar {
            action: visibility(&action),
        },
        Command::Popover { action } => IpcRequest::Popover {
            action: popover(action),
        },
        Command::Reload => IpcRequest::Reload,
        Command::Dump { action } => IpcRequest::Dump {
            target: match action {
                DumpAction::DefaultConfig => {
                    print!("{EXAMPLE_CONFIG_TOML}");
                    return ExitCode::SUCCESS;
                }
                DumpAction::Config => ipc::DumpTarget::Config,
                DumpAction::State => ipc::DumpTarget::State,
            },
        },
    };

    match ipc_client::request(&request) {
        Ok(IpcResponse::Ok) => ExitCode::SUCCESS,
        Ok(IpcResponse::Value { text }) => {
            println!("{text}");
            ExitCode::SUCCESS
        }
        Ok(IpcResponse::Error { message }) => {
            eprintln!("Error: {message}");
            ExitCode::FAILURE
        }
        Ok(IpcResponse::Hello { .. }) => {
            eprintln!("Error: panel replied with an unexpected handshake");
            ExitCode::FAILURE
        }
        Err(err) => {
            eprintln!("Error: {} ({err})", ipc_client::UNREACHABLE);
            ExitCode::FAILURE
        }
    }
}

/// Subcommands whose local (panel-free) path is not wired up yet.
///
/// They still try the socket first so the message matches the eventual
/// behaviour, then explain what is missing.
fn unimplemented_locally(group: &str, action: &str) -> ExitCode {
    match ipc_client::probe() {
        Ok(()) => eprintln!("Error: `{group} {action}` is not implemented yet (milestone M8)"),
        Err(_) => eprintln!("Error: {}", ipc_client::UNREACHABLE),
    }
    ExitCode::FAILURE
}

fn action_name_brightness(action: &cli::BrightnessAction) -> &'static str {
    match action {
        cli::BrightnessAction::Get => "get",
        cli::BrightnessAction::Set { .. } => "set",
        cli::BrightnessAction::Inc { .. } => "inc",
        cli::BrightnessAction::Dec { .. } => "dec",
    }
}

fn action_name_volume(action: &cli::VolumeAction) -> &'static str {
    match action {
        cli::VolumeAction::Get => "get",
        cli::VolumeAction::Set { .. } => "set",
        cli::VolumeAction::Inc { .. } => "inc",
        cli::VolumeAction::Dec { .. } => "dec",
        cli::VolumeAction::Mute => "mute",
        cli::VolumeAction::Unmute => "unmute",
        cli::VolumeAction::ToggleMute => "toggle-mute",
    }
}

fn media_action(action: &cli::MediaAction) -> ipc::MediaAction {
    match action {
        cli::MediaAction::PlayPause => ipc::MediaAction::PlayPause,
        cli::MediaAction::Next => ipc::MediaAction::Next,
        cli::MediaAction::Previous => ipc::MediaAction::Previous,
        cli::MediaAction::Stop => ipc::MediaAction::Stop,
        cli::MediaAction::Status => ipc::MediaAction::Status,
    }
}

fn visibility(action: &VisibilityAction) -> ipc::VisibilityAction {
    match action {
        VisibilityAction::Show => ipc::VisibilityAction::Show,
        VisibilityAction::Hide => ipc::VisibilityAction::Hide,
        VisibilityAction::Toggle => ipc::VisibilityAction::Toggle,
    }
}

fn popover(action: PopoverAction) -> ipc::PopoverAction {
    match action {
        PopoverAction::Show { widget } => ipc::PopoverAction::Show(widget),
        PopoverAction::Hide { widget } => ipc::PopoverAction::Hide(widget),
        PopoverAction::Toggle { widget } => ipc::PopoverAction::Toggle(widget),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn cli_definition_is_valid() {
        Cli::command().debug_assert();
    }

    #[test]
    fn v1_flags_still_parse() {
        let cli = Cli::try_parse_from([
            "gnome-topbar",
            "-c",
            "/tmp/config.toml",
            "-vv",
            "--check-config",
            "--strict",
        ])
        .expect("v1 flag surface must keep working");
        assert_eq!(
            cli.config.as_deref(),
            Some(std::path::Path::new("/tmp/config.toml"))
        );
        assert_eq!(cli.verbose, 2);
        assert!(cli.check_config);
        assert!(cli.strict);
    }

    #[test]
    fn v1_subcommands_still_parse() {
        for args in [
            vec!["gnome-topbar", "volume", "inc", "5"],
            vec!["gnome-topbar", "brightness", "set", "40"],
            vec!["gnome-topbar", "inhibit", "toggle"],
            vec!["gnome-topbar", "media", "play-pause"],
            vec!["gnome-topbar", "bar", "toggle"],
            vec!["gnome-topbar", "popover", "toggle", "clock"],
            vec!["gnome-topbar", "reload"],
            vec!["gnome-topbar", "dump", "default-config"],
        ] {
            Cli::try_parse_from(&args).unwrap_or_else(|err| panic!("{args:?}: {err}"));
        }
    }

    #[test]
    fn brightness_percent_is_range_checked() {
        assert!(Cli::try_parse_from(["gnome-topbar", "brightness", "set", "101"]).is_err());
    }

    #[test]
    fn example_config_is_embedded_and_parses() {
        let (config, warnings) =
            Config::parse(EXAMPLE_CONFIG_TOML).expect("embedded example must parse");
        assert_eq!(config, Config::default());
        assert!(warnings.is_empty());
    }
}
