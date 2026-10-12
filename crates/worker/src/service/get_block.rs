use std::fs::File;
use std::io::Read;

use tokio::sync::mpsc;
use tonic::Status;

use crate::error::StorageError;
use crate::pb::block_frame::Kind;
use crate::pb::{BlockFrame, DataFrame};

pub(super) fn data_frame(bytes: &[u8]) -> BlockFrame {
    BlockFrame {
        kind: Some(Kind::Data(DataFrame {
            data: bytes.to_vec(),
            crc32c: 0, // 0 = not set (per-frame CRC is P2)
        })),
    }
}

/// Reads the file in frames of `frame_size` bytes and hands each one to `emit`.
/// `emit` returns false when the receiver is gone, which stops the loop.
/// It blocks the thread, so it must run inside spawn_blocking.
pub(super) fn read_frames(
    mut file: File,
    frame_size: usize,
    mut emit: impl FnMut(BlockFrame) -> bool,
) -> Result<(), std::io::Error> {
    let mut buffer = vec![0u8; frame_size];

    loop {
        let bytes_read = match file.read(&mut buffer) {
            Ok(0) => return Ok(()), // end of the block
            Ok(n) => n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };

        if !emit(data_frame(&buffer[..bytes_read])) {
            return Ok(());
        }
    }
}

/// GetBlock side: feeds the response channel, reporting a disk error to the client.
pub(super) fn send_frames(
    file: File,
    frame_size: usize,
    tx: mpsc::Sender<Result<BlockFrame, Status>>,
) {
    let result = read_frames(file, frame_size, |frame| {
        tx.blocking_send(Ok(frame)).is_ok()
    });

    if let Err(e) = result {
        let _ = tx.blocking_send(Err(Status::from(StorageError::Io(e))));
    }
}
