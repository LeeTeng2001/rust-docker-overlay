use anyhow::{Context, Result};
use dockworker::Docker;
use dockworker::response::Response;
use futures::stream::StreamExt;
use futures::stream::TryStreamExt;
use oci_spec::image::MediaType;
use std::fs::{File, Permissions, set_permissions};
use std::io::Read;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::{collections::HashMap, path::Path};
use tar::Archive;

use serde::{self, Deserialize, Serialize};

use crate::utils;

#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct DockerManifestLayerSource {
    pub media_type: String,
    pub size: u64,
    pub digest: String,
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all = "PascalCase")]
pub struct DockerManifest {
    pub config: String,
    pub repo_tags: Vec<String>,
    pub layers: Vec<String>,
    pub layer_sources: HashMap<String, DockerManifestLayerSource>,
}

#[derive(Debug, Default)]
pub struct ContainerInfo {
    pub pid: u64,
    pub merged_dir: String,
}

pub struct DockerHelper {
    docker: Docker,
}

impl DockerHelper {
    pub fn new() -> Result<Self> {
        let docker = Docker::connect_with_defaults()?;
        Ok(DockerHelper { docker })
    }

    pub async fn get_container_info(&self, container_id: &str) -> Result<ContainerInfo> {
        let container_info = self
            .docker
            .container_info(container_id)
            .await
            .context("inspect container")?;

        if !container_info.State.Running {
            return Err(anyhow::anyhow!("container is not running"));
        }
        if container_info.Driver != "overlay2" {
            return Err(anyhow::anyhow!(
                "only overlay2 driver is supported, found: {}",
                container_info.Driver
            ));
        }

        let pid = container_info.State.Pid as u64;
        let merged_dir = container_info
            .GraphDriver
            .Data
            .get("MergedDir")
            .context("expect MergedDir in GraphDriver setting")?;

        Ok(ContainerInfo {
            pid,
            merged_dir: merged_dir.to_string(),
        })
    }

    pub async fn export_overlay_image(
        &self,
        image: &str,
        tmp_dir: &Path,
        export_dir: &Path,
        pull: bool,
    ) -> Result<()> {
        if pull {
            println!("pulling overlay image: {}", image);
            let (image_name, tag) = image.split_once(":").unwrap_or((image, "latest"));
            let mut download_stats = self.docker.create_image(image_name, tag).await?;
            while let Some(Ok(stat)) = download_stats.next().await {
                match stat {
                    Response::Status(status) => {
                        println!("{}", status.status);
                    }
                    Response::Progress(progress) => {
                        if let Some(p) = progress.progress {
                            println!("{}", p);
                        } else {
                            println!("{}", progress.status);
                        }
                    }
                    Response::Error(err) => {
                        println!("error: {err:?}");
                    }
                    _ => {}
                }
            }
        }

        // TODO: optimsie with tar stream decompression in memory?
        let tar_path = tmp_dir.join("temp.tar");
        {
            println!("exporting overlay image: {}", image);
            let mut tmp_file = tokio::fs::File::create(&tar_path).await?;
            let img_res = self
                .docker
                .export_image(image)
                .await
                .context("unable to export image")?;
            let mut res = tokio_util::io::StreamReader::new(
                img_res.map_err(|err| std::io::Error::new(std::io::ErrorKind::Other, err)),
            );
            tokio::io::copy(&mut res, &mut tmp_file).await.unwrap();
        }

        // manifest
        let mut manifest: Vec<DockerManifest> = Vec::new();
        let mut blob: HashMap<String, Vec<u8>> = HashMap::new();
        println!("extracting raw overlay image to memory: {}", image);
        let mut tar_archive = Archive::new(File::open(&tar_path)?);
        for file in tar_archive.entries().unwrap() {
            let mut tar_file = file?;
            let path = tar_file.path()?;
            let dst_path = tmp_dir.join(&path);

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

            // extract archive
            let mut blob_reader = std::io::Cursor::new(layer_blob);
            utils::extract_archive(&mut blob_reader, &export_dir)?;
        }

        Ok(())
    }
}
