use std::collections::HashMap;

use serde::{self, Deserialize, Serialize};

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
