mod modrinth;

use crate::commands::add::AddModArgs;
use crate::instance::ModMetadata;
use clap::ValueEnum;
use serde::{Deserialize, Serialize};

#[derive(Debug, Copy, Clone, PartialEq, Eq, ValueEnum, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ModProvider {
    Hangar,
    Modrinth,
}

impl ModProvider {
    pub fn add_mod(&self, args: AddModArgs<'_>) -> anyhow::Result<ModMetadata> {
        match self {
            Self::Hangar => todo!(),
            Self::Modrinth => modrinth::add_mod(args),
        }
    }
}
