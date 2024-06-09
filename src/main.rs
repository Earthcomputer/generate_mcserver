use crate::cli::{Cli, Command};
use crate::commands::add::add_mod;
use crate::commands::new::make_new_instance;
use anyhow::Context;
use clap::{crate_name, crate_version, Parser};
use indicatif::{ProgressBar, ProgressStyle};
use reqwest::blocking::Client;
use std::borrow::Cow;
use std::path::{Path, PathBuf};
use std::{env, fs};

mod cli;
mod commands;
mod hashing;
mod instance;
mod ioutil;
mod java;
mod mod_loader;
mod mod_provider;
mod mojang;

const CACHE_DIR: &str = concat!(".", crate_name!(), "_cache");

#[cfg(target_os = "windows")]
const RUN_SERVER_FILENAME: &str = "run_server.bat";
#[cfg(not(target_os = "windows"))]
const RUN_SERVER_FILENAME: &str = "run_server";

#[cfg(windows)]
const LINE_ENDING: &str = "\r\n";
#[cfg(not(windows))]
const LINE_ENDING: &str = "\n";

fn main() {
    if let Err(err) = do_main() {
        #[cfg(feature = "dev")]
        eprintln!("{} error: {:#?}", crate_name!(), err);
        #[cfg(not(feature = "dev"))]
        {
            let mut chain = err.chain();
            eprintln!("{} error: {}", crate_name!(), chain.next().unwrap());
            for err in chain {
                eprintln!("caused by: {err}");
            }
        }
    }
}

fn do_main() -> anyhow::Result<()> {
    let cache_dir = get_cache_dir();
    fs::create_dir_all(&cache_dir)?;

    let cli = Cli::parse();
    cli.validate()?;

    match cli.command {
        Command::Add(command) => add_mod(command, cache_dir),
        Command::New(command) => make_new_instance(command, cache_dir),
    }
}

fn make_client() -> anyhow::Result<Client> {
    Ok(Client::builder()
        .user_agent(concat!(
            crate_name!(),
            " ",
            crate_version!(),
            " (",
            env!("GIT_URL"),
            ")"
        ))
        .build()?)
}

fn make_progress_bar(len: u64, message: impl Into<Cow<'static, str>>) -> ProgressBar {
    let pb = ProgressBar::new(len).with_message(message);
    pb.set_style(
        ProgressStyle::default_bar()
            .template("{msg}\n{bar:40.cyan/blue} {bytes}/{total_bytes} ({eta})")
            .unwrap()
            .progress_chars("##-"),
    );
    pb
}

#[cfg(feature = "dev")]
fn get_cache_dir() -> PathBuf {
    PathBuf::from(CACHE_DIR)
}

#[cfg(all(not(feature = "dev"), target_os = "windows"))]
fn get_cache_dir() -> PathBuf {
    env::var_os("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| home::home_dir().unwrap_or_default())
        .join(CACHE_DIR)
}

#[cfg(all(not(feature = "dev"), not(target_os = "windows")))]
fn get_cache_dir() -> PathBuf {
    home::home_dir().unwrap_or_default().join(CACHE_DIR)
}

pub trait ContextExt<T> {
    fn with_path_context(self, path: &Path) -> anyhow::Result<T>;
}

impl<T, E> ContextExt<T> for Result<T, E>
where
    Result<T, E>: Context<T, E>,
{
    fn with_path_context(self, path: &Path) -> anyhow::Result<T> {
        self.with_context(|| path.display().to_string())
    }
}
