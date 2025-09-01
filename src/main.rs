mod cli;
mod docker_helper;
mod namespace_helper;
mod utils;

use anyhow::{Context, Result};
use clap::Parser;
use cli::Args;
use std::env::set_current_dir;
use std::ffi::CString;
use std::fs::Permissions;
use std::fs::create_dir_all;
use std::fs::{self, File};
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::path::absolute;
use std::time::Duration;
use sys_mount::MountFlags;
use sys_mount::SupportedFilesystems;
use sys_mount::UnmountFlags;
use tokio::runtime::Runtime;

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

    let cache_dir = Path::new(&args.cache_dir);
    let work_dir = Path::new(&args.workdir);
    let abs_workdir = absolute(&work_dir)?;

    // get container info & unmount all previously mounted specs
    let container_info = rt.block_on(docker.get_container_info(&args.id))?;
    println!("container info: {:?}", container_info);
    for mount_entry in procfs::mounts()? {
        // unmount everything under workdir
        if mount_entry
            .fs_file
            .starts_with(&abs_workdir.to_str().unwrap())
        {
            println!("unmounting: {}", mount_entry.fs_file);
            sys_mount::unmount(mount_entry.fs_file, UnmountFlags::DETACH)?;
        }
    }

    // prepare work directory
    let image_extract_dir = work_dir.join("tmp_extract");
    let overlay_lower_dir = work_dir.join("tmp_lower");
    let overlay_work_dir = work_dir.join("tmp_work");
    let rootfs_base_dir = work_dir.join("rootfs");
    let abs_rootfs_base_dir = absolute(&rootfs_base_dir)?;
    let mergedfs_dir = work_dir.join("mergedfs");
    if work_dir.exists() {
        let _ = fs::remove_dir_all(work_dir);
    }
    create_dir_all(&overlay_lower_dir)?;
    create_dir_all(&cache_dir)?;
    create_dir_all(&work_dir)?;
    create_dir_all(&image_extract_dir)?;
    create_dir_all(&rootfs_base_dir)?;
    create_dir_all(&overlay_work_dir)?;
    create_dir_all(&mergedfs_dir)?;

    // image preparation
    let mut found_cache = false;
    if args.cache {
        let cache_path = cache_dir.join(args.image_cache_filename());
        if cache_path.exists() {
            found_cache = true;
            println!("found cache: {}", cache_path.display());
            let mut f = File::open(cache_path)?;
            utils::extract_archive(&mut f, &rootfs_base_dir)?;
        }
    }

    if !found_cache {
        rt.block_on(docker.export_overlay_image(
            &args.image,
            &image_extract_dir,
            &rootfs_base_dir,
            args.pull,
        ))?;
    }
    rt.shutdown_timeout(Duration::from_secs(0));

    // build rootfs mount
    let mount_opt = format!(
        "lowerdir={},upperdir={},workdir={}",
        absolute(&overlay_lower_dir)?.display(),
        &abs_rootfs_base_dir.display(),
        absolute(&overlay_work_dir)?.display(),
    );
    sys_mount::Mount::builder()
        .fstype("overlay")
        .data(&mount_opt)
        .mount(&rootfs_base_dir, &mergedfs_dir)
        .context("failed to mount overlayfs")?;
    // TODO: make mount temporary?
    // mount_res.into_unmount_drop(UnmountFlags::DETACH);

    // build container mount

    // container dir preparation
    // TODO: readonly mount ?
    let container_mount_path =
        absolute(mergedfs_dir.join(&args.container_mount_path.trim_start_matches("/")))?;
    create_dir_all(&container_mount_path)?;
    sys_mount::Mount::builder()
        .flags(MountFlags::BIND)
        .mount(&container_info.merged_dir, &container_mount_path)
        .context("failed to mount container fs")?;

    // prepare init script
    {
        let init_script_content = include_str!("init.sh");
        let mut init_script_file = File::create(mergedfs_dir.join("init.sh"))?;
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
            if args.cache {
                let cache_path = cache_dir.join(args.image_cache_filename());
                println!("saving work cache to: {}", cache_path.display());
                let f = File::create(cache_path)?;
                let mut archive = tar::Builder::new(f);
                archive.follow_symlinks(false);
                archive
                    .append_dir_all("", &abs_rootfs_base_dir)
                    .context(format!(
                        "failed to append dir all, path: {}",
                        &abs_rootfs_base_dir.display(),
                    ))?;
            }
            // unmount
            sys_mount::unmount(&container_mount_path, UnmountFlags::DETACH)?;
            if args.unmount_on_exit {
                sys_mount::unmount(&mergedfs_dir, UnmountFlags::DETACH)?;
            }
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
            set_current_dir(&mergedfs_dir)?;
            let exec_res = unsafe {
                let cmd = CString::new("/usr/bin/bash").expect("CString::new failed");
                let arg1 = CString::new("--init-file").expect("CString::new failed");
                let arg2 = CString::new("init.sh").expect("CString::new failed");
                let args = [
                    arg1.as_ptr(),
                    arg2.as_ptr(),
                    std::ptr::null(), // Null-terminated argument list
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
