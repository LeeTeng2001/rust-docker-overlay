use anyhow::{Ok, Result};
use libc::c_int;

pub fn enter_namespace(pid: i32, ns_flags: c_int) -> Result<()> {
    println!("entering target process namespace",);
    let pidfd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) };
    if pidfd == -1 {
        println!("pidfd_open failed: {}", pidfd);
        return Err(anyhow::anyhow!("pidfd_open failed"));
    }
    let err_no = unsafe { libc::setns(pidfd as i32, ns_flags) };
    if err_no != 0 {
        println!("setns failed: {}", err_no);
        return Err(anyhow::anyhow!("setns failed"));
    }
    let close_res = unsafe { libc::close(pidfd as i32) };
    if close_res != 0 {
        println!("close pidfd failed: {}", close_res);
        return Err(anyhow::anyhow!("close pidfd failed"));
    }

    Ok(())
}
