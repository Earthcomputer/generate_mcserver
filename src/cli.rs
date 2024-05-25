use clap::{Args, Parser, Subcommand};
use std::fmt::Display;
use std::io;
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Create a new instance
    New(NewCommand),
}

#[derive(Args, Debug)]
pub struct NewCommand {
    /// The name of the new instance
    pub name: String,
    /// The Minecraft version of the new instance (defaults to latest stable)
    #[arg(short, long)]
    pub version: Option<String>,
    /// An explicit path to the Java executable to use
    #[arg(short = 'j', long)]
    pub custom_java_exe: Option<PathBuf>,
    /// Skip Java compatibility checks
    #[arg(long)]
    pub skip_java_check: bool,
    /// Agree to the EULA. By adding this argument you agree to the Minecraft EULA as specified at https://aka.ms/MinecraftEULA.
    #[arg(short, long)]
    pub eula: bool,
}

pub fn select_from_list<T: Display>(mut list: Vec<T>, prompt: &str) -> io::Result<Option<T>> {
    match list.len() {
        0 => Ok(None),
        1 => Ok(Some(list.remove(0))),
        _ => loop {
            eprintln!("{}:", prompt);

            for (index, element) in list.iter().enumerate() {
                if index == 0 {
                    eprintln!("1. {element} (default)");
                } else {
                    eprintln!("{}. {element}", index + 1);
                }
            }

            let mut input = String::new();
            io::stdin().read_line(&mut input)?;
            let input = input.trim();

            if input.is_empty() {
                return Ok(list.into_iter().next());
            }

            if let Ok(input) = input.parse::<usize>() {
                if (1..=list.len()).contains(&input) {
                    return Ok(list.into_iter().nth(input - 1));
                }
            }

            eprintln!("invalid input");
        },
    }
}
