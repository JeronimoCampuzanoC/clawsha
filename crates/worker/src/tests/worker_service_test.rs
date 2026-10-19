use std::sync::Arc;

use sha2::{Digest, Sha256};
use tokio_stream::wrappers::TcpListenerStream;
use tokio_stream::{Stream, StreamExt};
use tonic::{Code, Request, Status};

use crate::error::StorageError;
use crate::pb::block_frame::Kind;
use crate::pb::worker_service_server::{WorkerService, WorkerServiceServer};
use crate::pb::{
    BlockFrame, BlockHeader, DataFrame, DeleteBlockRequest, GetBlockRequest, ReplicateBlockRequest,
};
use crate::service::{WorkerSvc, blocking, receive_block};
use crate::storage::Storage;

const ID: &str = "3fa85f64-5717-4562-b3fc-2c963f66afa6";

#[tokio::test]
async fn storage_can_be_called_from_async_code() {
    let dir = tempfile::tempdir().unwrap();
    let storage = Arc::new(Storage::new(dir.path().to_path_buf()));

    let storage_for_task = Arc::clone(&storage);
    let result = tokio::task::spawn_blocking(move || storage_for_task.verify_block(ID))
        .await
        .unwrap();

    assert!(matches!(result, Err(StorageError::NotFound)));
}

#[tokio::test]
async fn blocking_turns_storage_errors_into_grpc_status() {
    let dir = tempfile::tempdir().unwrap();
    let storage = Arc::new(Storage::new(dir.path().to_path_buf()));

    let result = blocking(move || storage.verify_block(ID)).await;

    let status = result.unwrap_err();
    assert_eq!(status.code(), tonic::Code::NotFound);
}

const DEFAULT_FRAME_SIZE: usize = 256 * 1024;

/// The temp dir is returned too: it must stay alive during the whole test.
fn setup() -> (tempfile::TempDir, Arc<Storage>, WorkerSvc) {
    setup_with(DEFAULT_FRAME_SIZE)
}

/// Same as `setup`, but with a custom frame size to make splitting easy to test.
fn setup_with(frame_size: usize) -> (tempfile::TempDir, Arc<Storage>, WorkerSvc) {
    let dir = tempfile::tempdir().unwrap();
    let storage = Arc::new(Storage::new(dir.path().to_path_buf()));
    let svc = WorkerSvc::new(Arc::clone(&storage), frame_size);
    (dir, storage, svc)
}

/// Stores a whole block straight through Storage, no gRPC involved.
fn store_block(storage: &Storage, id: &str, data: &[u8]) {
    let mut sha = [0u8; 32];
    sha.copy_from_slice(&Sha256::digest(data));

    let mut writer = storage.begin_write(id, data.len() as u64, sha).unwrap();
    writer.write_chunk(data).unwrap();
    writer.finish().unwrap();
}

fn get_request(id: &str, offset: u64) -> Request<GetBlockRequest> {
    Request::new(GetBlockRequest {
        block_id: id.to_string(),
        offset,
    })
}

#[tokio::test]
async fn healthy_block_is_accepted() {
    let (_dir, storage, svc) = setup();
    store_block(&storage, ID, b"hello distributed world");

    let result = svc.get_block(get_request(ID, 0)).await;

    assert!(result.is_ok());
}

#[tokio::test]
async fn missing_block_is_not_found() {
    let (_dir, _storage, svc) = setup();

    let status = svc.get_block(get_request(ID, 0)).await.err().unwrap();

    assert_eq!(status.code(), Code::NotFound);
}

#[tokio::test]
async fn invalid_id_is_invalid_argument() {
    let (_dir, _storage, svc) = setup();

    let status = svc
        .get_block(get_request("../../etc/passwd", 0))
        .await
        .err()
        .unwrap();

    assert_eq!(status.code(), Code::InvalidArgument);
}

#[tokio::test]
async fn offset_past_the_end_is_out_of_range() {
    let (_dir, storage, svc) = setup();
    store_block(&storage, ID, b"data");

    let status = svc.get_block(get_request(ID, 5)).await.err().unwrap();

    assert_eq!(status.code(), Code::OutOfRange);
}

#[tokio::test]
async fn corrupted_block_is_data_loss() {
    let (_dir, storage, svc) = setup();
    store_block(&storage, ID, b"hello");
    // Same length, different bytes: only the hash can tell.
    std::fs::write(storage.block_path(ID).unwrap(), b"jello").unwrap();

    let status = svc.get_block(get_request(ID, 0)).await.err().unwrap();

    assert_eq!(status.code(), Code::DataLoss);
}

#[tokio::test]
async fn block_is_split_into_frames_of_the_configured_size() {
    let (_dir, storage, svc) = setup_with(10);
    store_block(&storage, ID, b"0123456789abcdefghijklmno"); // 25 bytes

    let (bytes, frame_sizes) = read_all(&svc, ID, 0).await.unwrap();

    assert_eq!(bytes, b"0123456789abcdefghijklmno");
    assert_eq!(frame_sizes, vec![10, 10, 5]);
}

#[tokio::test]
async fn exact_multiple_of_frame_size_has_no_empty_frame() {
    let (_dir, storage, svc) = setup_with(10);
    store_block(&storage, ID, b"0123456789abcdefghij"); // 20 bytes

    let (_, frame_sizes) = read_all(&svc, ID, 0).await.unwrap();

    assert_eq!(frame_sizes, vec![10, 10]);
}

#[tokio::test]
async fn block_smaller_than_a_frame_is_one_frame() {
    let (_dir, storage, svc) = setup_with(1024);
    store_block(&storage, ID, b"hello");

    let (bytes, frame_sizes) = read_all(&svc, ID, 0).await.unwrap();

    assert_eq!(bytes, b"hello");
    assert_eq!(frame_sizes, vec![5]);
}

#[tokio::test]
async fn offset_skips_the_start_of_the_block() {
    let (_dir, storage, svc) = setup_with(8);
    store_block(&storage, ID, b"hello distributed world");

    let (bytes, frame_sizes) = read_all(&svc, ID, 6).await.unwrap();

    assert_eq!(bytes, b"distributed world"); // 17 bytes
    assert_eq!(frame_sizes, vec![8, 8, 1]);
}

#[tokio::test]
async fn offset_equal_to_size_gives_an_empty_stream() {
    let (_dir, storage, svc) = setup_with(8);
    store_block(&storage, ID, b"data");

    let (bytes, frame_sizes) = read_all(&svc, ID, 4).await.unwrap();

    assert!(bytes.is_empty());
    assert!(frame_sizes.is_empty());
}

#[tokio::test]
async fn default_frame_size_splits_a_600_kib_block_in_three() {
    let (_dir, storage, svc) = setup();
    let data: Vec<u8> = (0..600 * 1024).map(|i| (i % 251) as u8).collect();
    store_block(&storage, ID, &data);

    let (bytes, frame_sizes) = read_all(&svc, ID, 0).await.unwrap();

    assert_eq!(bytes, data);
    assert_eq!(frame_sizes, vec![256 * 1024, 256 * 1024, 88 * 1024]);
}

/// Calls GetBlock and collects everything: (all the bytes, size of each frame).
async fn read_all(svc: &WorkerSvc, id: &str, offset: u64) -> Result<(Vec<u8>, Vec<usize>), Status> {
    let mut stream = svc.get_block(get_request(id, offset)).await?.into_inner();

    let mut bytes = Vec::new();
    let mut frame_sizes = Vec::new();

    while let Some(item) = stream.next().await {
        match item?.kind {
            Some(Kind::Data(frame)) => {
                frame_sizes.push(frame.data.len());
                bytes.extend_from_slice(&frame.data);
            }
            _ => panic!("GetBlock must only send data frames"),
        }
    }
    Ok((bytes, frame_sizes))
}

// ----- DeleteBlock ----------------------------------------------------------

fn delete_request(id: &str, fencing: u64) -> Request<DeleteBlockRequest> {
    Request::new(DeleteBlockRequest {
        block_id: id.to_string(),
        fencing,
    })
}

#[tokio::test]
async fn delete_removes_the_block_and_reports_freed_bytes() {
    let (_dir, storage, svc) = setup();
    store_block(&storage, ID, b"hello world");

    let response = svc
        .delete_block(delete_request(ID, 0))
        .await
        .unwrap()
        .into_inner();

    assert_eq!(response.freed_bytes, 11);
    assert!(!storage.block_path(ID).unwrap().exists());
    assert_eq!(storage.used_bytes(), 0);
}

#[tokio::test]
async fn delete_of_a_missing_block_is_ok_and_frees_nothing() {
    let (_dir, _storage, svc) = setup();

    let response = svc
        .delete_block(delete_request(ID, 0))
        .await
        .unwrap()
        .into_inner();

    assert_eq!(response.freed_bytes, 0);
}

#[tokio::test]
async fn delete_works_on_a_corrupted_block() {
    let (_dir, storage, svc) = setup();
    store_block(&storage, ID, b"hello");
    std::fs::write(storage.block_path(ID).unwrap(), b"jello").unwrap();

    svc.delete_block(delete_request(ID, 0)).await.unwrap();

    assert!(!storage.block_path(ID).unwrap().exists());
}

#[tokio::test]
async fn delete_rejects_an_invalid_id() {
    let (_dir, _storage, svc) = setup();

    let status = svc
        .delete_block(delete_request("../../etc/passwd", 0))
        .await
        .unwrap_err();

    assert_eq!(status.code(), Code::InvalidArgument);
}

#[tokio::test]
async fn delete_with_a_stale_fencing_token_is_rejected() {
    let (_dir, storage, svc) = setup();
    // An order with token 5 is accepted, so the worker now knows token 5.
    svc.delete_block(delete_request(ID, 5)).await.unwrap();
    store_block(&storage, ID, b"data");

    let status = svc.delete_block(delete_request(ID, 4)).await.unwrap_err();

    assert_eq!(status.code(), Code::FailedPrecondition);
    assert!(storage.block_path(ID).unwrap().exists());
}

// ----- PutBlock with overwrite ----------------------------------------------

fn sha_of(data: &[u8]) -> [u8; 32] {
    let mut sha = [0u8; 32];
    sha.copy_from_slice(&Sha256::digest(data));
    sha
}

fn header_frame(id: &str, size: u64, sha256: [u8; 32]) -> BlockFrame {
    BlockFrame {
        kind: Some(Kind::Header(BlockHeader {
            block_id: id.to_string(),
            sha256: sha256.to_vec(),
            size,
            next_worker: String::new(),
            overwrite: false,
        })),
    }
}

fn data_frame(bytes: &[u8]) -> BlockFrame {
    BlockFrame {
        kind: Some(Kind::Data(DataFrame {
            data: bytes.to_vec(),
            crc32c: 0,
        })),
    }
}

fn incoming(frames: Vec<BlockFrame>) -> impl Stream<Item = Result<BlockFrame, Status>> + Unpin {
    tokio_stream::iter(frames.into_iter().map(Ok))
}

fn with_overwrite(mut frame: BlockFrame) -> BlockFrame {
    if let Some(Kind::Header(header)) = frame.kind.as_mut() {
        header.overwrite = true;
    }
    frame
}

#[tokio::test]
async fn put_with_overwrite_repairs_a_corrupted_block() {
    let (_dir, storage, _svc) = setup();
    store_block(&storage, ID, b"hello");
    std::fs::write(storage.block_path(ID).unwrap(), b"jello").unwrap();
    let frames = vec![
        with_overwrite(header_frame(ID, 5, sha_of(b"hello"))),
        data_frame(b"hello"),
    ];

    receive_block(Arc::clone(&storage), incoming(frames))
        .await
        .unwrap();

    storage.verify_block(ID).unwrap();
    assert_eq!(storage.used_bytes(), 5);
}

#[tokio::test]
async fn put_with_overwrite_on_a_missing_block_is_a_normal_write() {
    let (_dir, storage, _svc) = setup();
    let frames = vec![
        with_overwrite(header_frame(ID, 5, sha_of(b"hello"))),
        data_frame(b"hello"),
    ];

    receive_block(Arc::clone(&storage), incoming(frames))
        .await
        .unwrap();

    storage.verify_block(ID).unwrap();
    assert_eq!(storage.used_bytes(), 5);
}

// ----- ReplicateBlock (two real workers talking over localhost) -------------

/// Starts a real gRPC server on a free local port and returns its address.
async fn start_server(svc: WorkerSvc) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();

    tokio::spawn(async move {
        tonic::transport::Server::builder()
            .add_service(WorkerServiceServer::new(svc))
            .serve_with_incoming(TcpListenerStream::new(listener))
            .await
            .unwrap();
    });

    address.to_string()
}

fn replicate_request(id: &str, target: &str) -> Request<ReplicateBlockRequest> {
    Request::new(ReplicateBlockRequest {
        block_id: id.to_string(),
        target: target.to_string(),
        fencing: 0,
        overwrite: false,
    })
}

#[tokio::test]
async fn replicate_copies_a_block_to_another_worker() {
    let (_source_dir, source, source_svc) = setup_with(8); // several frames
    let (_target_dir, target, target_svc) = setup();
    let address = start_server(target_svc).await;
    let data = b"hello distributed world";
    store_block(&source, ID, data);

    let response = source_svc
        .replicate_block(replicate_request(ID, &address))
        .await
        .unwrap()
        .into_inner();

    assert_eq!(response.sent_bytes, data.len() as u64);
    assert_eq!(std::fs::read(target.block_path(ID).unwrap()).unwrap(), data);
    target.verify_block(ID).unwrap();
    assert_eq!(target.used_bytes(), data.len() as u64);
}

#[tokio::test]
async fn replicate_refuses_to_send_a_corrupted_copy() {
    let (_source_dir, source, source_svc) = setup();
    let (_target_dir, target, target_svc) = setup();
    let address = start_server(target_svc).await;
    store_block(&source, ID, b"hello");
    std::fs::write(source.block_path(ID).unwrap(), b"jello").unwrap();

    let status = source_svc
        .replicate_block(replicate_request(ID, &address))
        .await
        .unwrap_err();

    assert_eq!(status.code(), Code::DataLoss);
    assert!(!target.block_path(ID).unwrap().exists());
}

#[tokio::test]
async fn replicate_of_a_missing_block_is_not_found() {
    let (_dir, _storage, svc) = setup();

    let status = svc
        .replicate_block(replicate_request(ID, "127.0.0.1:1"))
        .await
        .unwrap_err();

    assert_eq!(status.code(), Code::NotFound);
}

#[tokio::test]
async fn replicate_to_a_worker_that_already_has_the_block_is_rejected() {
    let (_source_dir, source, source_svc) = setup();
    let (_target_dir, target, target_svc) = setup();
    let address = start_server(target_svc).await;
    store_block(&source, ID, b"hello");
    store_block(&target, ID, b"hello");

    let status = source_svc
        .replicate_block(replicate_request(ID, &address))
        .await
        .unwrap_err();

    assert_eq!(status.code(), Code::AlreadyExists);
}

#[tokio::test]
async fn replicate_with_overwrite_repairs_a_corrupted_target() {
    let (_source_dir, source, source_svc) = setup();
    let (_target_dir, target, target_svc) = setup();
    let address = start_server(target_svc).await;
    store_block(&source, ID, b"hello");
    store_block(&target, ID, b"hello");
    std::fs::write(target.block_path(ID).unwrap(), b"jello").unwrap();

    let mut request = replicate_request(ID, &address);
    request.get_mut().overwrite = true;
    source_svc.replicate_block(request).await.unwrap();

    target.verify_block(ID).unwrap();
    assert_eq!(target.used_bytes(), 5);
}

#[tokio::test]
async fn replicate_to_an_unreachable_worker_is_unavailable() {
    let (_dir, source, svc) = setup();
    store_block(&source, ID, b"hello");
    // Bind a port and release it, so nothing is listening there.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap().to_string();
    drop(listener);

    let status = svc
        .replicate_block(replicate_request(ID, &address))
        .await
        .unwrap_err();

    assert_eq!(status.code(), Code::Unavailable);
}

#[tokio::test]
async fn replicate_without_a_target_is_invalid() {
    let (_dir, _storage, svc) = setup();

    let status = svc
        .replicate_block(replicate_request(ID, ""))
        .await
        .unwrap_err();

    assert_eq!(status.code(), Code::InvalidArgument);
}

#[tokio::test]
async fn replicate_with_a_stale_fencing_token_is_rejected() {
    let (_dir, _storage, svc) = setup();
    let mut newer = replicate_request(ID, "127.0.0.1:1");
    newer.get_mut().fencing = 5;
    let _ = svc.replicate_block(newer).await; // accepted by fencing, fails later: no block

    let mut older = replicate_request(ID, "127.0.0.1:1");
    older.get_mut().fencing = 4;
    let status = svc.replicate_block(older).await.unwrap_err();

    assert_eq!(status.code(), Code::FailedPrecondition);
}

// ----- PutBlock pipeline (A stores and forwards to B) -----------------------

fn with_next_worker(mut frame: BlockFrame, next_worker: &str) -> BlockFrame {
    if let Some(Kind::Header(header)) = frame.kind.as_mut() {
        header.next_worker = next_worker.to_string();
    }
    frame
}

fn pipeline_frames(next_worker: &str, parts: &[&[u8]]) -> Vec<BlockFrame> {
    let data = parts.concat();
    let header = header_frame(ID, data.len() as u64, sha_of(&data));
    let mut frames = vec![with_next_worker(header, next_worker)];
    frames.extend(parts.iter().map(|part| data_frame(part)));
    frames
}

#[tokio::test]
async fn pipeline_stores_the_block_on_both_workers() {
    let (_a_dir, a, _) = setup();
    let (_b_dir, b, b_svc) = setup();
    let b_address = start_server(b_svc).await;

    let frames = pipeline_frames(&b_address, &[b"hello ", b"distributed ", b"world"]);
    receive_block(Arc::clone(&a), incoming(frames))
        .await
        .unwrap();

    for storage in [&a, &b] {
        let stored = std::fs::read(storage.block_path(ID).unwrap()).unwrap();
        assert_eq!(stored, b"hello distributed world");
        storage.verify_block(ID).unwrap();
    }
}

#[tokio::test]
async fn pipeline_fails_with_unavailable_and_writes_nothing_if_the_next_worker_is_down() {
    let (a_dir, a, _) = setup();
    // Bind and drop a listener to get a port nobody listens on.
    let free = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let dead_address = free.local_addr().unwrap().to_string();
    drop(free);

    let frames = pipeline_frames(&dead_address, &[b"hello"]);
    let status = receive_block(Arc::clone(&a), incoming(frames))
        .await
        .unwrap_err();

    assert_eq!(status.code(), Code::Unavailable);
    assert!(!a.block_path(ID).unwrap().exists());
    assert_eq!(a.used_bytes(), 0);
    assert!(!a_dir.path().join("tmp").exists());
}

#[tokio::test]
async fn pipeline_leaves_no_copy_on_a_if_the_next_worker_rejects_the_block() {
    let (a_dir, a, _) = setup();
    let (_b_dir, b, b_svc) = setup();
    let b_address = start_server(b_svc).await;
    store_block(&b, ID, b"hello");

    let frames = pipeline_frames(&b_address, &[b"hello"]);
    let status = receive_block(Arc::clone(&a), incoming(frames))
        .await
        .unwrap_err();

    assert_eq!(status.code(), Code::AlreadyExists);
    assert!(!a.block_path(ID).unwrap().exists());
    assert_eq!(a.used_bytes(), 0);
    assert_eq!(
        std::fs::read_dir(a_dir.path().join("tmp")).unwrap().count(),
        0
    );
}
