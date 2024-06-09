use crate::hashing::HashWithAlgorithm;
use crate::mod_loader::ModLoader;
use crate::mod_provider::ModProvider;
use crate::ContextExt;
use clap::crate_name;
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::path::Path;

const INSTANCE_METADATA_FILE: &str = concat!(".", crate_name!(), "_metadata.json");

#[derive(Debug, Deserialize, Serialize)]
pub struct InstanceMetadata {
    pub loader: ModLoader,
    pub minecraft_version: String,
    #[serde(default)]
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub mods: Vec<ModMetadata>,
}

impl InstanceMetadata {
    pub fn new(loader: ModLoader, minecraft_version: impl Into<String>) -> Self {
        Self {
            loader,
            minecraft_version: minecraft_version.into(),
            mods: Vec::new(),
        }
    }

    pub fn load(instance_dir: &Path) -> anyhow::Result<InstanceMetadata> {
        let metadata_file = instance_dir.join(INSTANCE_METADATA_FILE);
        let file = File::open(&metadata_file).with_path_context(&metadata_file)?;
        serde_json::from_reader(file).with_path_context(&metadata_file)
    }

    pub fn save(&self, instance_dir: &Path) -> anyhow::Result<()> {
        let metadata_file = instance_dir.join(INSTANCE_METADATA_FILE);
        let file = File::options()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&metadata_file)
            .with_path_context(&metadata_file)?;
        serde_json::to_writer_pretty(file, self).with_path_context(&metadata_file)?;
        Ok(())
    }
}

#[derive(Debug, Deserialize, Serialize)]
pub struct ModMetadata {
    pub id: String,
    pub name: String,
    pub file_name: String,
    pub hash: HashWithAlgorithm,
    pub provider: ModProvider,
}
