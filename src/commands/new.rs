use crate::cli::NewCommand;
use crate::java::{create_java_candidate_for_path, find_java_candidates};
use crate::mojang::Manifest;
use crate::{cli, copy_directory, link_or_copy, make_client, LINE_ENDING, RUN_SERVER_FILENAME};
use anyhow::{anyhow, bail, Context};
use indicatif::{ProgressBar, ProgressStyle};
use std::borrow::Cow;
use std::cmp::Ordering;
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::{fs, io};
use time::macros::datetime;
use time::OffsetDateTime;

const TIME_13W39A: OffsetDateTime = datetime!(2013-09-26 15:11:19 UTC);
const TIME_17W15A: OffsetDateTime = datetime!(2017-04-12 09:30:50 UTC);
const TIME_1_17_PRE1: OffsetDateTime = datetime!(2021-05-27 09:39:21 UTC);
const TIME_1_18_1_RC3: OffsetDateTime = datetime!(2021-12-10 03:36:38 UTC);

pub fn make_new_instance(command: NewCommand, cache_dir: PathBuf) -> anyhow::Result<()> {
    let instance_path = PathBuf::from(command.name);
    if instance_path.exists() {
        bail!("an instance with that name already exists");
    }

    let client = make_client()?;

    eprintln!("fetching minecraft versions");
    let manifest = Manifest::download(
        &client,
        &cache_dir.join("version_manifest.json"),
        &cache_dir.join("version_manifest.json.etag"),
    )?;

    let version = command.version.unwrap_or(manifest.latest.release);
    let Some(manifest_version) = manifest.versions.into_iter().find(|ver| ver.id == version) else {
        bail!("no such version: {version}");
    };

    eprintln!("fetching metadata for version {version}");
    let version_metadata_path = cache_dir.join("version_metadata");
    fs::create_dir_all(&version_metadata_path)?;
    let full_version = manifest_version.download(
        &client,
        &version_metadata_path.join(format!("{version}.json")),
    )?;

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

    let Some(server_download) = full_version.downloads.server else {
        bail!("version {version} does not have a server download");
    };
    let server_download_path = cache_dir.join("jars");
    fs::create_dir_all(&server_download_path)?;
    let server_jar_path = server_download_path.join(format!("{version}.jar"));

    let pb = ProgressBar::new(server_download.size).with_message("downloading server jar");
    pb.set_style(
        ProgressStyle::default_bar()
            .template("{msg}\n{bar:40.cyan/blue} {bytes}/{total_bytes} ({eta})")
            .unwrap()
            .progress_chars("##-"),
    );
    server_download.download(&client, &server_jar_path, |progress| {
        pb.set_position(progress)
    })?;
    pb.finish_with_message("downloaded server jar");

    fs::create_dir(&instance_path)?;

    link_or_copy(server_jar_path, instance_path.join("server.jar"))?;

    let mut start_server_command = format!(
        "{} ",
        escape_executable_name(
            java_candidate
                .path
                .to_str()
                .ok_or_else(|| anyhow!("java path had invalid UTF-8 characters"))?
        )
    );

    if (TIME_13W39A..TIME_1_18_1_RC3).contains(&manifest_version.release_time) {
        if manifest_version.release_time < TIME_17W15A {
            let log4j_config_path = instance_path.join("log4j2_17-111.xml");
            fs::write(
                &log4j_config_path,
                include_str!("../../res/log4j2_17-111.xml"),
            )
            .with_context(|| log4j_config_path.display().to_string())?;
            start_server_command.push_str("-Dlog4j.configurationFile=log4j2_17-111.xml ");
        } else if manifest_version.release_time < TIME_1_17_PRE1 {
            let log4j_config_path = instance_path.join("log4j2_112-116.xml");
            fs::write(
                &log4j_config_path,
                include_str!("../../res/log4j2_112-116.xml"),
            )
            .with_context(|| log4j_config_path.display().to_string())?;
            start_server_command.push_str("-Dlog4j.configurationFile=log4j2_112-116.xml ");
        } else {
            start_server_command.push_str("-Dlog4j2.formatMsgNoLookups=true ");
        }
    }

    start_server_command.push_str("-jar server.jar nogui");
    start_server_command.push_str(LINE_ENDING);

    let run_server_path = instance_path.join(RUN_SERVER_FILENAME);
    let mut open_options = File::options();
    open_options.create(true).truncate(true).write(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut open_options, 0o744);
    open_options
        .open(&run_server_path)
        .with_context(|| run_server_path.display().to_string())?
        .write_all(start_server_command.as_bytes())
        .with_context(|| run_server_path.display().to_string())?;

    let mut eula = command.eula;
    if !eula {
        eprintln!("Do you agree to the Minecraft EULA (y/N)? You can read the EULA at https://aka.ms/MinecraftEULA");
        let mut input = String::new();
        io::stdin().read_line(&mut input)?;
        eula = input.starts_with('y') || input.starts_with('Y');
    }

    if eula {
        let eula_path = instance_path.join("eula.txt");
        fs::write(&eula_path, format!("eula=true{}", LINE_ENDING))
            .with_context(|| eula_path.display().to_string())?;
    }

    if command.config_template == Path::new("default-config-template")
        && !command.config_template.exists()
    {
        // sync-chunk-writes is on by default but super slow on unix systems
        #[cfg(unix)]
        let default_server_properties = concat!(
            "sync-chunk-writes=false\n",
            include_str!("../../res/default-server.properties")
        );
        #[cfg(not(unix))]
        let default_server_properties = include_str!("../../res/default-server.properties");

        fs::create_dir(&command.config_template)
            .with_context(|| command.config_template.display().to_string())?;
        let properties_template_path = command.config_template.join("server.properties");
        fs::write(&properties_template_path, default_server_properties)
            .with_context(|| properties_template_path.display().to_string())?;
    }

    copy_directory(&command.config_template, &instance_path).with_context(|| {
        format!(
            "copying from {} to {}",
            command.config_template.display(),
            instance_path.display()
        )
    })?;

    Ok(())
}

#[cfg(windows)]
fn escape_executable_name(exe_name: &str) -> Cow<str> {
    fn char_needs_escape(c: char) -> bool {
        if c.is_whitespace() {
            return true;
        }
        !matches!(
            c,
            '%' | '^' | '&' | '<' | '>' | '|' | '\'' | '"' | '(' | ')'
        )
    }

    if !exe_name.chars().any(char_needs_escape) {
        return exe_name.into();
    }

    format!("\"{}\"", exe_name.replace('"', "\"\"").replace('%', "%%")).into()
}

#[cfg(not(windows))]
fn escape_executable_name(exe_name: &str) -> Cow<str> {
    fn char_needs_escape(index: usize, c: char) -> bool {
        if c.is_whitespace() {
            return true;
        }
        match c {
            '!' | '"' | '#' | '$' | '&' | '\'' | '(' | ')' | '*' | ';' | '<' | '=' | '>' | '?'
            | '[' | '\\' | ']' | '^' | '`' | '{' | '|' | '}' => true,
            '~' => index != 0,
            _ => false,
        }
    }

    if !exe_name
        .char_indices()
        .any(|(index, c)| char_needs_escape(index, c))
    {
        return exe_name.into();
    }

    format!("'{}'", exe_name.replace('\'', "'\\''")).into()
}
