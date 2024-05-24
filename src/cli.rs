use clap::{Args, Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    New(NewCommand),
}

#[derive(Args, Debug)]
pub struct NewCommand {
    pub name: String,
    #[arg(short, long)]
    pub version: Option<String>,
    #[arg(short = 'j', long)]
    pub skip_java_check: bool,
}
