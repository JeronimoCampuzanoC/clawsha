use std::fs;
use std::time::Duration;

use sha2::{Digest, Sha256};

use crate::error::StorageError;
use crate::scrubber::{ScrubReport, scrub_once};
use crate::storage::{Storage, meta_path_for};

const GOOD: &str = "3fa85f64-5717-4562-b3fc-2c963f66afa6";
const BAD: &str = "9b2f6a10-1c3d-4e8a-8f0b-5d7e2a4c6b81";
const NO_WAIT: Duration = Duration::ZERO;

fn put(storage: &Storage, id: &str, data: &[u8]) {
    let mut hash = [0u8; 32];
    hash.copy_from_slice(&Sha256::digest(data));
    let mut writer = storage.begin_write(id, data.len() as u64, hash).unwrap();
    writer.write_chunk(data).unwrap();
    writer.finish().unwrap();
}

#[test]
fn an_empty_disk_is_a_clean_pass() {
    let dir = tempfile::tempdir().unwrap();
    let storage = Storage::new(dir.path().to_path_buf());

    assert_eq!(
        scrub_once(&storage, NO_WAIT).unwrap(),
        ScrubReport::default()
    );
}

#[test]
fn intact_blocks_are_left_alone() {
    let dir = tempfile::tempdir().unwrap();
    let storage = Storage::new(dir.path().to_path_buf());
    put(&storage, GOOD, b"hello");
    put(&storage, BAD, b"world");

    let report = scrub_once(&storage, NO_WAIT).unwrap();

    assert_eq!(report.checked, 2);
    assert!(report.quarantined.is_empty());
    assert!(storage.block_path(GOOD).unwrap().exists());
    assert!(!dir.path().join("quarantine").exists());
}

#[test]
fn a_corrupt_block_is_quarantined_and_the_good_one_stays() {
    let dir = tempfile::tempdir().unwrap();
    let storage = Storage::new(dir.path().to_path_buf());
    put(&storage, GOOD, b"hello");
    put(&storage, BAD, b"world");
    let used_before = storage.used_bytes();
    storage.take_changes();

    // Same length, different bytes: only the hash can tell.
    fs::write(storage.block_path(BAD).unwrap(), b"wurld").unwrap();

    let report = scrub_once(&storage, NO_WAIT).unwrap();

    assert_eq!(report.checked, 1);
    assert_eq!(report.quarantined, vec![BAD.to_string()]);

    // The bad block is out of blocks/ and kept, with its checksum file.
    assert!(!storage.block_path(BAD).unwrap().exists());
    let kept = dir.path().join("quarantine").join(BAD);
    assert_eq!(fs::read(&kept).unwrap(), b"wurld");
    assert!(meta_path_for(&kept).exists());
    assert!(storage.block_path(GOOD).unwrap().exists());

    // The block report would no longer list it, and the heartbeat says so.
    let listed: Vec<String> = storage
        .list_blocks()
        .unwrap()
        .iter()
        .map(|b| b.block_id.hyphenated().to_string())
        .collect();
    assert_eq!(listed, vec![GOOD.to_string()]);
    assert_eq!(storage.take_changes().removed_blocks, vec![BAD.to_string()]);
    assert_eq!(storage.used_bytes(), used_before - 5);
}

#[test]
fn a_block_without_its_checksum_file_is_quarantined() {
    let dir = tempfile::tempdir().unwrap();
    let storage = Storage::new(dir.path().to_path_buf());
    put(&storage, BAD, b"world");
    fs::remove_file(meta_path_for(&storage.block_path(BAD).unwrap())).unwrap();

    let report = scrub_once(&storage, NO_WAIT).unwrap();

    assert_eq!(report.quarantined, vec![BAD.to_string()]);
    assert!(dir.path().join("quarantine").join(BAD).exists());
}

#[test]
fn a_quarantined_block_is_not_found_again_on_the_next_pass() {
    let dir = tempfile::tempdir().unwrap();
    let storage = Storage::new(dir.path().to_path_buf());
    put(&storage, BAD, b"world");
    fs::write(storage.block_path(BAD).unwrap(), b"wurld").unwrap();

    assert_eq!(scrub_once(&storage, NO_WAIT).unwrap().quarantined.len(), 1);
    assert_eq!(
        scrub_once(&storage, NO_WAIT).unwrap(),
        ScrubReport::default()
    );
}

#[test]
fn quarantining_a_block_that_is_not_there_is_not_found() {
    let dir = tempfile::tempdir().unwrap();
    let storage = Storage::new(dir.path().to_path_buf());

    assert!(matches!(
        storage.quarantine_block(GOOD).unwrap_err(),
        StorageError::NotFound
    ));
}
