use anyhow::bail;
use reqwest::blocking::Client;
use serde::de::{Error, Unexpected};
use serde::{Deserialize, Deserializer};
use sha1::digest::Output;
use sha1::{Digest, Sha1};
use std::fs::File;
use std::io::{Seek, SeekFrom, Write};
use std::path::Path;
use std::{fs, io};
use time::OffsetDateTime;
use url::Url;

const MANIFEST_URL: &str = "https://piston-meta.mojang.com/mc/game/version_manifest_v2.json";

#[derive(Debug, Deserialize)]
pub struct Manifest {
    pub latest: LatestVersions,
    pub versions: Vec<ManifestVersion>,
}

impl Manifest {
    pub fn download(client: &Client) -> anyhow::Result<Manifest> {
        Ok(client.get(MANIFEST_URL).send()?.json()?)
    }
}

#[derive(Debug, Deserialize)]
pub struct LatestVersions {
    pub release: String,
    pub snapshot: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManifestVersion {
    pub id: String,
    #[serde(rename = "type")]
    pub typ: VersionType,
    url: Url,
    #[serde(with = "time::serde::iso8601")]
    pub release_time: OffsetDateTime,
    sha1: Sha1String,
}

impl ManifestVersion {
    pub fn download(&self, client: &Client, file: &Path) -> anyhow::Result<Version> {
        if let Ok(file_contents) = fs::read(file) {
            if Sha1::digest(&file_contents) == self.sha1.0 {
                return Ok(serde_json::from_slice(&file_contents)?);
            }
        }

        let file_contents = client.get(self.url.clone()).send()?.bytes()?.to_vec();
        if Sha1::digest(&file_contents) != self.sha1.0 {
            bail!(
                "file downloaded from {} did not match the expected sha1 hash",
                self.url
            );
        }

        Ok(serde_json::from_slice(&file_contents)?)
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VersionType {
    Release,
    Snapshot,
    OldBeta,
    OldAlpha,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Version {
    pub downloads: VersionDownloads,
    pub java_version: JavaVersion,
}

#[derive(Debug, Deserialize)]
pub struct VersionDownloads {
    pub server: Option<VersionDownload>,
}

#[derive(Debug, Deserialize)]
pub struct VersionDownload {
    sha1: Sha1String,
    url: Url,
}

impl VersionDownload {
    pub fn download(&self, client: &Client, file: &Path) -> anyhow::Result<()> {
        if let Ok(mut existing_file) = File::open(file) {
            let mut digest = Sha1::default();
            if io::copy(&mut existing_file, &mut digest).is_ok() && digest.finalize() == self.sha1.0
            {
                return Ok(());
            }
        }

        let mut file = &mut File::options().read(true).write(true).open(file)?;
        client.get(self.url.clone()).send()?.copy_to(&mut file)?;
        file.flush()?;

        file.seek(SeekFrom::Start(0))?;
        let mut digest = Sha1::default();
        io::copy(&mut file, &mut digest)?;
        if digest.finalize() != self.sha1.0 {
            bail!(
                "file downloaded from {} did not match the expected sha1 hash",
                self.url
            );
        }

        Ok(())
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JavaVersion {
    pub major_version: u32,
}

#[derive(Debug)]
struct Sha1String(Output<Sha1>);

impl<'de> Deserialize<'de> for Sha1String {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let str: &str = Deserialize::deserialize(deserializer)?;
        if str.len() != 40 {
            return Err(Error::invalid_length(
                str.len(),
                &"sha1 string of length 40",
            ));
        }

        let mut result = Output::<Sha1>::default();

        fn digit_value<'de, D>(char: u8) -> Result<u8, D::Error>
        where
            D: Deserializer<'de>,
        {
            match char {
                b'0'..=b'9' => Ok(char - b'0'),
                b'A'..=b'F' => Ok(char - b'A' + 10),
                b'a'..=b'f' => Ok(char - b'a' + 10),
                _ => Err(Error::invalid_value(
                    Unexpected::Char(char as char),
                    &"sha1 string",
                )),
            }
        }

        for (i, chunk) in str.as_bytes().chunks_exact(2).enumerate() {
            result[i] = 16 * digit_value::<D>(chunk[0])? + digit_value::<D>(chunk[1])?;
        }

        Ok(Sha1String(result))
    }
}
