#[derive(Serialize, Deserialize)]
pub struct DockerManifest {
    #[serde(rename = "Config")]
    config_path: String,
    #[serde(rename = "RepoTags")]
    repo_tags: Vec<String>,
    #[serde(rename = "Layers")]
    layer_tar_paths: Vec<String>,
}
