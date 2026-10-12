use std::fs::File;
use std::io::{Seek, SeekFrom};

use super::Storage;
use crate::error::StorageError;

impl Storage {
    // Opens a block for reading, positioned at `offset`.
    pub fn open_block(&self, block_id: &str, offset: u64) -> Result<File, StorageError> {
        let path = self.block_path(block_id)?;

        let mut file = match File::open(&path) {
            Ok(file) => file,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(StorageError::NotFound);
            }
            Err(e) => return Err(e.into()),
        };

        let size = file.metadata()?.len();
        if offset > size {
            return Err(StorageError::OffsetOutOfRange { offset, size });
        }

        file.seek(SeekFrom::Start(offset))?;
        Ok(file)
    }
}
