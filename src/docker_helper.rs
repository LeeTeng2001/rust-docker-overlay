use std::collections::HashMap;

use serde::{self, Deserialize, Serialize};
use serde_json::Value;

#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all = "PascalCase")]
pub struct DockerManifest {
    config: String,
    repo_tags: Vec<String>,
    layers: Vec<String>,

    #[serde(flatten)]
    extra: HashMap<String, Value>,
}
