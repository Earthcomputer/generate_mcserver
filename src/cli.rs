use crate::mod_loader::ModLoader;
use anyhow::bail;
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

impl Cli {
    pub fn validate(&self) -> anyhow::Result<()> {
        self.command.validate()
    }
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Create a new instance
    New(NewCommand),
}

impl Command {
    fn validate(&self) -> anyhow::Result<()> {
        match self {
            Self::New(command) => command.validate(),
        }
    }
}

#[derive(Args, Debug)]
pub struct NewCommand {
    /// The name of the new instance
    pub name: String,
    /// The Minecraft version of the new instance [default: latest]
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
    /// The template directory to copy server configuration files from
    #[arg(short = 't', long, default_value_os_t = crate::get_cache_dir().join("default-config-template"))]
    pub config_template: PathBuf,
    /// Which mod loader to use for this server
    #[arg(short, long, default_value = "vanilla")]
    pub loader: ModLoader,
    /// The Fabric loader version to use (if using Fabric) [default: latest]
    #[arg(long)]
    pub fabric_loader_version: Option<String>,
    /// The Paper build to use (if using Paper) [default: latest]
    #[arg(long)]
    pub paper_build: Option<u32>,
}

impl NewCommand {
    fn validate(&self) -> anyhow::Result<()> {
        if self.fabric_loader_version.is_some() && self.loader != ModLoader::Fabric {
            bail!("Fabric loader version specified but the loader isn't Fabric");
        }

        if self.paper_build.is_some() && self.loader != ModLoader::Paper {
            bail!("Paper build specified but the loader isn't Paper");
        }

        Ok(())
    }
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
