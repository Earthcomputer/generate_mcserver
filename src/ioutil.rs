use crate::hashing::HashAlgorithm;
use crate::ContextExt;
use anyhow::{anyhow, bail, Context};
use reqwest::blocking::Client;
use reqwest::{IntoUrl, StatusCode};
use serde::de::DeserializeOwned;
use std::fmt::Display;
use std::fs::File;
use std::io::{Cursor, Read, Write};
use std::marker::PhantomData;
use std::path::Path;
use std::{fs, io};

pub fn link_or_copy(target: impl AsRef<Path>, link_name: impl AsRef<Path>) -> io::Result<()> {
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

pub fn copy_directory(src: impl AsRef<Path>, dst: impl AsRef<Path>) -> io::Result<()> {
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

pub fn is_not_found(err: &io::Error) -> bool {
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
    deserializer: impl GenericDeserializer<T>,
) -> anyhow::Result<T> {
    let mut file_name = file
        .file_name()
        .ok_or_else(|| anyhow!("failed to extract filename from {}", file.display()))?
        .to_owned();
    file_name.push(".etag");
    let etag_file = file.with_file_name(file_name);
    let etag = match fs::read(&etag_file) {
        Ok(etag) => Some(etag),
        Err(err) if is_not_found(&err) => None,
        Err(err) => return Err(err).with_path_context(&etag_file),
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
                    .with_path_context(file);
            }
            Err(err) if is_not_found(&err) => {}
            Err(err) => return Err(err).with_path_context(file),
        }
    } else if !response.status().is_success() {
        bail!(
            "request to {} returned status code {}",
            url,
            response.status()
        );
    }

    let etag = response.headers().get("ETag").cloned();

    let raw_json = response.bytes().with_context(|| url.to_string())?.to_vec();

    fs::write(&etag_file, "").with_path_context(&etag_file)?;
    fs::write(file, &raw_json).with_path_context(file)?;
    let result = deserializer
        .deserialize_slice(&raw_json)
        .with_path_context(file)?;

    if let Some(etag) = etag {
        fs::write(&etag_file, etag).with_path_context(&etag_file)?;
    }

    Ok(result)
}

pub fn download_large_with_hash<U>(
    client: &Client,
    url: U,
    path: &Path,
    algorithm: HashAlgorithm,
    expected_hash: &[u8],
    start_download: impl FnOnce(Option<u64>),
    progress_listener: impl FnMut(u64),
) -> anyhow::Result<()>
where
    U: IntoUrl,
{
    if let Ok(mut existing_file) = File::open(path) {
        let mut digest = algorithm.create_hasher();
        if io::copy(&mut existing_file, &mut digest).is_ok() && &*digest.finalize() == expected_hash
        {
            return Ok(());
        }
    }

    let url = url.into_url()?;
    download_large(client, url.clone(), path, start_download, progress_listener)?;

    let mut file = File::open(path).with_path_context(path)?;
    let mut digest = algorithm.create_hasher();
    io::copy(&mut file, &mut digest).with_path_context(path)?;
    if &*digest.finalize() != expected_hash {
        bail!(
            "file downloaded from {} did not match the expected hash",
            url
        );
    }

    Ok(())
}

pub fn download_large<U>(
    client: &Client,
    url: U,
    path: &Path,
    start_download: impl FnOnce(Option<u64>),
    mut progress_listener: impl FnMut(u64),
) -> anyhow::Result<()>
where
    U: IntoUrl,
{
    let url = url.into_url()?;

    let mut file = File::options()
        .create(true)
        .truncate(true)
        .write(true)
        .open(path)
        .with_path_context(path)?;
    let mut response = client
        .get(url.clone())
        .send()
        .with_context(|| url.clone())?;
    start_download(response.content_length());
    let mut downloaded = 0;
    let mut buffer = [0; 8192];
    loop {
        let n = response.read(&mut buffer).with_context(|| url.clone())?;
        if n == 0 {
            break;
        }
        file.write_all(&buffer[..n]).with_path_context(path)?;
        downloaded += n as u64;
        progress_listener(downloaded);
    }

    file.flush().with_path_context(path)?;

    Ok(())
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
    pub fn new() -> JsonDeserializer<T> {
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
