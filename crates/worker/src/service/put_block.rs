use std::sync::Arc;

use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_stream::wrappers::ReceiverStream;
use tokio_stream::{Stream, StreamExt};
use tonic::Status;
use tonic::transport::Channel;

use super::CHANNEL_CAPACITY;
use super::blocking::blocking;
use super::replicate::{connect_to, target_error};
use crate::pb::block_frame::Kind;
use crate::pb::worker_service_client::WorkerServiceClient;
use crate::pb::{BlockFrame, BlockHeader, DataFrame, PutBlockResponse};
use crate::storage::BlockWriter;
use crate::storage::Storage;

/// What the first frame of a PutBlock stream tells us about the block.
#[derive(Debug)]
struct PutHeader {
    block_id: String,
    size: u64,
    sha256: [u8; 32],
    overwrite: bool,
    /// Address of the next worker in the pipeline, empty if this one is the last.
    next_worker: String,
}

/// The next worker in the pipeline. For it, this worker is a normal client.
struct Downstream {
    frames: mpsc::Sender<BlockFrame>,
    answer: JoinHandle<Result<(), Status>>,
}

impl Downstream {
    /// Starts the PutBlock call to the next worker, header first.
    /// The next worker is the last one, so its header has no next_worker.
    async fn start(
        mut client: WorkerServiceClient<Channel>,
        target: String,
        header: &PutHeader,
    ) -> Result<Self, Status> {
        let (frames, rx) = mpsc::channel(CHANNEL_CAPACITY);
        let first = BlockFrame {
            kind: Some(Kind::Header(BlockHeader {
                block_id: header.block_id.clone(),
                sha256: header.sha256.to_vec(),
                size: header.size,
                next_worker: String::new(),
                overwrite: header.overwrite,
            })),
        };
        frames
            .send(first)
            .await
            .map_err(|_| Status::internal("could not queue the header"))?;

        let answer = tokio::spawn(async move {
            client
                .put_block(ReceiverStream::new(rx))
                .await
                .map(|_| ())
                .map_err(|status| target_error(&target, status))
        });
        Ok(Self { frames, answer })
    }

    async fn forward(&mut self, data: DataFrame) -> Result<(), Status> {
        let frame = BlockFrame {
            kind: Some(Kind::Data(data)),
        };
        if self.frames.send(frame).await.is_ok() {
            return Ok(());
        }
        // The next worker closed the stream early: its answer says why.
        match self.reply().await {
            Err(status) => Err(status),
            Ok(()) => Err(Status::internal("next worker stopped reading early")),
        }
    }

    /// Ends the stream and waits until the next worker has stored the block.
    async fn confirm(mut self) -> Result<(), Status> {
        drop(self.frames);
        (&mut self.answer)
            .await
            .unwrap_or_else(|_| Err(Status::internal("forwarding task failed")))
    }

    async fn reply(&mut self) -> Result<(), Status> {
        (&mut self.answer)
            .await
            .unwrap_or_else(|_| Err(Status::internal("forwarding task failed")))
    }
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

    Ok(PutHeader {
        block_id: header.block_id,
        size: header.size,
        sha256,
        overwrite: header.overwrite,
        next_worker: header.next_worker,
    })
}

/// Receives one block from a stream of frames and stores it.
/// With a next_worker, every frame is also forwarded and this copy is
/// published only after the next worker confirmed its own.
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

    // 2. Connect to the next worker before touching the disk: if it is down,
    //    fail now and leave nothing behind.
    let next = match header.next_worker.as_str() {
        "" => None,
        target => Some((connect_to(target).await?, target.to_string())),
    };

    // 3. Start writing: validates the id, checks duplicates and reserves quota.
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
    let mut downstream = match next {
        Some((client, target)) => Some(Downstream::start(client, target, &header).await?),
        None => None,
    };

    // 4. Every following frame must carry data.
    while let Some(item) = stream.next().await {
        let frame = item?;
        match frame.kind {
            Some(Kind::Data(data_frame)) => {
                if let Some(next) = downstream.as_mut() {
                    next.forward(data_frame.clone()).await?;
                }
                writer = append_chunk(writer, data_frame.data).await?;
            }
            _ => {
                return Err(Status::invalid_argument(
                    "expected only data frames after the header",
                ));
            }
        }
    }

    // 5. Wait for the next worker. If it failed we return here, and dropping
    //    the writer removes the temp file and gives the quota back.
    if let Some(next) = downstream {
        next.confirm().await?;
    }

    // 6. The stream ended: finish checks size and hash and publishes the block.
    blocking(move || writer.finish()).await?;

    Ok(PutBlockResponse {
        block_id: header.block_id,
        stored_bytes: header.size,
    })
}
