use crate::cli::{Cli, Command};
use crate::commands::new::make_new_instance;
use anyhow::Context;
use clap::{crate_name, crate_version, Parser};
use reqwest::blocking::Client;
use reqwest::{IntoUrl, StatusCode};
use serde::de::DeserializeOwned;
use std::fmt::Display;
use std::fs::File;
use std::io::{Cursor, Read};
use std::marker::PhantomData;
use std::path::{Path, PathBuf};
use std::{env, fs, io};

mod cli;
mod commands;
mod java;
mod mod_loader;
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
        Command::New(command) => make_new_instance(command, cache_dir),
    }
}

fn make_client() -> anyhow::Result<Client> {
    Ok(Client::builder()
        .user_agent(concat!(crate_name!(), " ", crate_version!()))
        .build()?)
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

fn link_or_copy(target: impl AsRef<Path>, link_name: impl AsRef<Path>) -> io::Result<()> {
    let target = fs::canonicalize(target)?;
    let target = &target;
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

pub fn download_with_etag<T>(
    client: &Client,
    url: impl IntoUrl + Copy + Display,
    file: &Path,
    etag_file: &Path,
    deserializer: impl GenericDeserializer<T>,
) -> anyhow::Result<T> {
    let etag = match fs::read(etag_file) {
        Ok(etag) => Some(etag),
        Err(err) if is_not_found(&err) => None,
        Err(err) => return Err(err).with_context(|| etag_file.display().to_string()),
    };

    let mut request = client.get(url);
    if let Some(etag) = etag {
        request = request.header("If-None-Match", etag);
    }

    let response = request.send().with_context(|| url.to_string())?;

    if response.status() == StatusCode::NOT_MODIFIED {
        match File::open(file) {
            Ok(cached_file) => {
                return deserializer
                    .deserialize_reader(cached_file)
                    .with_context(|| file.display().to_string());
            }
            Err(err) if is_not_found(&err) => {}
            Err(err) => return Err(err).with_context(|| file.display().to_string()),
        }
    }

    let etag = response.headers().get("ETag").cloned();

    let raw_json = response.bytes().with_context(|| url.to_string())?.to_vec();

    fs::write(etag_file, "").with_context(|| etag_file.display().to_string())?;
    fs::write(file, &raw_json).with_context(|| file.display().to_string())?;
    let result = deserializer
        .deserialize_slice(&raw_json)
        .with_context(|| file.display().to_string())?;

    if let Some(etag) = etag {
        fs::write(etag_file, etag).with_context(|| etag_file.display().to_string())?;
    }

    Ok(result)
}

pub trait GenericDeserializer<T> {
    fn deserialize_slice(&self, data: &[u8]) -> anyhow::Result<T> {
        self.deserialize_reader(Cursor::new(data))
    }

    fn deserialize_reader<R>(&self, data: R) -> anyhow::Result<T>
    where
        R: Read;
}

pub struct IgnoreDeserializer;

impl GenericDeserializer<()> for IgnoreDeserializer {
    fn deserialize_reader<R>(&self, _data: R) -> anyhow::Result<()>
    where
        R: Read,
    {
        Ok(())
    }
}

pub struct JsonDeserializer<T> {
    _phantom: PhantomData<T>,
}

impl<T> JsonDeserializer<T> {
    fn new() -> JsonDeserializer<T> {
        JsonDeserializer {
            _phantom: PhantomData,
        }
    }
}

impl<T> GenericDeserializer<T> for JsonDeserializer<T>
where
    T: DeserializeOwned,
{
    fn deserialize_slice(&self, data: &[u8]) -> anyhow::Result<T> {
        Ok(serde_json::from_slice(data)?)
    }

    fn deserialize_reader<R>(&self, data: R) -> anyhow::Result<T>
    where
        R: Read,
    {
        Ok(serde_json::from_reader(data)?)
    }
}
