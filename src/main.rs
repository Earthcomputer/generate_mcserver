use crate::cli::{Cli, Command, NewCommand};
use crate::java::{create_java_candidate_for_path, find_java_candidates};
use crate::mojang::Manifest;
use anyhow::bail;
use clap::{crate_name, crate_version, Parser};
use reqwest::blocking::Client;
use std::cmp::Ordering;
use std::io;
use std::path::PathBuf;

mod cli;
mod java;
mod mojang;

const CACHE_DIR: &str = ".cache";

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
    let cli = Cli::parse();

    match cli.command {
        Command::New(command) => make_new_profile(command),
    }
}

fn make_new_profile(command: NewCommand) -> anyhow::Result<()> {
    let profile_path = PathBuf::from(command.name);
    if let Err(err) = std::fs::create_dir(&profile_path) {
        if err.kind() == io::ErrorKind::AlreadyExists {
            bail!("a profile with that name already exists");
        }
        return Err(err.into());
    }

    let cache_dir = PathBuf::from(CACHE_DIR);
    std::fs::create_dir_all(&cache_dir)?;

    let client = make_client()?;

    eprintln!("fetching minecraft versions");
    let manifest = Manifest::download(&client)?;

    let version = command.version.unwrap_or(manifest.latest.release);
    let Some(manifest_version) = manifest.versions.into_iter().find(|ver| ver.id == version) else {
        bail!("no such version: {version}");
    };

    eprintln!("fetching metadata for version {version}");
    let full_version =
        manifest_version.download(&client, &cache_dir.join(format!("{version}.json")))?;

    let java_candidate = if let Some(java_exe) = command.custom_java_exe {
        let java_candidate = create_java_candidate_for_path(java_exe, &mut None)?;
        if !command.skip_java_check
            && java_candidate.version.major < full_version.java_version.major_version
        {
            bail!(
                "specified java is not compatible with {version}, need at least java {}",
                full_version.java_version.major_version
            );
        }
        java_candidate
    } else {
        eprintln!("searching for java versions");
        let mut java_candidates = find_java_candidates()?;
        if !command.skip_java_check {
            java_candidates.retain(|candidate| {
                candidate.version.major >= full_version.java_version.major_version
            });
        }

        // sort by major version ascending (to most closely match the required java version), and then by version descending, to prioritize the latest of each major version.
        // also put the versions that are too old at the end
        java_candidates.sort_by(|candidate1, candidate2| {
            let candidate1_old = candidate1.version.major < full_version.java_version.major_version;
            let candidate2_old = candidate2.version.major < full_version.java_version.major_version;
            let cmp = candidate1_old.cmp(&candidate2_old);
            if cmp != Ordering::Equal {
                return cmp;
            }

            let cmp = candidate1.version.major.cmp(&candidate2.version.major);
            if cmp != Ordering::Equal {
                return cmp;
            }

            candidate2.version.cmp(&candidate1.version)
        });
        let Some(java_candidate) =
            cli::select_from_list(java_candidates, "select java executable")?
        else {
            bail!(
                "could not find any java install compatible with {version}, need at least java {}",
                full_version.java_version.major_version
            );
        };
        java_candidate
    };
    if !command.skip_java_check
        && java_candidate.version.major > full_version.java_version.major_version
    {
        eprintln!("warning: selected java version {} is newer than the recommended java version {}, which may cause issues", java_candidate.version, full_version.java_version.major_version);
    }

    eprintln!("downloading server jar");
    let Some(server_download) = full_version.downloads.server else {
        bail!("version {version} does not have a server download");
    };
    server_download.download(&client, &profile_path.join("server.jar"))?;

    Ok(())
}

fn make_client() -> anyhow::Result<Client> {
    Ok(Client::builder()
        .user_agent(concat!(crate_name!(), " ", crate_version!()))
        .build()?)
}
