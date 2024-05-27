use crate::commands::new::{write_run_server_file, ServerInstallArgs};
use crate::mod_loader::vanilla::{agree_to_eula, download_vanilla_server};
use crate::{download_with_etag, link_or_copy, IgnoreDeserializer, JsonDeserializer};
use anyhow::{anyhow, Context};
use serde::Deserialize;
use std::fs;

const INSTALLER_VERSIONS_URL: &str = "https://meta.fabricmc.net/v2/versions/installer";

pub fn install_fabric(args: ServerInstallArgs<'_>) -> anyhow::Result<()> {
    let fabric_cache_dir = args.cache_dir.join("fabric");
    fs::create_dir_all(&fabric_cache_dir)
        .with_context(|| fabric_cache_dir.display().to_string())?;

    eprintln!("fetching fabric installer versions");
    let installer_versions: Vec<FabricVersion> = download_with_etag(
        args.client,
        INSTALLER_VERSIONS_URL,
        &fabric_cache_dir.join("installer_versions.json"),
        &fabric_cache_dir.join("installer_versions.json.etag"),
        JsonDeserializer::new(),
    )?;
    let installer_version = first_stable(installer_versions, "installer")?;
    let loader_version = match args.command.fabric_loader_version.as_ref() {
        Some(loader_version) => loader_version.clone(),
        None => {
            eprintln!("fetching fabric loader versions");
            let loader_versions: Vec<LoaderEntry> = download_with_etag(
                args.client,
                &format!(
                    "https://meta.fabricmc.net/v2/versions/loader/{}",
                    urlencoding::encode(args.version_name)
                ),
                &fabric_cache_dir.join(format!("loader_versions_{}.json", args.version_name)),
                &fabric_cache_dir.join(format!("loader_versions_{}.json.etag", args.version_name)),
                JsonDeserializer::new(),
            )?;
            let loader_versions = loader_versions.into_iter().map(|v| v.loader).collect();
            first_stable(loader_versions, "loader")?
        }
    };

    eprintln!("downloading fabric server launcher");
    let fabric_server_launch_path = fabric_cache_dir.join(format!(
        "fabric-server-launch-{}-{}-{}.jar",
        args.version_name, loader_version, installer_version
    ));
    let fabric_server_launch_etag_path = fabric_cache_dir.join(format!(
        "fabric-server-launch-{}-{}-{}.jar.etag",
        args.version_name, loader_version, installer_version
    ));
    download_with_etag(
        args.client,
        &format!(
            "https://meta.fabricmc.net/v2/versions/loader/{}/{}/{}/server/jar",
            urlencoding::encode(args.version_name),
            loader_version,
            installer_version
        ),
        &fabric_server_launch_path,
        &fabric_server_launch_etag_path,
        IgnoreDeserializer,
    )?;

    let server_jar_path = download_vanilla_server(&args)?;

    fs::create_dir(args.instance_path).with_context(|| args.instance_path.display().to_string())?;

    let server_jar_link_path = args.instance_path.join("server.jar");
    link_or_copy(&server_jar_path, &server_jar_link_path).with_context(|| {
        format!(
            "linking {} to {}",
            server_jar_link_path.display(),
            server_jar_path.display()
        )
    })?;

    let fabric_server_launch_link_path = args.instance_path.join("fabric-server-launch.jar");
    link_or_copy(&fabric_server_launch_path, &fabric_server_launch_link_path).with_context(
        || {
            format!(
                "linking {} to {}",
                fabric_server_launch_link_path.display(),
                fabric_server_launch_path.display()
            )
        },
    )?;

    let server_launch_command = format!(
        "{} -Dfabric.installer.server.gameJar=server.jar -jar fabric-server-launch.jar nogui",
        args.escaped_java_exe_name()?
    );
    write_run_server_file(&args, &server_launch_command)?;

    agree_to_eula(&args)?;

    Ok(())
}

#[derive(Debug, Deserialize)]
struct FabricVersion {
    version: String,
    stable: bool,
}

#[derive(Debug, Deserialize)]
struct LoaderEntry {
    loader: FabricVersion,
}

fn first_stable(versions: Vec<FabricVersion>, what: &str) -> anyhow::Result<String> {
    let mut result = None;
    for version in versions {
        if version.stable {
            return Ok(version.version);
        }
        if result.is_none() {
            result = Some(version.version);
        }
    }
    result.ok_or(anyhow!(
        "could not find any {what} version for this Minecraft version"
    ))
}
