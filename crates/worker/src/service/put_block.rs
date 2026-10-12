use std::sync::Arc;

use tokio_stream::{Stream, StreamExt};
use tonic::Status;

use super::blocking::blocking;
use crate::block_writer::BlockWriter;
use crate::pb::block_frame::Kind;
use crate::pb::{BlockFrame, PutBlockResponse};
use crate::storage::Storage;

/// What the first frame of a PutBlock stream tells us about the block.
#[derive(Debug)]
struct PutHeader {
    block_id: String,
    size: u64,
    sha256: [u8; 32],
    overwrite: bool,
}

/// Writes one chunk on the blocking pool. The writer goes into the task and comes back.
async fn append_chunk(mut writer: BlockWriter, chunk: Vec<u8>) -> Result<BlockWriter, Status> {
    blocking(move || {
        writer.write_chunk(&chunk)?;
        Ok(writer)
    })
    .await
}

fn parse_header(first_frame: Option<BlockFrame>) -> Result<PutHeader, Status> {
    let Some(frame) = first_frame else {
        return Err(Status::invalid_argument("stream ended before the header"));
    };

    let Some(Kind::Header(header)) = frame.kind else {
        return Err(Status::invalid_argument("the first frame must be a header"));
    };

    let sha256: [u8; 32] = header
        .sha256
        .try_into()
        .map_err(|_| Status::invalid_argument("sha256 must be exactly 32 bytes"))?;

    if !header.next_worker.is_empty() {
        return Err(Status::unimplemented(
            "pipeline (next_worker) is not implemented yet",
        ));
    }

    Ok(PutHeader {
        block_id: header.block_id,
        size: header.size,
        sha256,
        overwrite: header.overwrite,
    })
}

/// Receives one block from a stream of frames and stores it.
pub(crate) async fn receive_block<S>(
    storage: Arc<Storage>,
    mut stream: S,
) -> Result<PutBlockResponse, Status>
where
    S: Stream<Item = Result<BlockFrame, Status>> + Unpin,
{
    // 1. The first frame must be the header.
    let first_frame = match stream.next().await {
        Some(Ok(frame)) => Some(frame),
        Some(Err(status)) => return Err(status),
        None => None,
    };
    let header = parse_header(first_frame)?;

    // 2. Start writing: validates the id, checks duplicates and reserves quota.
    let block_id = header.block_id.clone();
    let (size, sha256, overwrite) = (header.size, header.sha256, header.overwrite);
    let storage_for_task = Arc::clone(&storage);
    let mut writer = blocking(move || {
        if overwrite {
            storage_for_task.begin_overwrite(&block_id, size, sha256)
        } else {
            storage_for_task.begin_write(&block_id, size, sha256)
        }
    })
    .await?;

    // 3. Every following frame must carry data.
    while let Some(item) = stream.next().await {
        let frame = item?;
        match frame.kind {
            Some(Kind::Data(data_frame)) => {
                writer = append_chunk(writer, data_frame.data).await?;
            }
            _ => {
                return Err(Status::invalid_argument(
                    "expected only data frames after the header",
                ));
            }
        }
    }

    // 4. The stream ended: finish checks size and hash and publishes the block.
    blocking(move || writer.finish()).await?;

    Ok(PutBlockResponse {
        block_id: header.block_id,
        stored_bytes: header.size,
    })
}
