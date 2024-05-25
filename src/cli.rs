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
    New(NewCommand),
}

#[derive(Args, Debug)]
pub struct NewCommand {
    pub name: String,
    #[arg(short, long)]
    pub version: Option<String>,
    #[arg(short = 'j', long)]
    pub custom_java_exe: Option<PathBuf>,
    #[arg(long)]
    pub skip_java_check: bool,
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
