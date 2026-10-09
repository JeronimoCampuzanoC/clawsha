use crate::error::StorageError;
use crate::storage::{BlockInfo, Storage};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::Read;
use std::path::PathBuf;
use uuid::Uuid;

const ID: &str = "3fa85f64-5717-4562-b3fc-2c963f66afa6";

#[test]
fn block_path_has_expected_layout() {
    let storage = Storage::new(PathBuf::from("/data"));
    let path = storage.block_path(ID).unwrap();
    assert_eq!(path, PathBuf::from("/data/blocks/3f/a8").join(ID));
}

#[test]
fn uppercase_id_gives_same_path() {
    let storage = Storage::new(PathBuf::from("/data"));
    let lower = storage.block_path(ID).unwrap();
    let upper = storage.block_path(&ID.to_uppercase()).unwrap();
    assert_eq!(lower, upper);
}

#[test]
fn invalid_ids_are_rejected() {
    let storage = Storage::new(PathBuf::from("/data"));
    for bad in ["../../etc/passwd", "", "hello"] {
        assert!(storage.block_path(bad).is_err(), "should reject {bad:?}");
    }
}

fn sha(data: &[u8]) -> [u8; 32] {
    let mut out = [0u8; 32];
    out.copy_from_slice(&Sha256::digest(data));
    out
}

/// Writes a whole block, splitting it into the given chunks.
fn put(storage: &Storage, id: &str, chunks: &[&[u8]]) -> Result<(), StorageError> {
    let all: Vec<u8> = chunks.concat();
    let mut writer = storage.begin_write(id, all.len() as u64, sha(&all))?;
    for chunk in chunks {
        writer.write_chunk(chunk)?;
    }
    writer.finish()
}

#[test]
fn written_block_can_be_read_back() {
    let dir = tempfile::tempdir().unwrap();
    let storage = Storage::new(dir.path().to_path_buf());

    put(&storage, ID, &[b"hello ", b"distributed ", b"world"]).unwrap();

    let stored = fs::read(storage.block_path(ID).unwrap()).unwrap();
    assert_eq!(stored, b"hello distributed world");
}

#[test]
fn wrong_hash_is_rejected_and_leaves_no_trace() {
    let dir = tempfile::tempdir().unwrap();
    let storage = Storage::new(dir.path().to_path_buf());

    let mut writer = storage.begin_write(ID, 5, sha(b"other")).unwrap();
    writer.write_chunk(b"hello").unwrap();
    let err = writer.finish().unwrap_err();

    assert!(matches!(err, StorageError::HashMismatch));
    assert!(!storage.block_path(ID).unwrap().exists());
    assert_eq!(fs::read_dir(dir.path().join("tmp")).unwrap().count(), 0);
}

#[test]
fn too_few_bytes_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let storage = Storage::new(dir.path().to_path_buf());

    let mut writer = storage.begin_write(ID, 10, sha(b"0123456789")).unwrap();
    writer.write_chunk(b"01234").unwrap();
    let err = writer.finish().unwrap_err();

    assert!(matches!(err, StorageError::SizeMismatch { .. }));
}

#[test]
fn too_many_bytes_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let storage = Storage::new(dir.path().to_path_buf());

    let mut writer = storage.begin_write(ID, 3, sha(b"abc")).unwrap();
    let err = writer.write_chunk(b"abcdef").unwrap_err();

    assert!(matches!(err, StorageError::SizeMismatch { .. }));
}

#[test]
fn duplicate_block_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let storage = Storage::new(dir.path().to_path_buf());

    put(&storage, ID, &[b"data"]).unwrap();

    let second = storage.begin_write(ID, 4, sha(b"data"));
    assert!(matches!(second, Err(StorageError::AlreadyExists)));
}

fn read_rest(mut file: fs::File) -> Vec<u8> {
    let mut out = Vec::new();
    file.read_to_end(&mut out).unwrap();
    out
}

#[test]
fn open_block_reads_from_the_start() {
    let dir = tempfile::tempdir().unwrap();
    let storage = Storage::new(dir.path().to_path_buf());
    put(&storage, ID, &[b"hello distributed world"]).unwrap();

    let file = storage.open_block(ID, 0).unwrap();
    assert_eq!(read_rest(file), b"hello distributed world");
}

#[test]
fn open_block_honors_the_offset() {
    let dir = tempfile::tempdir().unwrap();
    let storage = Storage::new(dir.path().to_path_buf());
    put(&storage, ID, &[b"hello distributed world"]).unwrap();

    let file = storage.open_block(ID, 6).unwrap();
    assert_eq!(read_rest(file), b"distributed world");
}

#[test]
fn offset_equal_to_size_gives_empty_read() {
    let dir = tempfile::tempdir().unwrap();
    let storage = Storage::new(dir.path().to_path_buf());
    put(&storage, ID, &[b"data"]).unwrap();

    let file = storage.open_block(ID, 4).unwrap();
    assert!(read_rest(file).is_empty());
}

#[test]
fn offset_past_the_end_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let storage = Storage::new(dir.path().to_path_buf());
    put(&storage, ID, &[b"data"]).unwrap();

    let err = storage.open_block(ID, 5).unwrap_err();
    assert!(matches!(err, StorageError::OffsetOutOfRange { .. }));
}

#[test]
fn missing_block_is_not_found() {
    let dir = tempfile::tempdir().unwrap();
    let storage = Storage::new(dir.path().to_path_buf());

    let err = storage.open_block(ID, 0).unwrap_err();
    assert!(matches!(err, StorageError::NotFound));
}

#[test]
fn invalid_id_is_rejected_on_read() {
    let dir = tempfile::tempdir().unwrap();
    let storage = Storage::new(dir.path().to_path_buf());

    let err = storage.open_block("../../etc/passwd", 0).unwrap_err();
    assert!(matches!(err, StorageError::InvalidBlockId(_)));
}

#[test]
fn delete_leaves_shard_directories_in_place() {
    let dir = tempfile::tempdir().unwrap();
    let storage = Storage::new(dir.path().to_path_buf());
    put(&storage, ID, &[b"data"]).unwrap();

    storage.delete_block(ID).unwrap();

    assert!(dir.path().join("blocks/3f/a8").exists());
}

#[test]
fn prune_removes_empty_shard_directories() {
    let dir = tempfile::tempdir().unwrap();
    let storage = Storage::new(dir.path().to_path_buf());
    put(&storage, ID, &[b"data"]).unwrap();
    storage.delete_block(ID).unwrap();

    let removed = storage.prune_empty_dirs().unwrap();

    assert_eq!(removed, 2);
    assert!(!dir.path().join("blocks/3f").exists());
    assert!(dir.path().join("blocks").exists());
}

#[test]
fn prune_keeps_directories_that_still_have_blocks() {
    const OTHER: &str = "3fa8aaaa-5717-4562-b3fc-2c963f66afa6";
    let dir = tempfile::tempdir().unwrap();
    let storage = Storage::new(dir.path().to_path_buf());
    put(&storage, ID, &[b"one"]).unwrap();
    put(&storage, OTHER, &[b"two"]).unwrap();
    storage.delete_block(ID).unwrap();

    let removed = storage.prune_empty_dirs().unwrap();

    assert_eq!(removed, 0);
    assert!(storage.block_path(OTHER).unwrap().exists());
}

#[test]
fn prune_without_a_blocks_directory_is_a_noop() {
    let dir = tempfile::tempdir().unwrap();
    let storage = Storage::new(dir.path().to_path_buf());

    assert_eq!(storage.prune_empty_dirs().unwrap(), 0);
}

#[test]
fn block_can_be_written_after_prune() {
    let dir = tempfile::tempdir().unwrap();
    let storage = Storage::new(dir.path().to_path_buf());
    put(&storage, ID, &[b"first"]).unwrap();
    storage.delete_block(ID).unwrap();
    storage.prune_empty_dirs().unwrap();

    put(&storage, ID, &[b"second"]).unwrap();

    let stored = fs::read(storage.block_path(ID).unwrap()).unwrap();
    assert_eq!(stored, b"second");
}

#[test]
fn list_on_empty_storage_is_empty() {
    let dir = tempfile::tempdir().unwrap();
    let storage = Storage::new(dir.path().to_path_buf());

    assert!(storage.list_blocks().unwrap().is_empty());
}

#[test]
fn list_returns_every_written_block_with_its_size() {
    const OTHER: &str = "7b04e9d0-1111-4562-b3fc-2c963f66afa6";
    let dir = tempfile::tempdir().unwrap();
    let storage = Storage::new(dir.path().to_path_buf());
    put(&storage, ID, &[b"hello"]).unwrap();
    put(&storage, OTHER, &[b"distributed"]).unwrap();

    let mut listed = storage.list_blocks().unwrap();
    listed.sort_by_key(|b| b.block_id);

    let mut expected = vec![
        BlockInfo {
            block_id: Uuid::parse_str(ID).unwrap(),
            size: 5,
        },
        BlockInfo {
            block_id: Uuid::parse_str(OTHER).unwrap(),
            size: 11,
        },
    ];
    expected.sort_by_key(|b| b.block_id);
    assert_eq!(listed, expected);
}

#[test]
fn list_skips_deleted_blocks() {
    let dir = tempfile::tempdir().unwrap();
    let storage = Storage::new(dir.path().to_path_buf());
    put(&storage, ID, &[b"data"]).unwrap();
    storage.delete_block(ID).unwrap();

    assert!(storage.list_blocks().unwrap().is_empty());
}

#[test]
fn list_ignores_files_that_are_not_blocks() {
    let dir = tempfile::tempdir().unwrap();
    let storage = Storage::new(dir.path().to_path_buf());
    put(&storage, ID, &[b"data"]).unwrap();
    fs::write(dir.path().join("blocks/3f/a8/notes.txt"), b"stray").unwrap();

    let listed = storage.list_blocks().unwrap();
    assert_eq!(listed.len(), 1);
}

#[test]
fn list_never_includes_in_progress_writes() {
    let dir = tempfile::tempdir().unwrap();
    let storage = Storage::new(dir.path().to_path_buf());

    let mut writer = storage.begin_write(ID, 4, sha(b"data")).unwrap();
    writer.write_chunk(b"da").unwrap();

    assert!(storage.list_blocks().unwrap().is_empty());
}

#[test]
fn list_works_after_prune() {
    let dir = tempfile::tempdir().unwrap();
    let storage = Storage::new(dir.path().to_path_buf());
    put(&storage, ID, &[b"data"]).unwrap();
    storage.delete_block(ID).unwrap();
    storage.prune_empty_dirs().unwrap();

    assert!(storage.list_blocks().unwrap().is_empty());
}

    #[test]
    fn block_larger_than_quota_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let storage = Storage::new(dir.path().to_path_buf()).with_quota(10);

        let result = storage.begin_write(ID, 11, sha(b"01234567890"));

        assert!(matches!(result, Err(StorageError::QuotaExceeded)));
        assert_eq!(storage.used_bytes(), 0);
        assert_eq!(fs::read_dir(dir.path().join("tmp")).unwrap().count(), 0);
    }

    #[test]
    fn used_bytes_grows_after_a_write() {
        let dir = tempfile::tempdir().unwrap();
        let storage = Storage::new(dir.path().to_path_buf()).with_quota(100);

        put(&storage, ID, &[b"hello"]).unwrap();

        assert_eq!(storage.used_bytes(), 5);
    }

    #[test]
    fn abandoned_writer_gives_the_space_back() {
        let dir = tempfile::tempdir().unwrap();
        let storage = Storage::new(dir.path().to_path_buf()).with_quota(100);

        let writer = storage.begin_write(ID, 8, sha(b"12345678")).unwrap();
        assert_eq!(storage.used_bytes(), 8);

        drop(writer);
        assert_eq!(storage.used_bytes(), 0);
    }

    #[test]
    fn failed_finish_gives_the_space_back() {
        let dir = tempfile::tempdir().unwrap();
        let storage = Storage::new(dir.path().to_path_buf()).with_quota(100);

        let mut writer = storage.begin_write(ID, 5, sha(b"other")).unwrap();
        writer.write_chunk(b"hello").unwrap();
        assert!(writer.finish().is_err());

        assert_eq!(storage.used_bytes(), 0);
    }

    #[test]
    fn concurrent_reservations_cannot_exceed_the_quota() {
        const OTHER: &str = "7b04e9d0-1111-4562-b3fc-2c963f66afa6";
        let dir = tempfile::tempdir().unwrap();
        let storage = Storage::new(dir.path().to_path_buf()).with_quota(10);

        let first = storage.begin_write(ID, 6, sha(b"abcdef")).unwrap();
        let second = storage.begin_write(OTHER, 6, sha(b"ghijkl"));

        assert!(matches!(second, Err(StorageError::QuotaExceeded)));
        drop(first);
    }

    #[test]
    fn delete_frees_quota() {
        const OTHER: &str = "7b04e9d0-1111-4562-b3fc-2c963f66afa6";
        let dir = tempfile::tempdir().unwrap();
        let storage = Storage::new(dir.path().to_path_buf()).with_quota(10);

        put(&storage, ID, &[b"12345678"]).unwrap();
        assert!(matches!(
            put(&storage, OTHER, &[b"abcdefgh"]),
            Err(StorageError::QuotaExceeded)
        ));

        storage.delete_block(ID).unwrap();
        put(&storage, OTHER, &[b"abcdefgh"]).unwrap();
        assert_eq!(storage.used_bytes(), 8);
    }

    #[test]
    fn open_counts_existing_blocks_and_clears_leftovers() {
        let dir = tempfile::tempdir().unwrap();
        let first = Storage::new(dir.path().to_path_buf());
        put(&first, ID, &[b"hello"]).unwrap();

        // Simulate a crash that left a temp file behind.
        fs::create_dir_all(dir.path().join("tmp")).unwrap();
        fs::write(dir.path().join("tmp/orphan.tmp"), b"junk").unwrap();

        let reopened = Storage::open(dir.path().to_path_buf(), 100).unwrap();

        assert_eq!(reopened.used_bytes(), 5);
        assert_eq!(fs::read_dir(dir.path().join("tmp")).unwrap().count(), 0);
    }
