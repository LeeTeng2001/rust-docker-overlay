use clap::Parser;

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
pub struct Args {
    /// PID of the process to get namespace info
    #[arg(short, long)]
    pub pid: u32,

    /// pull image
    #[arg(long, default_value_t = false)]
    pub pull: bool,

    // workdir
    #[arg(short, long, default_value = "tmpfs")]
    pub workdir: String,

    /// overlay image
    #[arg(short, long, default_value_t = String::from("ubuntu:latest"))]
    pub overlay_image: String,
}
