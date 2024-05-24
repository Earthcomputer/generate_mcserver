use crate::cli::{Cli, Command, NewCommand};
use crate::java::find_java_candidates;
use crate::mojang::Manifest;
use anyhow::bail;
use clap::{crate_name, crate_version, Parser};
use reqwest::blocking::Client;
use std::io;
use std::path::PathBuf;

mod cli;
mod java;
mod mojang;

const CACHE_DIR: &str = ".cache";

fn main() {
    if let Err(err) = do_main() {
        let mut chain = err.chain();
        eprintln!("{} error: {}", crate_name!(), chain.next().unwrap());
        for err in chain {
            eprintln!("caused by: {err}");
        }
    }
}

fn do_main() -> anyhow::Result<()> {
    let java_candidates = find_java_candidates()?;
    println!("Java candidates: {java_candidates:#?}");
    Ok(())
    // let cli = Cli::parse();
    //
    // match cli.command {
    //     Command::New(command) => make_new_profile(command),
    // }
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

    let java_version = if command.skip_java_check {
        // "java".into()
    } else {
        eprintln!("checking java version");
        // find_java_executable(full_version.java_version.major_version)?
    };

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
