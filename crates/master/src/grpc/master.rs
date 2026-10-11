//! MasterService: what the gateway asks the leader.
//!
//! Every handler starts with `require_fence()`: on a standby that returns
//! NotLeader (UNAVAILABLE) and the gateway retries through the NLB, which by
//! then points at the new leader. The fence it returns is what every write
//! below carries to DynamoDB.

use std::sync::Arc;

use tonic::{Request, Response, Status};

use crate::app::App;
use crate::auth;
use crate::cluster::{WorkerInfo, WorkerState};
use crate::db::inodes::{Inode, InodeType};
use crate::error::MasterError;
use crate::pb;
use crate::pb::master_service_server::MasterService;
use crate::read_plan::BlockLocation;
use crate::time::now_ms;
use crate::upload::{BlockAssignment, CommittedBlock, PATH_LEASE_TTL_MS};

/// Longest lock a client may ask for with AcquireLease.
const MAX_LEASE_TTL_MS: u64 = 10 * 60 * 1_000;

pub struct MasterApi {
    app: Arc<App>,
}

impl MasterApi {
    pub fn new(app: Arc<App>) -> Self {
        Self { app }
    }
}

fn require_user(user_id: &str) -> Result<&str, Status> {
    if user_id.is_empty() {
        return Err(Status::invalid_argument("user_id is required"));
    }
    Ok(user_id)
}

fn worker_addr(w: &WorkerInfo) -> pb::WorkerAddr {
    pb::WorkerAddr {
        worker_id: w.id.clone(),
        addr: w.addr.clone(),
        az: w.az.clone(),
    }
}

/// Where the worker keeps the block on disk, with the same sharding the
/// worker uses (blocks/ab/cd/<uuid>). Only for the detail panel; the master
/// never opens these paths.
fn disk_path(w: &WorkerInfo, block_id: &str) -> String {
    if w.data_dir.is_empty() || block_id.len() < 4 {
        return String::new();
    }
    format!(
        "{}/blocks/{}/{}/{}",
        w.data_dir,
        &block_id[0..2],
        &block_id[2..4],
        block_id
    )
}

fn entry(inode: &Inode) -> pb::Entry {
    pb::Entry {
        name: inode.name.clone(),
        r#type: match inode.kind {
            InodeType::Dir => pb::EntryType::Dir,
            InodeType::File => pb::EntryType::File,
        } as i32,
        size: inode.size,
        modified_at: inode.modified_at as i64,
        inode_id: inode.inode_id.clone(),
    }
}

fn assignment(b: &BlockAssignment) -> pb::BlockAssignment {
    pb::BlockAssignment {
        idx: b.idx,
        block_id: b.block_id.clone(),
        size: b.size,
        workers: b.workers.iter().map(worker_addr).collect(),
    }
}

fn location(b: &BlockLocation) -> pb::BlockLocation {
    pb::BlockLocation {
        idx: b.idx,
        block_id: b.block_id.clone(),
        size: b.size,
        sha256: b.sha256.clone(),
        replicas: b.replicas.iter().map(worker_addr).collect(),
    }
}

fn block_stat(b: &BlockLocation) -> pb::BlockStat {
    pb::BlockStat {
        idx: b.idx,
        block_id: b.block_id.clone(),
        size: b.size,
        sha256: b.sha256.clone(),
        replicas: b
            .replicas
            .iter()
            .map(|w| pb::ReplicaInfo {
                worker: Some(worker_addr(w)),
                disk_path: disk_path(w, &b.block_id),
            })
            .collect(),
    }
}

#[tonic::async_trait]
impl MasterService for MasterApi {
    async fn register(
        &self,
        request: Request<pb::RegisterRequest>,
    ) -> Result<Response<pb::RegisterResponse>, Status> {
        let fence = self.app.leadership.require_fence()?;
        let req = request.into_inner();
        auth::validate_username(&req.username)?;
        auth::validate_password(&req.password)?;

        let user_id = uuid::Uuid::new_v4().to_string();
        let hash = auth::hash_password(&req.password)?;
        self.app
            .db
            .create_user(&fence, &user_id, &req.username, &hash, now_ms())
            .await?;
        tracing::info!(username = %req.username, "user registered");
        Ok(Response::new(pb::RegisterResponse { user_id }))
    }

    async fn login(
        &self,
        request: Request<pb::LoginRequest>,
    ) -> Result<Response<pb::LoginResponse>, Status> {
        self.app.leadership.require_fence()?;
        let req = request.into_inner();

        // Same answer for "no such user" and "wrong password".
        let user = self
            .app
            .db
            .find_user_by_username(&req.username)
            .await?
            .ok_or(MasterError::Unauthenticated)?;
        if !auth::verify_password(&req.password, &user.password_hash) {
            return Err(MasterError::Unauthenticated.into());
        }
        Ok(Response::new(pb::LoginResponse {
            user_id: user.user_id,
        }))
    }

    async fn list(
        &self,
        request: Request<pb::PathRequest>,
    ) -> Result<Response<pb::ListResponse>, Status> {
        self.app.leadership.require_fence()?;
        let req = request.into_inner();
        let user = require_user(&req.user_id)?;
        let entries = self.app.namespace.list(user, &req.path).await?;
        Ok(Response::new(pb::ListResponse {
            entries: entries.iter().map(entry).collect(),
        }))
    }

    async fn mkdir(
        &self,
        request: Request<pb::PathRequest>,
    ) -> Result<Response<pb::Empty>, Status> {
        let fence = self.app.leadership.require_fence()?;
        let req = request.into_inner();
        let user = require_user(&req.user_id)?;
        self.app
            .namespace
            .mkdir(&fence, user, &req.path, now_ms())
            .await?;
        Ok(Response::new(pb::Empty {}))
    }

    async fn remove(
        &self,
        request: Request<pb::RemoveRequest>,
    ) -> Result<Response<pb::Empty>, Status> {
        let fence = self.app.leadership.require_fence()?;
        let req = request.into_inner();
        let user = require_user(&req.user_id)?;
        self.app
            .namespace
            .remove(&fence, user, &req.path, req.recursive, now_ms())
            .await?;
        Ok(Response::new(pb::Empty {}))
    }

    async fn r#move(
        &self,
        request: Request<pb::MoveRequest>,
    ) -> Result<Response<pb::Empty>, Status> {
        let fence = self.app.leadership.require_fence()?;
        let req = request.into_inner();
        let user = require_user(&req.user_id)?;
        self.app
            .namespace
            .mv(&fence, user, &req.from, &req.to, now_ms())
            .await?;
        Ok(Response::new(pb::Empty {}))
    }

    async fn stat(
        &self,
        request: Request<pb::PathRequest>,
    ) -> Result<Response<pb::StatResponse>, Status> {
        self.app.leadership.require_fence()?;
        let req = request.into_inner();
        let user = require_user(&req.user_id)?;

        let (inode, detail) = self.app.reads.detail(user, &req.path, now_ms()).await?;
        let mut response = pb::StatResponse {
            entry: Some(entry(&inode)),
            version_no: 0,
            sha256: Vec::new(),
            blocks: Vec::new(),
        };
        if let Some((version, blocks)) = detail {
            response.version_no = version.version_no;
            response.sha256 = version.sha256;
            response.blocks = blocks.iter().map(block_stat).collect();
        }
        Ok(Response::new(response))
    }

    async fn start_upload(
        &self,
        request: Request<pb::StartUploadRequest>,
    ) -> Result<Response<pb::UploadPlan>, Status> {
        let fence = self.app.leadership.require_fence()?;
        let req = request.into_inner();
        let user = require_user(&req.user_id)?;

        let lease = (!req.lease_id.is_empty()).then_some(req.lease_id.as_str());
        let plan = self
            .app
            .uploads
            .start(&fence, user, &req.path, req.size, lease, now_ms())
            .await?;
        tracing::info!(path = %req.path, size = req.size, blocks = plan.blocks.len(), "upload started");
        Ok(Response::new(pb::UploadPlan {
            inode_id: plan.inode_id,
            version_no: plan.version_no,
            lease_id: plan.lease_id,
            block_size: plan.block_size,
            blocks: plan.blocks.iter().map(assignment).collect(),
        }))
    }

    async fn reassign_block(
        &self,
        request: Request<pb::ReassignBlockRequest>,
    ) -> Result<Response<pb::BlockAssignment>, Status> {
        let fence = self.app.leadership.require_fence()?;
        let req = request.into_inner();
        let user = require_user(&req.user_id)?;
        let a = self
            .app
            .uploads
            .reassign(
                &fence,
                user,
                &req.inode_id,
                req.version_no,
                &req.lease_id,
                req.idx,
                &req.exclude_worker_ids,
                now_ms(),
            )
            .await?;
        Ok(Response::new(assignment(&a)))
    }

    async fn commit_upload(
        &self,
        request: Request<pb::CommitUploadRequest>,
    ) -> Result<Response<pb::FileInfo>, Status> {
        let fence = self.app.leadership.require_fence()?;
        let req = request.into_inner();
        let user = require_user(&req.user_id)?;

        let blocks = req
            .blocks
            .into_iter()
            .map(|b| CommittedBlock {
                idx: b.idx,
                block_id: b.block_id,
                size: b.size,
                sha256: b.sha256,
                worker_ids: b.worker_ids,
            })
            .collect();
        let info = self
            .app
            .uploads
            .commit(
                &fence,
                user,
                &req.path,
                &req.inode_id,
                req.version_no,
                &req.lease_id,
                req.size,
                &req.sha256,
                blocks,
                now_ms(),
            )
            .await?;
        tracing::info!(path = %req.path, version = info.version_no, size = info.size, "upload committed");
        Ok(Response::new(pb::FileInfo {
            path: info.path,
            inode_id: info.inode_id,
            version_no: info.version_no,
            size: info.size,
            sha256: info.sha256,
            block_count: info.block_count,
            modified_at: info.modified_at as i64,
        }))
    }

    async fn abort_upload(
        &self,
        request: Request<pb::AbortUploadRequest>,
    ) -> Result<Response<pb::Empty>, Status> {
        let fence = self.app.leadership.require_fence()?;
        let req = request.into_inner();
        let user = require_user(&req.user_id)?;
        self.app
            .uploads
            .abort(
                &fence,
                user,
                &req.path,
                &req.inode_id,
                req.version_no,
                &req.lease_id,
                now_ms(),
            )
            .await?;
        tracing::info!(path = %req.path, version = req.version_no, "upload aborted");
        Ok(Response::new(pb::Empty {}))
    }

    async fn get_read_plan(
        &self,
        request: Request<pb::ReadPlanRequest>,
    ) -> Result<Response<pb::ReadPlan>, Status> {
        self.app.leadership.require_fence()?;
        let req = request.into_inner();
        let user = require_user(&req.user_id)?;

        let version = (req.version_no > 0).then_some(req.version_no);
        let plan = self
            .app
            .reads
            .plan(user, &req.path, version, now_ms())
            .await?;
        Ok(Response::new(pb::ReadPlan {
            inode_id: plan.inode_id,
            version_no: plan.version_no,
            size: plan.size,
            sha256: plan.sha256,
            block_size: self.app.block_size,
            blocks: plan.blocks.iter().map(location).collect(),
        }))
    }

    async fn report_corruption(
        &self,
        request: Request<pb::CorruptionReport>,
    ) -> Result<Response<pb::Empty>, Status> {
        self.app.leadership.require_fence()?;
        let req = request.into_inner();
        if req.block_id.is_empty() || req.worker_id.is_empty() {
            return Err(Status::invalid_argument(
                "block_id and worker_id are required",
            ));
        }
        // Stop handing out that replica. Ordering the healthy copy to be
        // re-sent to the bad worker is the health module's job (B).
        self.app
            .cluster
            .remove_location(&req.block_id, &req.worker_id);
        tracing::warn!(block = %req.block_id, worker = %req.worker_id, "replica reported corrupt");
        Ok(Response::new(pb::Empty {}))
    }

    /// lock(): an exclusive write lease on a path. If the file does not exist
    /// yet, a hidden entry is created so the name is reserved; the upload that
    /// follows reuses the lease. (A lock that is never followed by an upload
    /// leaves that hidden entry behind; the name still works for uploads.)
    async fn acquire_lease(
        &self,
        request: Request<pb::LeaseRequest>,
    ) -> Result<Response<pb::Lease>, Status> {
        let fence = self.app.leadership.require_fence()?;
        let req = request.into_inner();
        let user = require_user(&req.user_id)?;
        let now = now_ms();

        let (parent, name) = self.app.namespace.resolve_parent(user, &req.path).await?;
        let inode = match self.app.db.get_inode(&parent.inode_id, &name).await? {
            Some(i) if i.kind == InodeType::Dir => {
                return Err(Status::invalid_argument("cannot lock a directory"));
            }
            Some(i) => i,
            None => {
                let fresh = Inode {
                    parent_id: parent.inode_id,
                    name,
                    inode_id: uuid::Uuid::new_v4().to_string(),
                    owner_id: user.to_string(),
                    kind: InodeType::File,
                    current_version: 0,
                    size: 0,
                    modified_at: now,
                };
                self.app.db.put_inode(&fence, &fresh).await?;
                fresh
            }
        };

        let lease_id = if req.lease_id.is_empty() {
            uuid::Uuid::new_v4().to_string()
        } else {
            req.lease_id
        };
        let ttl = match req.ttl_seconds {
            0 => PATH_LEASE_TTL_MS,
            s => (s as u64 * 1_000).min(MAX_LEASE_TTL_MS),
        };
        let expires_at = self
            .app
            .db
            .acquire_path_lease(&fence, &inode.inode_id, &lease_id, now, ttl)
            .await?;
        Ok(Response::new(pb::Lease {
            lease_id,
            inode_id: inode.inode_id,
            expires_at: expires_at as i64,
        }))
    }

    async fn release_lease(
        &self,
        request: Request<pb::Lease>,
    ) -> Result<Response<pb::Empty>, Status> {
        let fence = self.app.leadership.require_fence()?;
        let req = request.into_inner();
        self.app
            .db
            .release_path_lease(&fence, &req.inode_id, &req.lease_id)
            .await?;
        Ok(Response::new(pb::Empty {}))
    }

    /// Works on the standby too, so a dashboard can show both masters.
    async fn cluster_status(
        &self,
        _request: Request<pb::Empty>,
    ) -> Result<Response<pb::ClusterStatusResponse>, Status> {
        let now = now_ms();
        let leadership = &self.app.leadership;
        let workers = self
            .app
            .cluster
            .workers(now)
            .iter()
            .map(|w| pb::WorkerStatus {
                worker: Some(worker_addr(&w.info)),
                state: match w.state {
                    WorkerState::Alive => pb::WorkerState::Alive,
                    WorkerState::Suspect => pb::WorkerState::Suspect,
                    WorkerState::Dead => pb::WorkerState::Dead,
                } as i32,
                quota_bytes: w.info.quota_bytes,
                used_bytes: w.info.used_bytes,
                block_count: w.block_count,
                last_heartbeat: w.info.last_heartbeat as i64,
            })
            .collect();
        Ok(Response::new(pb::ClusterStatusResponse {
            master_id: leadership.id().to_string(),
            is_leader: leadership.is_leader(),
            fencing: leadership.fencing(),
            safe_mode: leadership.safe_mode(now),
            workers,
            under_replicated_blocks: self.app.cluster.under_replicated(2),
            total_blocks: self.app.cluster.total_blocks(),
        }))
    }
}
