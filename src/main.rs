use crate::cli::{Cli, Command};
use crate::commands::new::make_new_profile;
use clap::{crate_name, crate_version, Parser};
use reqwest::blocking::Client;
use std::path::{Path, PathBuf};
use std::{fs, io};

mod cli;
mod commands;
mod java;
mod mojang;

const CACHE_DIR: &str = ".cache";

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
        #[cfg(debug_assertions)]
        eprintln!("{} error: {:#?}", crate_name!(), err);
        #[cfg(not(debug_assertions))]
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
    let cache_dir = PathBuf::from(CACHE_DIR);
    fs::create_dir_all(&cache_dir)?;

    let cli = Cli::parse();

    match cli.command {
        Command::New(command) => make_new_profile(command, cache_dir),
    }
}

fn make_client() -> anyhow::Result<Client> {
    Ok(Client::builder()
        .user_agent(concat!(crate_name!(), " ", crate_version!()))
        .build()?)
}

fn link_or_copy(target: impl AsRef<Path>, link_name: impl AsRef<Path>) -> io::Result<()> {
    let target = target.as_ref();
    let link_name = link_name.as_ref();

    #[cfg(windows)]
    let result = match std::os::windows::fs::symlink_file(target, link_name) {
        Err(err) if err.raw_os_error() == Some(1) || err.raw_os_error() == Some(1314) => {
            // ERROR_INVALID_FUNCTION returned when filesystem doesn't support symlinks
            // ERROR_PRIVILEGE_NOT_HELD returned when the program doesn't have permission to create symlinks
            fs::copy(target, link_name).map(|_| ())
        }
        result => result,
    };
    #[cfg(unix)]
    let result = match std::os::unix::fs::symlink(target, link_name) {
        Err(err) if err.raw_os_error() == Some(1) => {
            // EPERM returned when filesystem doesn't support symlinks,
            // in contrast to EACCES for when the user is missing read or write perms (Rust translates both to PermissionDenied)
            fs::copy(target, link_name).map(|_| ())
        }
        result => result,
    };
    #[cfg(not(any(windows, unix)))]
    let result = fs::copy(target, link_name).map(|_| ());

    result
}

fn copy_directory(src: impl AsRef<Path>, dst: impl AsRef<Path>) -> io::Result<()> {
    let src = src.as_ref();
    let dst = dst.as_ref();

    let entries = fs::read_dir(src)?;

    if !dst.exists() {
        fs::create_dir_all(dst)?;
    }

    for entry in entries {
        let entry = entry?;

        let src_entry_path = entry.path();
        let dst_entry_path = dst.join(entry.file_name());

        if src_entry_path.is_dir() {
            copy_directory(src_entry_path, dst_entry_path)?;
        } else {
            fs::copy(src_entry_path, dst_entry_path)?;
        }
    }

    Ok(())
}

fn is_not_found(err: &io::Error) -> bool {
    if err.kind() == io::ErrorKind::NotFound {
        return true;
    }

    // TODO: use ErrorKind::NotADirectory once it's stable

    #[cfg(unix)]
    let not_a_directory_error = Some(20);
    #[cfg(windows)]
    let not_a_directory_error = Some(267);
    #[cfg(not(any(unix, windows)))]
    let not_a_directory_error = None;

    let Some(not_a_directory_error) = not_a_directory_error else {
        return false;
    };

    let Some(raw_os_error) = err.raw_os_error() else {
        return false;
    };

    raw_os_error == not_a_directory_error
}
