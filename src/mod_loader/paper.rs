use crate::commands::new::{write_run_server_file, ServerInstallArgs};
use crate::hashing::{HashAlgorithm, Sha2String};
use crate::ioutil::JsonDeserializer;
use crate::mod_loader::vanilla::{agree_to_eula, download_vanilla_server};
use crate::{ioutil, make_progress_bar, ContextExt};
use anyhow::{anyhow, bail, Context};
use serde::Deserialize;
use std::cell::RefCell;
use std::fs;
use std::fs::File;
use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use zip::result::ZipError;
use zip::ZipArchive;

pub fn install_paper(args: ServerInstallArgs<'_>) -> anyhow::Result<()> {
    let paper_cache_dir = args.cache_dir.join("paper");
    fs::create_dir_all(&paper_cache_dir).with_path_context(&paper_cache_dir)?;

    let paper_build = match args.command.paper_build {
        Some(paper_build) => paper_build,
        None => {
            eprintln!("fetching paper builds");
            let builds: PaperBuilds = ioutil::download_with_etag(
                args.client,
                &format!(
                    "https://api.papermc.io/v2/projects/paper/versions/{}",
                    urlencoding::encode(args.version_name)
                ),
                &paper_cache_dir.join(format!("version-info-{}.json", args.version_name)),
                JsonDeserializer::new(),
            )?;
            builds
                .builds
                .iter()
                .copied()
                .max()
                .ok_or_else(|| anyhow!("no paper builds for this minecraft version"))?
        }
    };

    eprintln!("fetching paper build metadata");
    let build_metadata: PaperBuildMetadata = ioutil::download_with_etag(
        args.client,
        &format!(
            "https://api.papermc.io/v2/projects/paper/versions/{}/builds/{}",
            args.version_name, paper_build
        ),
        &paper_cache_dir.join(format!(
            "build-metadata-{}-{}.json",
            args.version_name, paper_build
        )),
        JsonDeserializer::new(),
    )?;

    let pb = RefCell::new(None);
    let paperclip_path = paper_cache_dir.join(format!(
        "paperclip-{}-{}.jar",
        args.version_name, paper_build
    ));
    ioutil::download_large_with_hash(
        args.client,
        format!(
            "https://api.papermc.io/v2/projects/paper/versions/{}/builds/{}/downloads/{}",
            args.version_name, paper_build, build_metadata.downloads.application.name
        ),
        &paperclip_path,
        HashAlgorithm::Sha256,
        &build_metadata.downloads.application.sha256.inner,
        |download_size| {
            if let Some(download_size) = download_size {
                *pb.borrow_mut() = Some(make_progress_bar(download_size, "downloading paperclip"));
            } else {
                eprintln!("downloading paperclip");
            }
        },
        |progress| {
            if let Some(pb) = &*pb.borrow() {
                pb.set_position(progress);
            }
        },
    )?;
    if let Some(pb) = pb.into_inner() {
        pb.finish_with_message("downloaded paperclip");
    }

    let server_jar_path = download_vanilla_server(&args)?;

    let mojang_jar_name = find_mojang_jar_name(&paperclip_path)?
        .unwrap_or_else(|| format!("mojang_{}.jar", args.version_name));

    fs::create_dir_all(args.instance_path).with_path_context(args.instance_path)?;

    let paperclip_link_path = args.instance_path.join("paperclip.jar");
    ioutil::link_or_copy(&paperclip_path, &paperclip_link_path).with_context(|| {
        format!(
            "linking {} to {}",
            paperclip_link_path.display(),
            paperclip_path.display()
        )
    })?;

    let paperclip_cache_dir = args.instance_path.join("cache");
    fs::create_dir(&paperclip_cache_dir).with_path_context(&paperclip_cache_dir)?;
    let mojang_jar_path = paperclip_cache_dir.join(mojang_jar_name);
    ioutil::link_or_copy(&server_jar_path, &mojang_jar_path).with_context(|| {
        format!(
            "linking {} to {}",
            mojang_jar_path.display(),
            server_jar_path.display()
        )
    })?;

    eprintln!("running paperclip");
    let output = Command::new(&args.java_candidate.path)
        .arg("-Dpaperclip.patchonly=true")
        .arg("-jar")
        .arg("paperclip.jar")
        .current_dir(args.instance_path)
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .output()?;
    if !output.status.success() {
        bail!("paperclip exited with code {}", output.status)
    }

    write_run_server_file(
        &args,
        &format!("{} -jar paperclip.jar", args.escaped_java_exe_name()?),
    )?;

    agree_to_eula(&args)?;

    Ok(())
}

fn find_mojang_jar_name(paperclip_jar: &Path) -> anyhow::Result<Option<String>> {
    let file = File::open(paperclip_jar).with_path_context(paperclip_jar)?;
    let mut archive = ZipArchive::new(file).with_path_context(paperclip_jar)?;
    let result = match archive.by_name("META-INF/download-context") {
        Ok(mut file) => {
            let mut contents = String::new();
            file.read_to_string(&mut contents)
                .with_path_context(paperclip_jar)?;
            let Some(result) = contents.splitn(3, '\t').nth(2) else {
                bail!("failed to read download context");
            };
            Ok(Some(result.to_owned()))
        }
        Err(ZipError::FileNotFound) => Ok(None),
        Err(err) => Err(err).with_path_context(paperclip_jar),
    };
    result
}

#[derive(Debug, Deserialize)]
struct PaperBuilds {
    builds: Vec<u32>,
}

#[derive(Debug, Deserialize)]
struct PaperBuildMetadata {
    downloads: PaperDownloads,
}

#[derive(Debug, Deserialize)]
struct PaperDownloads {
    application: PaperDownload,
}

#[derive(Debug, Deserialize)]
struct PaperDownload {
    name: String,
    sha256: Sha2String,
}
