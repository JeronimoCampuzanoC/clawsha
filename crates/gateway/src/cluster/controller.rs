use axum::{Json, extract::State, http::StatusCode};
use serde::Serialize;

use crate::pb::{ClusterStatusResponse, WorkerState, WorkerStatus};
use crate::state::AppState;

#[derive(Debug, Serialize, PartialEq)]
pub struct ClusterStatus {
    /// The master that answered. Requests go through the internal load
    /// balancer, which only routes to the leader.
    pub master_id: String,
    pub is_leader: bool,
    pub fencing: u64,
    /// Right after a failover: the leader waits for block reports and does
    /// not repair or delete anything yet.
    pub safe_mode: bool,
    pub under_replicated_blocks: u64,
    pub total_blocks: u64,
    pub workers: Vec<WorkerSummary>,
}

#[derive(Debug, Serialize, PartialEq)]
pub struct WorkerSummary {
    pub id: String,
    pub addr: String,
    pub az: String,
    /// "alive", "suspect" or "dead".
    pub state: &'static str,
    pub quota_bytes: u64,
    pub used_bytes: u64,
    pub block_count: u64,
    /// Unix time in milliseconds.
    pub last_heartbeat: i64,
}

pub async fn status(State(state): State<AppState>) -> Result<Json<ClusterStatus>, StatusCode> {
    let response = state
        .master
        .cluster_status()
        .await
        .map_err(map_status_error)?;
    Ok(Json(ClusterStatus::from(response)))
}

impl From<ClusterStatusResponse> for ClusterStatus {
    fn from(r: ClusterStatusResponse) -> Self {
        Self {
            master_id: r.master_id,
            is_leader: r.is_leader,
            fencing: r.fencing,
            safe_mode: r.safe_mode,
            under_replicated_blocks: r.under_replicated_blocks,
            total_blocks: r.total_blocks,
            workers: r.workers.into_iter().map(WorkerSummary::from).collect(),
        }
    }
}

impl From<WorkerStatus> for WorkerSummary {
    fn from(w: WorkerStatus) -> Self {
        let state = match WorkerState::try_from(w.state).unwrap_or(WorkerState::Unspecified) {
            WorkerState::Alive => "alive",
            WorkerState::Suspect => "suspect",
            WorkerState::Dead => "dead",
            WorkerState::Unspecified => "unknown",
        };
        let addr = w.worker.unwrap_or_default();
        Self {
            id: addr.worker_id,
            addr: addr.addr,
            az: addr.az,
            state,
            quota_bytes: w.quota_bytes,
            used_bytes: w.used_bytes,
            block_count: w.block_count,
            last_heartbeat: w.last_heartbeat,
        }
    }
}

fn map_status_error(status: tonic::Status) -> StatusCode {
    match status.code() {
        // No leader yet, or the master is unreachable.
        tonic::Code::Unavailable | tonic::Code::FailedPrecondition => {
            StatusCode::SERVICE_UNAVAILABLE
        }
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pb::WorkerAddr;

    #[test]
    fn the_master_answer_becomes_plain_json_fields() {
        let response = ClusterStatusResponse {
            master_id: "master-b".into(),
            is_leader: true,
            fencing: 103,
            safe_mode: false,
            under_replicated_blocks: 2,
            total_blocks: 22,
            workers: vec![WorkerStatus {
                worker: Some(WorkerAddr {
                    worker_id: "worker-b".into(),
                    addr: "worker-b:50052".into(),
                    az: "b".into(),
                }),
                state: WorkerState::Dead as i32,
                quota_bytes: 100,
                used_bytes: 40,
                block_count: 22,
                last_heartbeat: 1_700_000_000_000,
            }],
        };

        let status = ClusterStatus::from(response);
        assert_eq!(status.master_id, "master-b");
        assert_eq!(status.under_replicated_blocks, 2);
        assert_eq!(
            status.workers,
            vec![WorkerSummary {
                id: "worker-b".into(),
                addr: "worker-b:50052".into(),
                az: "b".into(),
                state: "dead",
                quota_bytes: 100,
                used_bytes: 40,
                block_count: 22,
                last_heartbeat: 1_700_000_000_000,
            }]
        );
    }

    #[test]
    fn no_leader_is_service_unavailable() {
        assert_eq!(
            map_status_error(tonic::Status::unavailable("this master is not the leader")),
            StatusCode::SERVICE_UNAVAILABLE
        );
    }
}
