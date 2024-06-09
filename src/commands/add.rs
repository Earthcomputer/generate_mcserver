use crate::cli::AddCommand;
use crate::instance::InstanceMetadata;
use crate::make_client;
use anyhow::bail;
use reqwest::blocking::Client;
use std::path::{Path, PathBuf};

pub fn add_mod(command: AddCommand, cache_dir: PathBuf) -> anyhow::Result<()> {
    let instance_path = Path::new(".");
    let mut instance_metadata = InstanceMetadata::load(instance_path)?;

    let Some(provider) = command
        .provider
        .or_else(|| instance_metadata.loader.default_mod_provider())
    else {
        bail!(
            "cannot install mods on loader '{}'",
            instance_metadata.loader
        );
    };

    let added_mod = provider.add_mod(AddModArgs {
        command: &command,
        client: &make_client()?,
        cache_dir: &cache_dir,
        instance_path,
        instance_metadata: &instance_metadata,
    })?;

    instance_metadata.mods.retain(|m| m.id != added_mod.id);
    instance_metadata.mods.push(added_mod);
    instance_metadata.save(instance_path)?;

    Ok(())
}

pub struct AddModArgs<'a> {
    pub command: &'a AddCommand,
    pub client: &'a Client,
    pub cache_dir: &'a Path,
    pub instance_path: &'a Path,
    pub instance_metadata: &'a InstanceMetadata,
}
