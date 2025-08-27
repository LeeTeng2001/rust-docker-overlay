mod cli;
mod docker_helper;

use anyhow::{Context, Result};
use clap::Parser;
use cli::Args;
use dockworker::Docker;
use dockworker::response::Response;
use futures::stream::StreamExt;
use futures::stream::TryStreamExt;
use oci_spec::image::MediaType;
use std::collections::HashMap;
use std::ffi::OsString;
use std::fs::{self, File};
use std::io::Read;
use std::os::fd::AsRawFd;
use std::os::unix;
use std::path::Path;
use std::path::PathBuf;
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

struct OverlayBuildInfo {
    upper_dir: PathBuf,
}

async fn export_overlay_image(image: &str, work_dir: &str, pull: bool) -> Result<OverlayBuildInfo> {
    let extract_dir = Path::new(work_dir).join("extract");
    let upper_dir = Path::new(work_dir).join("upper");

    // makesure dst is empty
    tokio::fs::remove_dir_all(work_dir).await?;
    tokio::fs::create_dir_all(work_dir).await?;
    tokio::fs::create_dir_all(&extract_dir).await?;
    tokio::fs::create_dir_all(&upper_dir).await?;

    let docker = Docker::connect_with_defaults()?;

    if pull {
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

    // manifest
    let mut manifest: Vec<docker_helper::DockerManifest> = Vec::new();
    let mut blob: HashMap<String, Vec<u8>> = HashMap::new();

    println!("extracting raw overlay image to memory: {}", image);
    let mut tar_archive = Archive::new(File::open("temp.tar")?);
    for file in tar_archive.entries().unwrap() {
        let mut tar_file = file?;
        let path = tar_file.path()?;
        let dst_path = extract_dir.join(&path);

        match tar_file.header().entry_type() {
            tar::EntryType::Regular => {
                if path.ends_with("manifest.json") {
                    manifest = serde_json::from_reader(&mut tar_file)?;
                } else if path.starts_with("blobs/sha256/") {
                    let entry_name = path.to_str().unwrap().to_string();
                    let mut content_buffer = Vec::new();
                    tar_file.read_to_end(&mut content_buffer)?;
                    blob.insert(entry_name, content_buffer);
                } else {
                    let mut dst_file = File::create(dst_path)?;
                    std::io::copy(&mut tar_file, &mut dst_file)?;
                }
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

    println!("parsing manifest & extract rootfs");
    if manifest.len() == 0 {
        return Err(anyhow::anyhow!("no manifest found"));
    }
    // TODO: usually manifest only has one entry?
    if manifest.len() > 1 {
        println!("warning: multiple manifest entries found, only the first one will be used");
    }
    let manifest = manifest.first().unwrap();
    for layer in manifest.layers.iter() {
        let layer_blob = blob
            .get(layer)
            .ok_or(anyhow::anyhow!("layer blob not found"))?;
        let layer_entry_name = layer
            .splitn(2, '/')
            .nth(1)
            .unwrap()
            .replace("/", ":")
            .to_string();
        let layer_info = manifest
            .layer_sources
            .get(&layer_entry_name)
            .ok_or(anyhow::anyhow!("layer info not found"))?;

        // TODO: support other format
        let layer_type = MediaType::from(&layer_info.media_type[..]);
        if layer_type != MediaType::ImageLayer {
            return Err(anyhow::anyhow!(
                "unsupported layer type: {}",
                layer_info.media_type
            ));
        }

        // decompress
        let mut tar_archive = Archive::new(std::io::Cursor::new(layer_blob));
        for entry in tar_archive.entries().unwrap() {
            let mut tar_file = entry?;
            let path = tar_file.path()?;
            let dst_path = upper_dir.join(&path);

            match tar_file.header().entry_type() {
                tar::EntryType::Regular => {
                    let mut dst_file = File::create(dst_path)?;
                    std::io::copy(&mut tar_file, &mut dst_file)?;
                }
                tar::EntryType::Directory => {
                    tokio::fs::create_dir_all(dst_path).await?;
                }
                tar::EntryType::Symlink | tar::EntryType::Link => {
                    let link = tar_file
                        .header()
                        .link_name()?
                        .unwrap()
                        .to_string_lossy()
                        .to_string();
                    let original_path = Path::new(&link);
                    unix::fs::symlink(original_path, &dst_path)
                        .map_err(|e| anyhow::anyhow!("failed to symlink: {}", e))?;
                }
                _ => println!(
                    "warning: skipping entry type: {:?} for {}",
                    tar_file.header().entry_type(),
                    dst_path.display()
                ),
            }
        }
    }

    Ok(OverlayBuildInfo { upper_dir })
}

fn create_overlay_ns(build_info: &OverlayBuildInfo) -> Result<()> {
    // find rootfs id
    let width = 15;
    for mount_entry in procfs::mounts()? {
        println!("Device: {}", mount_entry.fs_spec);
        println!("{:>width$}: {}", "Mount point", mount_entry.fs_file);
        println!("{:>width$}: {}", "FS type", mount_entry.fs_vfstype);
        println!("{:>width$}: {}", "Dump", mount_entry.fs_freq);
        println!("{:>width$}: {}", "Check", mount_entry.fs_passno);
        print!("{:>width$}: ", "Options");
        for (name, entry) in mount_entry.fs_mntops {
            if let Some(entry) = entry {
                print!("{name}: {entry} ");
            }
        }
        println!("");
    }

    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::try_parse()?;

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

    // get running container info
    let target_process = procfs::process::Process::new(args.pid as i32)?;
    let namespaces = target_process.namespaces().unwrap();
    let mount_ns = namespaces.0.get(&OsString::from("mnt")).unwrap();

    // export rootfs image
    let build_info = export_overlay_image(&args.overlay_image, &args.workdir, args.pull).await?;

    // enter process mount namespace
    {
        println!("entering mount namespace: {}", mount_ns.path.display());
        let mount_desc = File::open(mount_ns.path.clone())?;
        let err_no = unsafe { libc::setns(mount_desc.as_raw_fd(), libc::CLONE_NEWNS) };
        if err_no != 0 {
            println!("setns failed: {}", err_no);
            return Err(anyhow::anyhow!("setns failed"));
        }
    }

    // create overlay ns
    // create_overlay_ns(&build_info)?;

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
