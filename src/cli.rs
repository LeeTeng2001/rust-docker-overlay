use clap::Parser;

#[derive(Parser, Debug)]
#[command(disable_version_flag = true, about, long_about = None)]
pub struct VerArgs {
    // version
    #[arg(short, long, default_value_t = false)]
    pub version: bool,
}

#[derive(Parser, Debug)]
#[command(disable_version_flag = true, about, long_about = None)]
pub struct Args {
    /// Docker container ID
    #[arg()]
    pub id: String,

    /// force repull image
    #[arg(long, default_value_t = false)]
    pub pull: bool,

    /// workdir
    #[arg(short, long, default_value = "/var/lib/rustnsoverlay/work")]
    pub workdir: String,

    /// image to act as rootfs
    #[arg(long, default_value_t = String::from("debian:12"))]
    pub image: String,

    /// reuse image cache
    #[arg(long, default_value_t = true)]
    pub cache: bool,

    /// work cache directory
    #[arg(long, default_value_t = String::from("/var/cache/rustnsoverlay"))]
    pub cache_dir: String,

    /// container fs mount path inside debug rootfs
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
