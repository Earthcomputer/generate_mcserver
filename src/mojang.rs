use crate::hashing::{HashAlgorithm, Sha1String};
use crate::ioutil::JsonDeserializer;
use crate::{ioutil, ContextExt};
use anyhow::{bail, Context};
use reqwest::blocking::Client;
use serde::Deserialize;
use sha1::{Digest, Sha1};
use std::fs;
use std::path::Path;
use time::OffsetDateTime;
use url::Url;

const MANIFEST_URL: &str = "https://piston-meta.mojang.com/mc/game/version_manifest_v2.json";

#[derive(Debug, Deserialize)]
pub struct Manifest {
    pub latest: LatestVersions,
    pub versions: Vec<ManifestVersion>,
}

impl Manifest {
    pub fn download(client: &Client, file: &Path) -> anyhow::Result<Manifest> {
        ioutil::download_with_etag(client, MANIFEST_URL, file, JsonDeserializer::new())
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
            if *Sha1::digest(&file_contents) == self.sha1.inner {
                return serde_json::from_slice(&file_contents).with_path_context(file);
            }
        }

        let response = client
            .get(self.url.clone())
            .send()
            .with_context(|| self.url.clone())?;
        if !response.status().is_success() {
            bail!(
                "request to {} returned status code {}",
                self.url,
                response.status()
            );
        }
        let file_contents = response.bytes().with_context(|| self.url.clone())?.to_vec();
        if *Sha1::digest(&file_contents) != self.sha1.inner {
            bail!(
                "file downloaded from {} did not match the expected hash",
                self.url
            );
        }

        fs::write(file, &file_contents)?;

        serde_json::from_slice(&file_contents).with_path_context(file)
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VersionType {
    Release,
    Snapshot,
    OldBeta,
    OldAlpha,
    #[serde(other)]
    Other,
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
    pub size: u64,
    url: Url,
}

impl VersionDownload {
    pub fn download(
        &self,
        client: &Client,
        path: &Path,
        progress_listener: impl FnMut(u64),
    ) -> anyhow::Result<()> {
        ioutil::download_large_with_hash(
            client,
            self.url.clone(),
            path,
            HashAlgorithm::Sha1,
            &self.sha1.inner,
            |_| {},
            progress_listener,
        )
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JavaVersion {
    pub major_version: u32,
}
