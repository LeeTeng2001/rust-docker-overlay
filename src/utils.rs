use anyhow::{Ok, Result};
use std::{
    fs::{File, Permissions, create_dir_all, remove_file, set_permissions},
    io::{Read, copy},
    os::unix::fs::{PermissionsExt, symlink},
    path::Path,
};
use tar::Archive;

pub fn extract_archive(reader: &mut dyn Read, dst_dir: &Path) -> Result<()> {
    let mut tar_archive = Archive::new(reader);
    for entry in tar_archive.entries().unwrap() {
        let mut tar_file = entry?;
        let path = tar_file.path()?;
        let dst_path = dst_dir.join(&path);

        match tar_file.header().entry_type() {
            tar::EntryType::Regular => {
                let mut dst_file = File::create(dst_path)?;
                dst_file.set_permissions(Permissions::from_mode(tar_file.header().mode()?))?;
                copy(&mut tar_file, &mut dst_file)?;
            }
            tar::EntryType::Directory => {
                create_dir_all(&dst_path)?;
                set_permissions(dst_path, Permissions::from_mode(tar_file.header().mode()?))?;
            }
            tar::EntryType::Symlink | tar::EntryType::Link => {
                let link = tar_file
                    .header()
                    .link_name()?
                    .unwrap()
                    .to_string_lossy()
                    .to_string();
                let original_path = Path::new(&link);
                if dst_path.exists() {
                    println!("overriding symlink: {}", dst_path.display());
                    remove_file(&dst_path)?;
                }
                symlink(original_path, &dst_path).map_err(|e| {
                    anyhow::anyhow!(
                        "failed to symlink: {}, original path: {}, file {}",
                        e,
                        original_path.display(),
                        dst_path.display()
                    )
                })?;
            }
            _ => println!(
                "warning: skipping entry type: {:?} for {}",
                tar_file.header().entry_type(),
                dst_path.display()
            ),
        }
    }

    Ok(())
}
