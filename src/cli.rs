use clap::Parser;

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
pub struct Args {
    /// Docker container ID
    #[arg(short, long)]
    pub id: String,

    /// pull image
    #[arg(long, default_value_t = false)]
    pub pull: bool,

    // workdir
    #[arg(short, long, default_value = "tmpfs")]
    pub workdir: String,

    /// image to act as rootfs
    #[arg(long, default_value_t = String::from("ubuntu:latest"))]
    pub image: String,

    /// reuse image cache
    #[arg(long, default_value_t = false)]
    pub cache: bool,

    /// work cache directory
    #[arg(long, default_value_t = String::from("/var/cache/rustnsoverlay"))]
    pub cache_dir: String,

    // mount container fs instead of overlaying on top of it, this is the mount path inside container
    #[arg(long, default_value_t = String::from("/mnt/container"))]
    pub container_mount_path: String,

    /// unmount mergedfs on exit
    #[arg(long, default_value_t = true)]
    pub unmount_on_exit: bool,
}

impl Args {
    pub fn image_cache_filename(&self) -> String {
        let (image_name, tag) = self
            .image
            .split_once(":")
            .unwrap_or((&self.image, "latest"));
        let image_name = image_name.replace("/", "_");
        return format!("{}_{}.tar", image_name, tag);
    }
}
