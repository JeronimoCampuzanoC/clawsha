use uuid::Uuid;

use super::Storage;
use super::fs_util::read_dir_if_exists;
use crate::error::StorageError;

/// A finished block found on disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlockInfo {
    pub block_id: Uuid,
    pub size: u64,
}

impl Storage {
    /// Lists every complete block on disk (temp files are never included).
    pub fn list_blocks(&self) -> Result<Vec<BlockInfo>, StorageError> {
        let mut found = Vec::new();

        let Some(outer_entries) = read_dir_if_exists(&self.data_dir.join("blocks"))? else {
            return Ok(found);
        };

        for outer in outer_entries {
            let outer_path = outer?.path();
            if !outer_path.is_dir() {
                continue;
            }
            let Some(inner_entries) = read_dir_if_exists(&outer_path)? else {
                continue;
            };

            for inner in inner_entries {
                let inner_path = inner?.path();
                if !inner_path.is_dir() {
                    continue;
                }
                let Some(block_entries) = read_dir_if_exists(&inner_path)? else {
                    continue;
                };

                for entry in block_entries {
                    let entry = entry?;

                    // Only accept names in the exact form block_path produces.
                    let name = entry.file_name();
                    let Some(name) = name.to_str() else { continue };
                    let Ok(id) = Uuid::parse_str(name) else {
                        continue;
                    };
                    if id.hyphenated().to_string() != name {
                        continue;
                    }

                    // The block may be deleted while we are walking.
                    let meta = match entry.metadata() {
                        Ok(meta) => meta,
                        Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                        Err(e) => return Err(e.into()),
                    };
                    if meta.is_file() {
                        found.push(BlockInfo {
                            block_id: id,
                            size: meta.len(),
                        });
                    }
                }
            }
        }

        Ok(found)
    }
}
