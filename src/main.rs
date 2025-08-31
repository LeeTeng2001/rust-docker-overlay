mod cli;
mod docker_helper;
mod namespace_helper;

use anyhow::{Context, Result};
use clap::Parser;
use cli::Args;
use dockworker::Docker;
use dockworker::container;
use dockworker::container::Mount;
use dockworker::response::Response;
use futures::stream::StreamExt;
use futures::stream::TryStreamExt;
use oci_spec::image::MediaType;
use procfs::MountEntry;
use std::collections::HashMap;
use std::env::set_current_dir;
use std::ffi::CString;
use std::ffi::OsString;
use std::fs::Permissions;
use std::fs::create_dir_all;
use std::fs::set_permissions;
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;
use std::process::Stdio;
use std::ptr::null;
use std::thread;
use std::time::Duration;
use sys_mount::MountFlags;
use sys_mount::SupportedFilesystems;
use sys_mount::Unmount;
use sys_mount::UnmountFlags;
use tokio::runtime::Runtime;

fn print_process_count() -> Result<()> {
    // https://man7.org/linux/man-pages/man2/setns.2.html
    // let mut count = 0;
    // for process in procfs::process::all_processes()? {
    //     count += 1;
    // }
    // println!("process count: {}", count);
    let output = Command::new("cat")
        .args(["/proc/stat", "|", "grep", "procs_running"])
        .output()?;
    println!("{}", String::from_utf8_lossy(&output.stdout));
    Ok(())
}

fn debug_mount() -> Result<()> {
    let width = 15;
    for mount_entry in procfs::mounts()? {
        if !mount_entry.fs_vfstype.eq("overlay") {
            continue;
        }
        println!("Device: {}", mount_entry.fs_spec);
        println!("{:>width$}: {}", "Mount point", mount_entry.fs_file);
        println!("{:>width$}: {}", "FS type", mount_entry.fs_vfstype);
        // println!("{:>width$}: {}", "Dump", mount_entry.fs_freq);
        // println!("{:>width$}: {}", "Check", mount_entry.fs_passno);
        // print!("{:>width$}: ", "Options");
        // for (name, entry) in mount_entry.fs_mntops {
        //     if let Some(entry) = entry {
        //         print!("{name}: {entry} ");
        //     }
        // }
        println!("");
    }

    Ok(())
}

fn debug_ls() -> Result<()> {
    let output = Command::new("ls").arg(".").output()?;
    println!("{}", String::from_utf8_lossy(&output.stdout));
    Ok(())
}

// this is necessary to force single thread for setns
fn main() -> Result<()> {
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

    // init
    let rt = Runtime::new()?;
    let args = Args::try_parse()?;
    let docker = docker_helper::DockerHelper::new()?;

    // get container info & unmount previously mounted specs
    let container_info = rt.block_on(docker.get_container_info(&args.id))?;
    println!("container info: {:?}", container_info);
    for mount_entry in procfs::mounts()? {
        if mount_entry.fs_vfstype != "overlay" {
            continue;
        }
        if mount_entry.fs_file == container_info.merged_dir {
            continue;
        }
        let lower_dir = mount_entry
            .fs_mntops
            .get("lowerdir")
            .context("overlayfs should contains lowerdir")?
            .clone()
            .context("lowerdir should have value")?;
        if lower_dir == container_info.merged_dir {
            // TODO: unmount logic
            println!(
                "found previous overlay mount, unmounting: {}",
                mount_entry.fs_file
            );
            sys_mount::unmount(mount_entry.fs_file, UnmountFlags::DETACH)?;
        }
    }

    // prepare work directory
    let work_dir = Path::new(&args.workdir);
    let image_extract_dir = Path::new(work_dir).join("extract_tmp");
    let image_base_dir = Path::new(work_dir).join("image_base");
    let overlay_work_dir = Path::new(work_dir).join("work");
    let overlay_upper_dir = Path::new(work_dir).join("upper");
    let rootfs_dir = Path::new(work_dir).join("rootfs");
    if work_dir.exists() {
        // unmount if mounted
        let _ = fs::remove_dir_all(work_dir);
    }
    create_dir_all(&work_dir)?;
    create_dir_all(&image_extract_dir)?;
    create_dir_all(&image_base_dir)?;
    create_dir_all(&overlay_work_dir)?;
    create_dir_all(&overlay_upper_dir)?;
    create_dir_all(&rootfs_dir)?;

    // image preparation
    rt.block_on(docker.export_overlay_image(
        &args.overlay_image,
        &image_extract_dir,
        &image_base_dir,
        args.pull,
    ))?;
    rt.shutdown_timeout(Duration::from_secs(0));

    // build rootfs mount
    let mount_opt = format!(
        "lowerdir={},upperdir={},workdir={}",
        container_info.merged_dir,
        overlay_upper_dir.display(),
        overlay_work_dir.display(),
    );
    let mount_res = sys_mount::Mount::builder()
        .fstype("overlay")
        .data(&mount_opt)
        .mount(image_base_dir, &rootfs_dir)
        .context("failed to mount overlayfs")?;
    // TODO: make mount temporary?
    // mount_res.into_unmount_drop(UnmountFlags::DETACH);

    // prepare init script
    {
        let init_script_content = include_str!("init.sh");
        let mut init_script_file = File::create(rootfs_dir.join("init.sh"))?;
        init_script_file.write_all(init_script_content.as_bytes())?;
        init_script_file.set_permissions(Permissions::from_mode(0o755))?;
    }

    // enter container namespace
    namespace_helper::enter_namespace(
        container_info.pid as i32,
        libc::CLONE_NEWCGROUP
                | libc::CLONE_NEWIPC
                | libc::CLONE_NEWNET
                // | libc::CLONE_NEWNS // we will enter mount from host
                | libc::CLONE_NEWPID
                | libc::CLONE_NEWUTS,
    )?;

    // fork 1
    let fork_res = unsafe { libc::fork() };
    match fork_res {
        // In the child process
        0 => {
            // println!("fork 1 child");
        }
        // In the parent process
        pid if pid > 0 => {
            // println!("fork 1 parent");
            unsafe {
                libc::wait(0 as *mut i32);
            }
            // println!("fork 1 parent exit ");
            return Ok(());
        }
        // If fork fails
        _ => {
            eprintln!("Fork failed");
            return Err(anyhow::anyhow!("Fork failed"));
        }
    }

    // clone mount namespace
    let enter_res = unsafe { libc::unshare(libc::CLONE_NEWNS) };
    if enter_res != 0 {
        eprintln!("Failed to unshare namespaces, {}", enter_res);
        return Err(anyhow::anyhow!(
            "Failed to unshare namespaces, {}",
            enter_res
        ));
    }

    // fork 2
    let fork_res = unsafe { libc::fork() };
    match fork_res {
        // In the child process
        0 => {
            // println!("Child process 2");
            set_current_dir(&rootfs_dir)?;
            let exec_res = unsafe {
                let cmd = CString::new("/usr/bin/bash").expect("CString::new failed");
                let arg1 = CString::new("--init-file").expect("CString::new failed");
                let arg2 = CString::new("init.sh").expect("CString::new failed");
                let args = [
                    arg1.as_ptr(),
                    arg2.as_ptr(),
                    null(), // Null-terminated argument list
                ];
                libc::execv(cmd.as_ptr(), args.as_ptr())
            };
            println!("Exec failed: {}", exec_res);
        }
        // In the parent process
        pid if pid > 0 => {
            // println!("Parent process 2");
            unsafe {
                libc::wait(0 as *mut i32);
            }
            // println!("Parent process exit ");
            return Ok(());
        }
        // If fork fails
        _ => {
            eprintln!("Fork failed");
            return Err(anyhow::anyhow!("Fork failed"));
        }
    }

    Ok(())
}
