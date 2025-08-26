mod cli;

use anyhow::{Context, Result};
use clap::Parser;
use cli::Args;
use dockworker::Docker;
use dockworker::response::Response;
use futures::stream::StreamExt;
use futures::stream::TryStreamExt;
use std::fs::{self, File};
use std::path::Path;
use sys_mount::SupportedFilesystems;
use tar::Archive;

#[derive(Default, Debug)]
struct NamespaceInfo {
    pid: u32,
    mount_ns: usize,
    net_ns: usize,
    pid_ns: usize,
    ipc_ns: usize,
    uts_ns: usize,
    user_ns: usize,
    cgroup_ns: usize,
}

impl NamespaceInfo {
    fn open_mount_ns(&self) -> Result<File> {
        File::open(
            Path::new("/proc")
                .join(self.pid.to_string())
                .join("ns")
                .join("mnt"),
        )
        .map_err(|e| anyhow::anyhow!("failed to open mount ns: {}", e))
    }
}

fn parse_namespace_content(content: &str) -> Result<usize> {
    let start = content
        .find('[')
        .context("invalid namespace start content")?;
    let end = content.find(']').context("invalid namespace end content")?;

    let id_str = &content[start + 1..end];
    let id = id_str.parse::<usize>()?;

    Ok(id)
}

fn get_namespace_info_by_pid(pid: u32) -> Result<NamespaceInfo> {
    // first check pid exist
    let pid_path = Path::new("/proc").join(pid.to_string());
    if !pid_path.exists() {
        return Err(anyhow::anyhow!("PID {} does not exist", pid));
    }

    let mut info = NamespaceInfo::default();
    info.pid = pid;

    if let Ok(content) = fs::read_link(pid_path.join("ns/mnt")) {
        if let Ok(ns_id) = parse_namespace_content(content.to_str().unwrap()) {
            info.mount_ns = ns_id;
        }
    }

    Ok(info)
}

async fn export_overlay_image(image: &str, work_dir: &str) -> Result<()> {
    let extract_dir = Path::new(work_dir).join("extract");
    let upper_dir = Path::new(work_dir).join("upper");

    // makesure dst is empty
    tokio::fs::remove_dir_all(work_dir).await?;
    tokio::fs::create_dir_all(work_dir).await?;
    tokio::fs::create_dir_all(&extract_dir).await?;
    tokio::fs::create_dir_all(&upper_dir).await?;

    let docker = Docker::connect_with_defaults()?;

    println!("pulling overlay image: {}", image);
    let (image_name, tag) = image.split_once(":").unwrap_or((image, "latest"));
    let mut download_stats = docker.create_image(image_name, tag).await?;
    while let Some(Ok(stat)) = download_stats.next().await {
        match stat {
            Response::Status(status) => {
                println!("{status:?}");
            }
            Response::Progress(progress) => {
                println!("{progress:?}");
            }
            Response::Error(err) => {
                println!("error: {err:?}");
            }
            _ => {}
        }
    }

    // TODO: optimsie with tar stream decompression in memory?
    {
        println!("exporting overlay image: {}", image);
        let mut tmp_file = tokio::fs::File::create("temp.tar").await?;
        let img_res = docker.export_image(image).await?;
        let mut res = tokio_util::io::StreamReader::new(
            img_res.map_err(|err| std::io::Error::new(std::io::ErrorKind::Other, err)),
        );
        tokio::io::copy(&mut res, &mut tmp_file).await.unwrap();
    }

    

    println!("extracting raw overlay image to memory: {}", image);
    let mut tar_archive = Archive::new(File::open("temp.tar")?);
    for file in tar_archive.entries().unwrap() {
        let mut tar_file = file?;
        let dst_path = extract_dir.join(tar_file.path()?);

        match tar_file.header().entry_type() {
            tar::EntryType::Regular => {
                let mut dst_file = File::create(dst_path)?;
                std::io::copy(&mut tar_file, &mut dst_file)?;

                // if dst_path.ends_with("manifest.json") {
                //     println!("image manifest: {:?}", image_manifest);
                // }
            }
            tar::EntryType::Directory => {
                tokio::fs::create_dir_all(dst_path).await?;
            }
            _ => println!(
                "warning: skipping entry type: {:?} for {}",
                tar_file.header().entry_type(),
                dst_path.display()
            ),
        }
    }

    println!("extracting overlay rootfs: {}", image);

    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::try_parse()?;

    // export rootfs image
    export_overlay_image(&args.overlay_image, &args.workdir).await?;

    // check for overlay support
    let supported = match SupportedFilesystems::new() {
        Ok(supported) => supported,
        Err(why) => {
            eprintln!("failed to get supported file systems: {}", why);
            return Err(anyhow::anyhow!(
                "failed to get supported file systems: {}",
                why
            ));
        }
    };
    if !supported.is_supported("overlay") {
        return Err(anyhow::anyhow!("overlay is not supported"));
    }

    // let info = get_namespace_info_by_pid(args.pid)?;
    // println!("Namespace info: {:?}", info);

    // let mount_fs = info.open_mount_ns()?;

    // // change current process into target namespace
    // let err_no = unsafe { libc::setns(mount_fs.as_raw_fd(), libc::CLONE_NEWNS) };
    // if err_no != 0 {
    //     println!("setns failed: {}", err_no);
    //     return Err(anyhow::anyhow!("setns failed"));
    // }

    // // run process in namespace
    // let output = Command::new("ls").arg("/").output()?;
    // println!("{}", String::from_utf8_lossy(&output.stdout));

    // // clone mount namespace
    // let err_no = unsafe { libc::unshare(libc::CLONE_NEWNS) };
    // if err_no != 0 {
    //     println!("unshare failed: {}", err_no);
    //     return Err(anyhow::anyhow!("unshare failed"));
    // }

    // // run process in namespace
    // let output = Command::new("ls").arg("/").output()?;
    // println!("{}", String::from_utf8_lossy(&output.stdout));

    // create file
    // {
    //     let mut file = File::create("/test.txt")?;
    //     file.write_all(b"hello")?;
    //     file.sync_all()?;
    // }

    // // wait user input
    // let mut buffer = String::new();
    // io::stdin().read_line(&mut buffer)?;
    // let output = Command::new("ls").arg("/").output()?;
    // println!("{}", String::from_utf8_lossy(&output.stdout));

    // create overlay NS

    // libc::clone

    Ok(())
}
