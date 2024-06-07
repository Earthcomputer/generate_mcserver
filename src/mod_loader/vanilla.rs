use crate::commands::new::{write_run_server_file, ServerInstallArgs};
use crate::{ioutil, make_progress_bar, ContextExt, LINE_ENDING};
use anyhow::{bail, Context};
use std::path::PathBuf;
use std::{fs, io};
use time::macros::datetime;
use time::OffsetDateTime;

const TIME_13W39A: OffsetDateTime = datetime!(2013-09-26 15:11:19 UTC);
const TIME_17W15A: OffsetDateTime = datetime!(2017-04-12 09:30:50 UTC);
const TIME_1_17_PRE1: OffsetDateTime = datetime!(2021-05-27 09:39:21 UTC);
const TIME_1_18_1_RC3: OffsetDateTime = datetime!(2021-12-10 03:36:38 UTC);

pub fn install_vanilla(args: ServerInstallArgs<'_>) -> anyhow::Result<()> {
    let server_jar_path = download_vanilla_server(&args)?;

    fs::create_dir_all(args.instance_path).with_path_context(args.instance_path)?;

    let link_path = args.instance_path.join("server.jar");
    ioutil::link_or_copy(&server_jar_path, &link_path).with_context(|| {
        format!(
            "linking {} to {}",
            link_path.display(),
            server_jar_path.display()
        )
    })?;

    let mut start_server_command = format!("{} ", args.escaped_java_exe_name()?);

    apply_vanilla_log4j_fix(&args, &mut start_server_command)?;

    start_server_command.push_str("-jar server.jar nogui");
    start_server_command.push_str(LINE_ENDING);

    write_run_server_file(&args, &start_server_command)?;

    agree_to_eula(&args)?;

    Ok(())
}

pub fn download_vanilla_server(args: &ServerInstallArgs<'_>) -> anyhow::Result<PathBuf> {
    let Some(server_download) = &args.full_version.downloads.server else {
        bail!(
            "version {} does not have a server download",
            args.version_name
        );
    };
    let server_download_path = args.cache_dir.join("jars");
    fs::create_dir_all(&server_download_path)?;
    let server_jar_path = server_download_path.join(format!("{}.jar", args.version_name));

    let pb = make_progress_bar(server_download.size, "downloading server jar");
    server_download.download(args.client, &server_jar_path, |progress| {
        pb.set_position(progress)
    })?;
    pb.finish_with_message("downloaded server jar");

    Ok(server_jar_path)
}

pub fn agree_to_eula(args: &ServerInstallArgs<'_>) -> anyhow::Result<()> {
    let mut eula = args.command.eula;
    if !eula {
        eprintln!("Do you agree to the Minecraft EULA (y/N)? You can read the EULA at https://aka.ms/MinecraftEULA");
        let mut input = String::new();
        io::stdin().read_line(&mut input)?;
        eula = input.starts_with('y') || input.starts_with('Y');
    }

    if eula {
        let eula_path = args.instance_path.join("eula.txt");
        fs::write(&eula_path, format!("eula=true{}", LINE_ENDING)).with_path_context(&eula_path)?;
    }

    Ok(())
}

fn apply_vanilla_log4j_fix(
    args: &ServerInstallArgs,
    start_server_command: &mut String,
) -> anyhow::Result<()> {
    if (TIME_13W39A..TIME_1_18_1_RC3).contains(&args.manifest_version.release_time) {
        if args.manifest_version.release_time < TIME_17W15A {
            let log4j_config_path = args.instance_path.join("log4j2_17-111.xml");
            fs::write(
                &log4j_config_path,
                include_str!("../../res/log4j2_17-111.xml"),
            )
            .with_path_context(&log4j_config_path)?;
            start_server_command.push_str("-Dlog4j.configurationFile=log4j2_17-111.xml ");
        } else if args.manifest_version.release_time < TIME_1_17_PRE1 {
            let log4j_config_path = args.instance_path.join("log4j2_112-116.xml");
            fs::write(
                &log4j_config_path,
                include_str!("../../res/log4j2_112-116.xml"),
            )
            .with_path_context(&log4j_config_path)?;
            start_server_command.push_str("-Dlog4j.configurationFile=log4j2_112-116.xml ");
        } else {
            start_server_command.push_str("-Dlog4j2.formatMsgNoLookups=true ");
        }
    }

    Ok(())
}
