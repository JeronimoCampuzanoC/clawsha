use futures::Stream;
use tonic::Status;
use tonic::transport::{Channel, Endpoint};

use crate::pb::{
    BlockFrame, GetBlockRequest, PutBlockResponse, worker_service_client::WorkerServiceClient,
};

#[derive(Clone)]
pub struct WorkerClient {
    client: WorkerServiceClient<Channel>,
}

impl WorkerClient {
    pub async fn connect(address: impl Into<String>) -> Result<Self, tonic::transport::Error> {
        let address = address.into();
        let uri = if address.starts_with("http://") || address.starts_with("https://") {
            address
        } else {
            format!("http://{address}")
        };
        let endpoint = Endpoint::from_shared(uri)?;
        let channel = endpoint.connect().await?;

        Ok(Self {
            client: WorkerServiceClient::new(channel),
        })
    }

    pub async fn put_block<S>(&self, frames: S) -> Result<PutBlockResponse, Status>
    where
        S: Stream<Item = BlockFrame> + Send + 'static,
    {
        let mut client = self.client.clone();
        let response = client.put_block(frames).await?;

        Ok(response.into_inner())
    }

    pub async fn get_block(
        &self,
        request: GetBlockRequest,
    ) -> Result<tonic::Streaming<BlockFrame>, Status> {
        let mut client = self.client.clone();
        let response = client.get_block(request).await?;

        Ok(response.into_inner())
    }
}
