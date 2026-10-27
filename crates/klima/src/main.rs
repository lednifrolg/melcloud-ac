//! `klima`: terminal client for Mitsubishi Electric air conditioners on classic MELCloud.

mod app;
mod cache;
mod commands;
mod config;
mod error;
mod files;
mod governor;
mod output;
mod paths;
mod session;

use std::process::ExitCode;

use clap::{Parser, Subcommand};

use crate::app::App;
use crate::error::AppError;

#[derive(Debug, Parser)]
#[command(name = "klima", version, about, arg_required_else_help = true)]
struct Cli {
    /// Device name or ID; needed when the account has more than one.
    #[arg(short, long, global = true)]
    device: Option<String>,
    /// Print JSON instead of text.
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    command: Command,
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
    /// Lift the local pause after MELCloud throttled the account (HTTP 429).
    Unblock,
}

async fn run(cli: Cli) -> Result<(), AppError> {
    let app = App::new()?;
    match cli.command {
        Command::Login {
            email,
            password_stdin,
        } => commands::login(&app, email, password_stdin).await,
        Command::Logout => commands::logout(&app),
        Command::Devices { fresh } => commands::devices(&app, fresh, cli.json).await,
        Command::Status { fresh } => {
            commands::status(&app, cli.device.as_deref(), fresh, cli.json).await
        }
        Command::Unblock => commands::unblock(&app),
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
