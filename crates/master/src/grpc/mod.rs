//! The gRPC surface: MasterService for gateways and ClusterService for
//! workers, both on the same port. Handlers here only translate between the
//! protobuf messages and the domain modules; the rules live in those modules.

mod cluster;
mod master;

use std::sync::Arc;

use tokio_stream::wrappers::TcpListenerStream;
use tonic::transport::Server;

use crate::app::App;
use crate::pb::cluster_service_server::ClusterServiceServer;
use crate::pb::master_service_server::MasterServiceServer;

pub async fn serve(app: Arc<App>, listener: tokio::net::TcpListener) {
    let result = Server::builder()
        .add_service(MasterServiceServer::new(master::MasterApi::new(
            app.clone(),
        )))
        .add_service(ClusterServiceServer::new(cluster::ClusterApi::new(app)))
        .serve_with_incoming(TcpListenerStream::new(listener))
        .await;
    if let Err(e) = result {
        tracing::error!(error = %e, "grpc server stopped");
    }
}
