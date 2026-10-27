//! `klima`: terminal client for Mitsubishi Electric air conditioners on classic MELCloud.

mod app;
mod cache;
mod chart;
mod commands;
mod config;
mod control;
mod error;
mod files;
mod governor;
mod output;
mod paths;
mod session;
mod theme;
mod tui;

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{ArgGroup, CommandFactory, Parser, Subcommand};
use clap_complete::Shell;
use jiff::Span;
use melcloud::{FanSpeed, OperationMode, VaneHorizontal, VaneVertical};

use crate::app::App;
use crate::control::{Options, Power, Temperature, Wanted};
use crate::error::AppError;

#[derive(Debug, Parser)]
#[command(
    name = "klima",
    version,
    about,
    after_help = "Without a command, klima opens the interactive TUI."
)]
struct Cli {
    /// Device name or ID; needed when the account has more than one.
    #[arg(short, long, global = true)]
    device: Option<String>,
    /// Print JSON instead of text.
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Log in to MELCloud and keep the session. The password is never stored.
    Login {
        /// Account email; asked for when missing.
        #[arg(long)]
        email: Option<String>,
        /// Read the password from the first line of standard input.
        #[arg(long)]
        password_stdin: bool,
    },
    /// Forget the stored session.
    Logout,
    /// List the devices on the account.
    Devices {
        /// Ask MELCloud instead of using the list cached today.
        #[arg(long)]
        fresh: bool,
    },
    /// Show the state of the unit.
    Status {
        /// Ask MELCloud even if the cached state is recent.
        #[arg(long)]
        fresh: bool,
    },
    /// Show the room temperature and the setting changes klima recorded, without asking MELCloud.
    ///
    /// Every read of the unit, by a command or the TUI, is recorded; the history keeps 90 days.
    History {
        /// How far back, at most 90 days: 30m, 24h, 7d.
        #[arg(long, default_value = "24h", value_parser = past_span, value_name = "DURATION")]
        since: Span,
    },
    /// Switch the unit on.
    On {
        #[command(flatten)]
        options: Options,
    },
    /// Switch the unit off.
    Off {
        #[command(flatten)]
        options: Options,
    },
    /// Switch the unit on when it is off, and off when it is on.
    Toggle {
        #[command(flatten)]
        options: Options,
    },
    /// Set the target temperature in °C: 22.5, or relative to the current one: +1, -0.5.
    Temp {
        /// Target in °C, or a change of it starting with + or -.
        #[arg(allow_negative_numbers = true, value_name = "TEMP")]
        temperature: Temperature,
        #[command(flatten)]
        options: Options,
    },
    /// Set the mode.
    Mode {
        #[arg(value_name = "heat|dry|cool|fan|auto")]
        mode: OperationMode,
        #[command(flatten)]
        options: Options,
    },
    /// Set the fan speed.
    Fan {
        #[arg(value_name = "auto|LEVEL")]
        speed: FanSpeed,
        #[command(flatten)]
        options: Options,
    },
    /// Point the vanes.
    #[command(group(ArgGroup::new("direction").required(true).multiple(true)))]
    Vane {
        /// Up and down.
        #[arg(long, group = "direction", value_name = "auto|1-5|swing")]
        vertical: Option<VaneVertical>,
        /// Left and right; split needs a unit with a wide vane.
        #[arg(long, group = "direction", value_name = "auto|1-5|split|swing")]
        horizontal: Option<VaneHorizontal>,
        #[command(flatten)]
        options: Options,
    },
    /// Change several settings in one command.
    #[command(group(ArgGroup::new("settings").required(true).multiple(true)))]
    Set {
        #[arg(long, group = "settings", value_enum)]
        power: Option<Power>,
        #[arg(long, group = "settings", value_name = "heat|dry|cool|fan|auto")]
        mode: Option<OperationMode>,
        /// Target in °C, or relative with + or -.
        #[arg(long, group = "settings", allow_negative_numbers = true)]
        temp: Option<Temperature>,
        #[arg(long, group = "settings", value_name = "auto|LEVEL")]
        fan: Option<FanSpeed>,
        #[arg(long, group = "settings", value_name = "auto|1-5|swing")]
        vertical: Option<VaneVertical>,
        #[arg(long, group = "settings", value_name = "auto|1-5|split|swing")]
        horizontal: Option<VaneHorizontal>,
        #[command(flatten)]
        options: Options,
    },
    /// Lift the local pause after MELCloud throttled the account (HTTP 429).
    Unblock,
    /// Print a shell completion script.
    ///
    /// For example: klima completions zsh > ~/.zfunc/_klima
    Completions { shell: Shell },
    /// Write the man pages for klima and each of its commands into a directory.
    ///
    /// For example: klima man ~/.local/share/man/man1
    Man {
        /// Directory for the pages; created when missing.
        dir: PathBuf,
    },
}

/// A positive span of time for `--since`.
fn past_span(text: &str) -> Result<Span, String> {
    match text.parse::<Span>() {
        Ok(span) if span.is_positive() => Ok(span),
        _ => Err("expected a duration such as 24h, 90m or 7d".into()),
    }
}

/// The settings a command asks for, with the options it was given.
fn wanted(command: &Command) -> Option<(Wanted, Options)> {
    let power = |power| Wanted {
        power: Some(power),
        ..Wanted::default()
    };
    Some(match *command {
        Command::On { options } => (power(Power::On), options),
        Command::Off { options } => (power(Power::Off), options),
        Command::Toggle { options } => (power(Power::Toggle), options),
        Command::Temp {
            temperature,
            options,
        } => (
            Wanted {
                temperature: Some(temperature),
                ..Wanted::default()
            },
            options,
        ),
        Command::Mode { mode, options } => (
            Wanted {
                mode: Some(mode),
                ..Wanted::default()
            },
            options,
        ),
        Command::Fan { speed, options } => (
            Wanted {
                fan: Some(speed),
                ..Wanted::default()
            },
            options,
        ),
        Command::Vane {
            vertical,
            horizontal,
            options,
        } => (
            Wanted {
                vane_vertical: vertical,
                vane_horizontal: horizontal,
                ..Wanted::default()
            },
            options,
        ),
        Command::Set {
            power,
            mode,
            temp,
            fan,
            vertical,
            horizontal,
            options,
        } => (
            Wanted {
                power,
                mode,
                temperature: temp,
                fan,
                vane_vertical: vertical,
                vane_horizontal: horizontal,
            },
            options,
        ),
        Command::Login { .. }
        | Command::Logout
        | Command::Devices { .. }
        | Command::Status { .. }
        | Command::History { .. }
        | Command::Unblock
        | Command::Completions { .. }
        | Command::Man { .. } => return None,
    })
}

async fn run(cli: Cli) -> Result<(), AppError> {
    // These need no configuration, so they work before anything is set up.
    match cli.command {
        Some(Command::Completions { shell }) => {
            let mut script = Vec::new();
            clap_complete::generate(shell, &mut Cli::command(), "klima", &mut script);
            return output::raw(&script);
        }
        Some(Command::Man { dir }) => {
            if dir.exists() && !dir.is_dir() {
                return Err(AppError::Usage(format!(
                    "{} is not a directory",
                    dir.display()
                )));
            }
            std::fs::create_dir_all(&dir)
                .and_then(|()| clap_mangen::generate_to(Cli::command(), &dir))
                .map_err(|e| AppError::io(&dir, e))?;
            output::success(format_args!(
                "man pages written to {0}; read one with `man -l {0}/klima.1`",
                dir.display()
            ));
            return Ok(());
        }
        _ => {}
    }
    let app = App::new()?;
    let Some(command) = cli.command else {
        return tui::run(app).await;
    };
    if let Some((wanted, options)) = wanted(&command) {
        return control::run(&app, cli.device.as_deref(), wanted, options, cli.json).await;
    }
    match command {
        Command::Login {
            email,
            password_stdin,
        } => commands::login(&app, email, password_stdin).await,
        Command::Logout => commands::logout(&app),
        Command::Devices { fresh } => commands::devices(&app, fresh, cli.json).await,
        Command::Status { fresh } => {
            commands::status(&app, cli.device.as_deref(), fresh, cli.json).await
        }
        Command::History { since } => {
            commands::history(&app, cli.device.as_deref(), since, cli.json).await
        }
        Command::Unblock => commands::unblock(&app),
        _ => unreachable!("commands that change the unit are handled above"),
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("a single-threaded runtime always starts");
    match runtime.block_on(run(cli)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            output::error(&error);
            error.exit_code()
        }
    }
}
