use crate::cli::NewCommand;
use crate::java::{create_java_candidate_for_path, find_java_candidates, JavaCandidate};
use crate::mojang::{Manifest, ManifestVersion, Version};
use crate::{cli, copy_directory, make_client, RUN_SERVER_FILENAME};
use anyhow::{anyhow, bail, Context};
use reqwest::blocking::Client;
use std::borrow::Cow;
use std::cmp::Ordering;
use std::fmt::Display;
use std::fs;
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};

pub fn make_new_instance(command: NewCommand, cache_dir: PathBuf) -> anyhow::Result<()> {
    let instance_path = PathBuf::from(&command.name);
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

    let version = command
        .version
        .as_deref()
        .unwrap_or(&manifest.latest.release);
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

    let (required_java_version, required_java_version_reason): (_, &dyn Display) =
        if command.loader.minimum_java_version() > full_version.java_version.major_version {
            (command.loader.minimum_java_version(), &command.loader)
        } else {
            (full_version.java_version.major_version, &version)
        };

    let java_candidate = if let Some(java_exe) = command.custom_java_exe.clone() {
        let java_candidate = create_java_candidate_for_path(java_exe, &mut None)?;
        if !command.skip_java_check && java_candidate.version.major < required_java_version {
            bail!("specified java is not compatible with {required_java_version_reason}, need at least java {required_java_version}");
        }
        java_candidate
    } else {
        eprintln!("searching for java versions");
        let mut java_candidates = find_java_candidates()?;
        if !command.skip_java_check {
            java_candidates.retain(|candidate| candidate.version.major >= required_java_version);
        }

        // sort by major version ascending (to most closely match the required java version), and then by version descending, to prioritize the latest of each major version.
        // also put the versions that are too old at the end
        java_candidates.sort_by(|candidate1, candidate2| {
            let candidate1_old = candidate1.version.major < required_java_version;
            let candidate2_old = candidate2.version.major < required_java_version;
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
            bail!("could not find any java install compatible with {required_java_version_reason}, need at least java {required_java_version}");
        };
        java_candidate
    };
    if !command.skip_java_check && java_candidate.version.major > required_java_version {
        eprintln!("warning: selected java version {} is newer than the recommended java version {required_java_version}, which may cause issues", java_candidate.version);
    }

    command.loader.install(ServerInstallArgs {
        command: &command,
        client: &client,
        cache_dir: &cache_dir,
        instance_path: &instance_path,
        version_name: version,
        manifest_version: &manifest_version,
        full_version: &full_version,
        java_candidate: &java_candidate,
    })?;

    if command.config_template == cache_dir.join("default-config-template")
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

pub struct ServerInstallArgs<'a> {
    pub command: &'a NewCommand,
    pub client: &'a Client,
    pub cache_dir: &'a Path,
    pub instance_path: &'a Path,
    pub version_name: &'a str,
    pub manifest_version: &'a ManifestVersion,
    pub full_version: &'a Version,
    pub java_candidate: &'a JavaCandidate,
}

impl ServerInstallArgs<'_> {
    pub fn escaped_java_exe_name(&self) -> anyhow::Result<Cow<str>> {
        Ok(escape_executable_name(
            self.java_candidate
                .path
                .to_str()
                .ok_or_else(|| anyhow!("java path had invalid UTF-8 characters"))?,
        ))
    }
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

pub fn write_run_server_file(args: &ServerInstallArgs<'_>, command: &str) -> anyhow::Result<()> {
    let run_server_path = args.instance_path.join(RUN_SERVER_FILENAME);
    let mut open_options = File::options();
    open_options.create(true).truncate(true).write(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut open_options, 0o744);
    open_options
        .open(&run_server_path)
        .with_context(|| run_server_path.display().to_string())?
        .write_all(command.as_bytes())
        .with_context(|| run_server_path.display().to_string())?;

    Ok(())
}
