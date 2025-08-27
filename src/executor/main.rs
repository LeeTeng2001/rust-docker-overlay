use anyhow::Result;
use clap::Parser;
use std::ffi::CString;
use std::io::{self, Read, Write};
use std::os::unix;
use std::process::{Command, Stdio};
use std::ptr::null;

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
pub struct Args {
    // rootfs
    #[arg(short, long, default_value = "tmpfs")]
    pub rootfs: String,

    // init program
    #[arg(short, long, default_value = "/bin/bash")]
    pub init_program: String,
}

fn run_bash_interactive() -> Result<()> {
    let mut child = Command::new("/bin/bash")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

    // Get handles to stdin/stdout
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = child.stdout.take().unwrap();
    let mut stderr = child.stderr.take().unwrap();

    // Pipe user input to child process
    let user_input_handle = std::thread::spawn(move || {
        let mut user_input = String::new();
        while let Ok(n) = io::stdin().read_line(&mut user_input) {
            if n == 0 {
                break;
            } // EOF
            stdin.write_all(user_input.as_bytes()).unwrap();
            user_input.clear();
        }
    });

    // Pipe child output to user
    let output_handle = std::thread::spawn(move || {
        let mut buffer = [0; 1024];
        loop {
            match stdout.read(&mut buffer) {
                Ok(0) => break, // EOF
                Ok(n) => {
                    io::stdout().write_all(&buffer[..n]).unwrap();
                    io::stdout().flush().unwrap();
                }
                Err(_) => break,
            }
        }
    });

    // Pipe child stderr to user
    let error_handle = std::thread::spawn(move || {
        let mut buffer = [0; 1024];
        loop {
            match stderr.read(&mut buffer) {
                Ok(0) => break,
                Ok(n) => {
                    io::stderr().write_all(&buffer[..n]).unwrap();
                    io::stderr().flush().unwrap();
                }
                Err(_) => break,
            }
        }
    });

    user_input_handle.join().unwrap();
    output_handle.join().unwrap();
    error_handle.join().unwrap();

    child.wait()?;
    Ok(())
}

fn main() -> Result<()> {
    let args = Args::try_parse()?;

    // chroot to new root
    if args.rootfs != "" {
        let err_no = unsafe {
            libc::mount(
                CString::new(args.rootfs).unwrap().as_ptr(),
                CString::new("/").unwrap().as_ptr(),
                null(),
                libc::MS_BIND | libc::MS_PRIVATE,
                null(),
            )
        };
        if err_no != 0 {
            println!("mount failed: {}", err_no);
            return Err(anyhow::anyhow!("mount failed"));
        }
        unix::fs::chroot("/")?;
        std::env::set_current_dir("/")?;
    }

    run_bash_interactive()?;

    Ok(())
}
