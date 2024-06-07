use crate::commands::new::ServerInstallArgs;
use crate::mod_loader::fabric::install_fabric;
use crate::mod_loader::paper::install_paper;
use crate::mod_loader::vanilla::install_vanilla;
use crate::mojang::{ManifestVersion, Version};
use clap::ValueEnum;
use std::fmt::{Display, Formatter};
use time::macros::datetime;

pub mod fabric;
pub mod paper;
pub mod vanilla;

#[derive(ValueEnum, Debug, Copy, Clone, PartialEq, Eq)]
pub enum ModLoader {
    Vanilla,
    Fabric,
    Paper,
}

impl ModLoader {
    pub fn minimum_java_version(
        &self,
        manifest_version: &ManifestVersion,
        full_version: &Version,
    ) -> u32 {
        match self {
            Self::Vanilla => full_version.java_version.major_version,
            Self::Fabric => full_version.java_version.major_version.max(8),
            Self::Paper => {
                // TODO: un-hardcode this when Paper's web API v3 comes out
                // TODO: these are the recommended versions, not the minimum versions
                if manifest_version.release_time < datetime!(2017-06-02 13:50:27 UTC) {
                    // <1.12
                    8
                } else if manifest_version.release_time < datetime!(2021-01-14 16:05:32 UTC) {
                    // >=1.12 <1.16.5
                    11
                } else if manifest_version.release_time < datetime!(2021-06-08 11:00:40 UTC) {
                    // >=1.16.5 <1.17
                    16
                } else {
                    // >=1.17
                    full_version.java_version.major_version.max(21)
                }
            }
        }
    }

    pub fn install(&self, args: ServerInstallArgs<'_>) -> anyhow::Result<()> {
        match self {
            Self::Vanilla => install_vanilla(args),
            Self::Fabric => install_fabric(args),
            Self::Paper => install_paper(args),
        }
    }
}

impl Display for ModLoader {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Vanilla => "vanilla",
            Self::Fabric => "fabric",
            Self::Paper => "paper",
        })
    }
}
