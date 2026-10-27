//! `klima`: terminal client for Mitsubishi Electric air conditioners on classic MELCloud.

use clap::Parser;

#[derive(Debug, Parser)]
#[command(name = "klima", version, about, arg_required_else_help = true)]
struct Cli {}

fn main() {
    Cli::parse();
}
