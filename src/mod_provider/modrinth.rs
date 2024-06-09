use crate::cli::select_from_list;
use crate::commands::add::AddModArgs;
use crate::hashing::{HashAlgorithm, HashWithAlgorithm, Sha1String, Sha512String};
use crate::instance::ModMetadata;
use crate::ioutil::{download_large, download_large_with_hash};
use crate::mod_loader::ModLoader;
use crate::mod_provider::ModProvider;
use crate::{make_progress_bar, ContextExt, LINE_ENDING};
use anyhow::{bail, Context};
use reqwest::blocking::Client;
use reqwest::StatusCode;
use serde::{Deserialize, Deserializer};
use sha2::{Digest, Sha512};
use std::cmp::Reverse;
use std::fmt::{Display, Formatter};
use std::fs::File;
use std::{fs, io};
use time::OffsetDateTime;
use url::Url;

const SEARCH_URL: &str = "https://api.modrinth.com/v2/search";

// TODO: download mod dependencies
pub fn add_mod(args: AddModArgs<'_>) -> anyhow::Result<ModMetadata> {
    let mut project = None;
    if !args.command.force_search && is_valid_slug(&args.command.name) {
        project = find_project(args.client, &args.command.name)?;
    }
    let perform_search = project.is_none();
    if perform_search {
        let mut search_result = search_for_mods(
            args.client,
            &args.command.name,
            args.instance_metadata.loader,
        )?;
        search_result
            .hits
            .sort_by_key(|result| result.server_side == SideRequirement::Unsupported);
        let Some(chosen_hit) = select_from_list(
            search_result.hits,
            &format!(
                "mod {} was not found, but similar results were found. Did you mean:",
                args.command.name
            ),
        )?
        else {
            bail!(
                "mod {} was not found, and no similar results were found.",
                args.command.name
            );
        };
        project = find_project(args.client, &chosen_hit.slug)?;
    }
    let Some(project) = project else {
        bail!("mod {} was not found", args.command.name);
    };
    let team_members = get_team_members(args.client, &project.slug)?;
    print_installing_message(&project, &team_members, perform_search);

    if !project.game_versions.is_empty()
        && !project
            .game_versions
            .contains(&args.instance_metadata.minecraft_version)
    {
        if args.command.skip_version_check {
            eprintln!(
                "warning: mod does not support minecraft version {}",
                args.instance_metadata.minecraft_version
            );
        } else {
            bail!(
                "mod does not support minecraft version {}",
                args.instance_metadata.minecraft_version
            );
        }
    }

    let mut versions = get_project_versions(
        args.client,
        &project.slug,
        args.instance_metadata.loader,
        &args.instance_metadata.minecraft_version,
        args.command.skip_version_check,
    )?;
    if versions.is_empty() {
        bail!("mod does not have any matching versions");
    }
    versions.sort_by_key(|version| Reverse(version.date_published));

    let Some((version, file)) = versions
        .iter()
        .flat_map(|version| version.files.iter().map(move |file| (version, file)))
        .find(|(_, file)| file.file_type == ProjectFileType::Regular)
    else {
        bail!("mod does not have any matching files");
    };

    let Some(mods_folder) = args.instance_metadata.loader.mods_folder() else {
        bail!(
            "cannot install mods on loader '{}'",
            args.instance_metadata.loader
        );
    };
    let mods_folder = args.instance_path.join(mods_folder);

    let existing_mod = args
        .instance_metadata
        .mods
        .iter()
        .find(|m| m.provider == ModProvider::Modrinth && m.id == project.id);
    if let Some(existing_mod) = existing_mod {
        let hash_matches = match existing_mod.hash.algorithm {
            HashAlgorithm::Sha1 => {
                Some(&*existing_mod.hash.hash)
                    == file.hashes.sha1.as_ref().map(|sha1| &sha1.inner[..])
            }
            HashAlgorithm::Sha512 => {
                Some(&*existing_mod.hash.hash)
                    == file.hashes.sha512.as_ref().map(|sha512| &sha512.inner[..])
            }
            _ => false,
        };
        if existing_mod.file_name == file.filename && hash_matches {
            bail!("mod is already up-to-date");
        }
    }

    for m in &args.instance_metadata.mods {
        if m.id != project.id && m.file_name == file.filename {
            bail!(
                "mod conflicts with existing mod {} ({}), which also has the filename '{}'",
                m.id,
                m.name,
                m.file_name
            );
        }
    }

    let (mut algorithm, mut hash) = match &file.hashes {
        ProjectFileHashes {
            sha512: Some(sha512),
            ..
        } => (
            Some(HashAlgorithm::Sha512),
            Some(sha512.inner.to_vec().into_boxed_slice()),
        ),
        ProjectFileHashes {
            sha1: Some(sha1), ..
        } => (
            Some(HashAlgorithm::Sha1),
            Some(sha1.inner.to_vec().into_boxed_slice()),
        ),
        _ => (None, None),
    };

    fs::create_dir_all(&mods_folder).with_path_context(&mods_folder)?;
    let mod_path = mods_folder.join(&file.filename);

    if let (Some(algorithm), Some(hash)) = (algorithm, &hash) {
        let pb = make_progress_bar(
            file.size,
            format!("downloading {} {}", project.slug, version.name),
        );
        download_large_with_hash(
            args.client,
            file.url.clone(),
            &mod_path,
            algorithm,
            hash,
            |_| {},
            |progress| pb.set_position(progress),
        )?;
        pb.finish_with_message(format!("downloaded {} {}", project.slug, version.name));
    } else {
        let pb = make_progress_bar(
            file.size,
            format!("downloading {} {}", project.slug, version.name),
        );
        download_large(
            args.client,
            file.url.clone(),
            &mod_path,
            |_| {},
            |progress| pb.set_position(progress),
        )?;
        let mut digest = Sha512::new();
        io::copy(
            &mut File::open(&mod_path).with_path_context(&mod_path)?,
            &mut digest,
        )
        .with_path_context(&mod_path)?;
        algorithm = Some(HashAlgorithm::Sha512);
        hash = Some(digest.finalize().to_vec().into_boxed_slice());
        pb.finish_with_message(format!("downloaded {} {}", project.slug, version.name));
    }

    if let Some(existing_mod) = existing_mod {
        if existing_mod.file_name != file.filename {
            let old_mod_file = mods_folder.join(&existing_mod.file_name);
            if let Err(err) = fs::remove_file(&old_mod_file) {
                if err.kind() != io::ErrorKind::NotFound {
                    return Err(err).with_path_context(&old_mod_file);
                }
            }
        }
    }

    Ok(ModMetadata {
        name: project.slug,
        id: project.id,
        file_name: file.filename.to_owned(),
        hash: HashWithAlgorithm {
            algorithm: algorithm.unwrap(),
            hash: hash.unwrap(),
        },
        provider: ModProvider::Modrinth,
    })
}

fn is_valid_slug(slug: &str) -> bool {
    fn is_valid_slug_char(char: u8) -> bool {
        char.is_ascii_alphanumeric()
            || matches!(
                char,
                b'!' | b'@'
                    | b'$'
                    | b'('
                    | b')'
                    | b'`'
                    | b'.'
                    | b'+'
                    | b','
                    | b'"'
                    | b'\\'
                    | b'-'
                    | b'\''
            )
    }

    (3..=64).contains(&slug.len()) && slug.bytes().all(is_valid_slug_char)
}

fn find_project(client: &Client, slug: &str) -> anyhow::Result<Option<Project>> {
    let url = format!(
        "https://api.modrinth.com/v2/project/{}",
        urlencoding::encode(slug)
    );
    let response = client.get(&url).send().with_context(|| url.clone())?;
    if response.status() == StatusCode::NOT_FOUND {
        return Ok(None);
    } else if !response.status().is_success() {
        bail!(
            "request to {} returned status code {}",
            url,
            response.status()
        );
    }

    response.json().map(Some).with_context(|| url.clone())
}

fn get_team_members(client: &Client, slug: &str) -> anyhow::Result<Vec<TeamMember>> {
    let url = format!(
        "https://api.modrinth.com/v2/project/{}/members",
        urlencoding::encode(slug)
    );
    let response = client.get(&url).send().with_context(|| url.clone())?;
    if !response.status().is_success() {
        bail!(
            "request to {} returned status code {}",
            url,
            response.status()
        );
    }
    response.json().with_context(|| url.clone())
}

fn search_for_mods(
    client: &Client,
    slug: &str,
    loader: ModLoader,
) -> anyhow::Result<SearchResults> {
    let response = client
        .get(SEARCH_URL)
        .query(&[
            ("query", slug),
            (
                "facets",
                &format!("[[\"categories:{loader}\"],[\"project_type:mod\"]]"),
            ),
        ])
        .send()
        .context(SEARCH_URL)?;
    if !response.status().is_success() {
        bail!(
            "request to {} returned status code {}",
            SEARCH_URL,
            response.status()
        );
    }
    response.json().context(SEARCH_URL)
}

fn get_project_versions(
    client: &Client,
    slug: &str,
    loader: ModLoader,
    mc_version: &str,
    skip_version_check: bool,
) -> anyhow::Result<Vec<ProjectVersion>> {
    let url = format!(
        "https://api.modrinth.com/v2/project/{}/version",
        urlencoding::encode(slug)
    );
    let mut request_builder = client
        .get(&url)
        .query(&[("loaders", &format!("[\"{loader}\"]"))]);
    if !skip_version_check {
        request_builder = request_builder.query(&[(
            "game_versions",
            &format!(
                "[\"{}\"]",
                mc_version.replace('\\', "\\\\").replace('"', "\\\"")
            ),
        )]);
    }
    let response = request_builder.send().with_context(|| url.clone())?;
    if !response.status().is_success() {
        bail!(
            "request to {} returned status code {}",
            url,
            response.status()
        );
    }
    response.json().with_context(|| url.clone())
}

fn print_installing_message(
    project: &Project,
    team_members: &[TeamMember],
    performed_search: bool,
) {
    eprintln!(
        "installing {} ({}, {})",
        project.slug, project.id, project.title
    );
    if !team_members.is_empty() {
        eprintln!("by:");
        for team_member in team_members {
            eprintln!("- {} ({})", team_member.user.username, team_member.role);
        }
    }
    if project.game_versions.is_empty() {
        eprintln!("no supported minecraft versions");
    } else {
        eprintln!("supported minecraft versions:");
        for version in &project.game_versions {
            eprintln!("- {}", version);
        }
    }
    if !project.loaders.is_empty() {
        eprintln!("supported loaders:");
        for loader in &project.loaders {
            eprintln!("- {}", loader);
        }
    }
    if !performed_search {
        eprintln!("if this is not the right mod, force a search with -s");
    }
}

#[derive(Debug, Deserialize)]
struct Project {
    slug: String,
    title: String,
    description: String,
    server_side: SideRequirement,
    project_type: ProjectType,
    id: String,
    #[serde(default)]
    game_versions: Vec<String>,
    #[serde(default)]
    loaders: Vec<ModrinthLoader>,
}

#[derive(Debug, Deserialize)]
struct TeamMember {
    user: User,
    role: String,
}

#[derive(Debug, Deserialize)]
struct User {
    username: String,
    #[serde(default)]
    name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SearchResults {
    pub hits: Vec<SearchHit>,
}

#[derive(Debug, Deserialize)]
struct SearchHit {
    slug: String,
    title: String,
    description: String,
    server_side: SideRequirement,
    project_type: ProjectType,
    project_id: String,
    author: String,
    versions: Vec<String>,
}

impl Display for SearchHit {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} ({}) by {}", self.slug, self.title, self.author)?;
        if !self.description.is_empty() {
            write!(f, "{}   {}", LINE_ENDING, self.description)?;
        }
        if self.server_side == SideRequirement::Unsupported {
            write!(f, "{}   warning: client-side only", LINE_ENDING)?;
        }
        Ok(())
    }
}

#[derive(Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
enum SideRequirement {
    Required,
    Optional,
    Unsupported,
    #[serde(other)]
    Unknown,
}

#[derive(Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ProjectType {
    Mod,
    Modpack,
    Resourcepack,
    Shader,
    #[serde(other)]
    Unknown,
}

#[derive(Debug, PartialEq, Eq, Deserialize)]
#[serde(untagged)]
enum ModrinthLoader {
    Known(ModLoader),
    Unknown(String),
}

impl Display for ModrinthLoader {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Known(loader) => Display::fmt(loader, f),
            Self::Unknown(loader) => Display::fmt(loader, f),
        }
    }
}

#[derive(Debug, Deserialize)]
struct ProjectVersion {
    name: String,
    version_number: String,
    dependencies: Vec<ProjectDependency>,
    game_versions: Vec<String>,
    #[serde(with = "time::serde::iso8601")]
    date_published: OffsetDateTime,
    files: Vec<ProjectFile>,
}

#[derive(Debug, Deserialize)]
struct ProjectDependency {
    #[serde(default)]
    version_id: Option<String>,
    #[serde(default)]
    project_id: Option<String>,
    #[serde(default)]
    file_name: Option<String>,
    dependency_type: ProjectDependencyType,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ProjectDependencyType {
    Required,
    Optional,
    Incompatible,
    Embedded,
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Deserialize)]
struct ProjectFile {
    hashes: ProjectFileHashes,
    url: Url,
    filename: String,
    size: u64,
    #[serde(default)]
    #[serde(deserialize_with = "null_to_default")]
    file_type: ProjectFileType,
}

#[derive(Debug, Deserialize)]
struct ProjectFileHashes {
    #[serde(default)]
    sha1: Option<Sha1String>,
    #[serde(default)]
    sha512: Option<Sha512String>,
}

#[derive(Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum ProjectFileType {
    #[default]
    #[serde(skip)]
    Regular,
    RequiredResourcePack,
    OptionalResourcePack,
    #[serde(other)]
    Unknown,
}

fn null_to_default<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: Default + Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer).map(|value| value.unwrap_or_default())
}
