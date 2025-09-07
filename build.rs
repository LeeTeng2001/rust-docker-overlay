// build.rs

use std::env;

use vergen_git2::{Emitter, Git2Builder};

fn main() {
    let git2 = Git2Builder::all_git().unwrap();
    Emitter::default()
        .add_instructions(&git2)
        .unwrap()
        .emit()
        .unwrap();

    // version
    let version = env::var("PROGRAM_VERSION").unwrap_or("dev".to_string());
    println!("cargo:rustc-env=PROGRAM_VERSION={}", version);
}
