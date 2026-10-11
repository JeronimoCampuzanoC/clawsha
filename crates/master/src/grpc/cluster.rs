//! ClusterService: what the workers tell the leader.

use std::sync::Arc;

use tonic::{Request, Response, Status};

use crate::app::App;
use crate::cluster::Heartbeat;
use crate::pb;
use crate::pb::cluster_service_server::ClusterService;
use crate::time::now_ms;

pub struct ClusterApi {
    app: Arc<App>,
}

impl ClusterApi {
    pub fn new(app: Arc<App>) -> Self {
        Self { app }
    }
}

#[tonic::async_trait]
impl ClusterService for ClusterApi {
    async fn heartbeat(
        &self,
        request: Request<pb::HeartbeatRequest>,
    ) -> Result<Response<pb::HeartbeatResponse>, Status> {
        // A standby must not build a view of the cluster: the NLB should not
        // send heartbeats here, and if it does the worker will retry.
        let fence = self.app.leadership.require_fence()?;
        let hb = request.into_inner();
        if hb.worker_id.is_empty() || hb.addr.is_empty() {
            return Err(Status::invalid_argument("worker_id and addr are required"));
        }

        let outcome = self.app.cluster.heartbeat(
            Heartbeat {
                worker_id: hb.worker_id,
                addr: hb.addr,
                az: hb.az,
                quota_bytes: hb.quota_bytes,
                used_bytes: hb.used_bytes,
                new_blocks: hb.new_blocks,
                removed_blocks: hb.removed_blocks,
                data_dir: hb.data_dir,
            },
            now_ms(),
        );

        Ok(Response::new(pb::HeartbeatResponse {
            fencing: fence.fencing,
            full_report_requested: outcome.full_report_requested,
        }))
    }

    async fn block_report(
        &self,
        request: Request<pb::BlockReportRequest>,
    ) -> Result<Response<pb::Empty>, Status> {
        self.app.leadership.require_fence()?;
        let report = request.into_inner();
        if report.worker_id.is_empty() {
            return Err(Status::invalid_argument("worker_id is required"));
        }
        let blocks = report.blocks.into_iter().map(|b| b.block_id).collect();
        self.app.cluster.block_report(&report.worker_id, blocks);
        Ok(Response::new(pb::Empty {}))
    }
}
