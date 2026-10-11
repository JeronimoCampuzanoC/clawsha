//! The write path, as the master sees it. The bytes never come here: the
//! gateway cuts the file into blocks and streams them to the workers. What we
//! do is hand out the plan (block ids + worker pairs), keep the metadata
//! PENDING while the gateway works, and publish it atomically at the end.
//!
//!   StartUpload   -> lease the path, PENDING version, block rows, assignments
//!   ReassignBlock -> a worker pair failed: new block id, new pair
//!   CommitUpload  -> one transaction: version COMMITTED, inode points to it,
//!                    previous version OLD, lease released
//!   AbortUpload   -> version OLD (GC frees the blocks), lease released
//!
//! Replacing a file is just another upload: readers keep seeing the old
//! version until the commit flips the pointer, and never a mix of both.

use std::sync::Arc;

use crate::cluster::{ClusterState, WorkerInfo};
use crate::db::blocks::BlockRecord;
use crate::db::inodes::{Inode, InodeType, delete_inode_item, set_current_version_item};
use crate::db::leases::release_path_lease_item;
use crate::db::versions::{Version, VersionStatus, commit_version_item, retire_version_item, version_id};
use crate::db::{Fence, on_condition_failed};
use crate::error::MasterError;
use crate::namespace::Namespace;
use crate::placement;

/// How long a gateway may go without renewing before another writer may take
/// the path. The gateway renews every 20 s while the upload runs.
pub const PATH_LEASE_TTL_MS: u64 = 60_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockAssignment {
    pub idx: u32,
    pub block_id: String,
    pub size: u64,
    /// Pipeline order: PutBlock goes to workers[0], which forwards to workers[1].
    pub workers: Vec<WorkerInfo>,
}

#[derive(Debug, Clone)]
pub struct UploadPlan {
    pub inode_id: String,
    pub version_no: u64,
    pub lease_id: String,
    pub block_size: u64,
    pub blocks: Vec<BlockAssignment>,
}

/// What the gateway tells us about one block at commit time.
#[derive(Debug, Clone)]
pub struct CommittedBlock {
    pub idx: u32,
    pub block_id: String,
    pub size: u64,
    pub sha256: Vec<u8>,
    pub worker_ids: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct FileInfo {
    pub path: String,
    pub inode_id: String,
    pub version_no: u64,
    pub size: u64,
    pub sha256: Vec<u8>,
    pub block_count: u32,
    pub modified_at: u64,
}

#[derive(Clone)]
pub struct UploadCoordinator {
    ns: Namespace,
    cluster: Arc<ClusterState>,
    block_size: u64,
    /// Copies every block must have before we accept the commit (R).
    replication: usize,
}

impl UploadCoordinator {
    pub fn new(ns: Namespace, cluster: Arc<ClusterState>, block_size: u64, replication: usize) -> Self {
        Self {
            ns,
            cluster,
            block_size,
            replication,
        }
    }

    pub fn block_size(&self) -> u64 {
        self.block_size
    }

    /// Sizes of the blocks of a file: all full except possibly the last.
    fn block_sizes(&self, total: u64) -> Vec<u64> {
        let mut sizes = Vec::new();
        let mut left = total;
        while left > 0 {
            let size = left.min(self.block_size);
            sizes.push(size);
            left -= size;
        }
        sizes
    }

    /// Workers that can take new blocks right now. A worker needs room for
    /// two more blocks to count, so a single assignment never fills it up.
    fn candidates(&self, now_ms: u64, exclude: &[String]) -> Vec<WorkerInfo> {
        self.cluster
            .write_candidates(now_ms, 2 * self.block_size)
            .into_iter()
            .filter(|w| !exclude.contains(&w.id))
            .collect()
    }

    /// One pair per block. As we assign, we charge the bytes to the chosen
    /// workers in our local copy, so a 1 GiB file does not land entirely on
    /// the worker that happened to be emptiest at the start.
    fn assign_blocks(
        &self,
        sizes: &[u64],
        mut candidates: Vec<WorkerInfo>,
    ) -> Result<Vec<BlockAssignment>, MasterError> {
        let mut rng = rand::rng();
        let mut blocks = Vec::with_capacity(sizes.len());
        for (idx, &size) in sizes.iter().enumerate() {
            let (first, second) = placement::pick_pair(&candidates, &mut rng)?;
            for chosen in [&first, &second] {
                if let Some(w) = candidates.iter_mut().find(|w| w.id == chosen.id) {
                    w.used_bytes += size;
                }
            }
            blocks.push(BlockAssignment {
                idx: idx as u32,
                block_id: uuid::Uuid::new_v4().to_string(),
                size,
                workers: vec![first, second],
            });
        }
        Ok(blocks)
    }

    pub async fn start(
        &self,
        fence: &Fence,
        user_id: &str,
        path: &str,
        size: u64,
        lease_id: Option<&str>,
        now_ms: u64,
    ) -> Result<UploadPlan, MasterError> {
        // Fail fast, before touching the metadata, if there is nowhere to write.
        let sizes = self.block_sizes(size);
        let candidates = self.candidates(now_ms, &[]);
        if !sizes.is_empty() && candidates.len() < self.replication {
            return Err(MasterError::NoWorkers(format!(
                "need {} alive workers with free space, have {}",
                self.replication,
                candidates.len()
            )));
        }

        let (parent, name) = self.ns.resolve_parent(user_id, path).await?;
        let db = self.ns.db();

        // Reuse the inode of an existing file (this is a replacement) or
        // create a hidden one that becomes visible at commit.
        let inode = match db.get_inode(&parent.inode_id, &name).await? {
            Some(existing) if existing.kind == InodeType::Dir => {
                return Err(MasterError::AlreadyExists);
            }
            Some(existing) => existing,
            None => {
                let fresh = Inode {
                    parent_id: parent.inode_id.clone(),
                    name: name.clone(),
                    inode_id: uuid::Uuid::new_v4().to_string(),
                    owner_id: user_id.to_string(),
                    kind: InodeType::File,
                    current_version: 0,
                    size: 0,
                    modified_at: now_ms,
                };
                db.put_inode(fence, &fresh).await?;
                fresh
            }
        };

        // The lease is what keeps two writers apart (test 4 of the plan).
        let lease_id = lease_id
            .filter(|l| !l.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        db.acquire_path_lease(fence, &inode.inode_id, &lease_id, now_ms, PATH_LEASE_TTL_MS)
            .await?;

        let version_no = db.latest_version_no(&inode.inode_id).await? + 1;
        db.put_pending_version(fence, &inode.inode_id, version_no, size, now_ms)
            .await?;

        let blocks = self.assign_blocks(&sizes, candidates)?;
        let vid = version_id(&inode.inode_id, version_no);
        let rows: Vec<BlockRecord> = blocks
            .iter()
            .map(|b| BlockRecord {
                version_id: vid.clone(),
                idx: b.idx,
                block_id: b.block_id.clone(),
                size: b.size,
                sha256: Vec::new(),
            })
            .collect();
        db.put_blocks(fence, &rows).await?;

        Ok(UploadPlan {
            inode_id: inode.inode_id,
            version_no,
            lease_id,
            block_size: self.block_size,
            blocks,
        })
    }

    /// Checks that the caller is the writer of this pending version. Renewing
    /// the lease is the check: only the holder can.
    async fn check_writer(
        &self,
        fence: &Fence,
        inode_id: &str,
        version_no: u64,
        lease_id: &str,
        now_ms: u64,
    ) -> Result<Version, MasterError> {
        let db = self.ns.db();
        let version = db
            .get_version(inode_id, version_no)
            .await?
            .ok_or(MasterError::NotFound)?;
        if version.status != VersionStatus::Pending {
            return Err(MasterError::InvalidArgument(format!(
                "version {version_no} is {}, not PENDING",
                version.status.as_str()
            )));
        }
        db.acquire_path_lease(fence, inode_id, lease_id, now_ms, PATH_LEASE_TTL_MS)
            .await?;
        Ok(version)
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn reassign(
        &self,
        fence: &Fence,
        _user_id: &str,
        inode_id: &str,
        version_no: u64,
        lease_id: &str,
        idx: u32,
        exclude: &[String],
        now_ms: u64,
    ) -> Result<BlockAssignment, MasterError> {
        self.check_writer(fence, inode_id, version_no, lease_id, now_ms)
            .await?;
        let db = self.ns.db();
        let vid = version_id(inode_id, version_no);

        let current = db
            .list_blocks(&vid)
            .await?
            .into_iter()
            .find(|b| b.idx == idx)
            .ok_or(MasterError::NotFound)?;

        let candidates = self.candidates(now_ms, exclude);
        let mut blocks = self.assign_blocks(&[current.size], candidates)?;
        let mut assignment = blocks.pop().expect("one size in, one assignment out");
        assignment.idx = idx;

        db.put_blocks(
            fence,
            &[BlockRecord {
                version_id: vid,
                idx,
                block_id: assignment.block_id.clone(),
                size: assignment.size,
                sha256: Vec::new(),
            }],
        )
        .await?;
        Ok(assignment)
    }

    fn check_report(&self, size: u64, blocks: &[CommittedBlock]) -> Result<(), MasterError> {
        let expected = self.block_sizes(size);
        if blocks.len() != expected.len() {
            return Err(MasterError::InvalidArgument(format!(
                "{} bytes need {} blocks, got {}",
                size,
                expected.len(),
                blocks.len()
            )));
        }
        for (i, (block, &want)) in blocks.iter().zip(&expected).enumerate() {
            if block.idx as usize != i {
                return Err(MasterError::InvalidArgument(format!(
                    "blocks must come in order: position {i} has idx {}",
                    block.idx
                )));
            }
            if block.size != want {
                return Err(MasterError::InvalidArgument(format!(
                    "block {i} should have {want} bytes, got {}",
                    block.size
                )));
            }
            if block.sha256.len() != 32 {
                return Err(MasterError::InvalidArgument(format!(
                    "block {i} needs a 32-byte sha256"
                )));
            }
            if block.worker_ids.len() < self.replication {
                // Never confirm a block with a single copy.
                return Err(MasterError::InvalidArgument(format!(
                    "block {i} has {} copies, {} required",
                    block.worker_ids.len(),
                    self.replication
                )));
            }
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn commit(
        &self,
        fence: &Fence,
        user_id: &str,
        path: &str,
        inode_id: &str,
        version_no: u64,
        lease_id: &str,
        size: u64,
        sha256: &[u8],
        blocks: Vec<CommittedBlock>,
        now_ms: u64,
    ) -> Result<FileInfo, MasterError> {
        if sha256.len() != 32 {
            return Err(MasterError::InvalidArgument(
                "file sha256 must be 32 bytes".into(),
            ));
        }
        self.check_report(size, &blocks)?;
        let version = self
            .check_writer(fence, inode_id, version_no, lease_id, now_ms)
            .await?;
        if version.size != size {
            return Err(MasterError::InvalidArgument(format!(
                "upload was announced with {} bytes, commit says {size}",
                version.size
            )));
        }

        // The entry must still be the file we started with (not removed or
        // replaced by a directory while the upload ran).
        let (parent, name) = self.ns.resolve_parent(user_id, path).await?;
        let db = self.ns.db();
        let inode = db
            .get_inode(&parent.inode_id, &name)
            .await?
            .filter(|i| i.inode_id == inode_id && i.kind == InodeType::File)
            .ok_or(MasterError::NotFound)?;

        // Block rows get their hashes first; if we crash here the version is
        // still PENDING and nothing is visible.
        let vid = version_id(inode_id, version_no);
        let rows: Vec<BlockRecord> = blocks
            .iter()
            .map(|b| BlockRecord {
                version_id: vid.clone(),
                idx: b.idx,
                block_id: b.block_id.clone(),
                size: b.size,
                sha256: b.sha256.clone(),
            })
            .collect();
        db.put_blocks(fence, &rows).await?;

        // The commit itself: all of these or none.
        let mut items = vec![
            commit_version_item(db, inode_id, version_no, size, sha256, blocks.len() as u64, now_ms)?,
            set_current_version_item(db, &parent.inode_id, &name, inode_id, version_no, size, now_ms)?,
        ];
        if inode.current_version > 0 && inode.current_version != version_no {
            items.push(retire_version_item(db, inode_id, inode.current_version, now_ms)?);
        }
        let lease_item = items.len();
        items.push(release_path_lease_item(db, inode_id, lease_id)?);

        db.fenced_write(fence, items).await.map_err(|e| match e {
            MasterError::ConditionFailed(0) => {
                MasterError::InvalidArgument("version is no longer PENDING".into())
            }
            MasterError::ConditionFailed(1) => MasterError::NotFound,
            MasterError::ConditionFailed(i) if i == lease_item => MasterError::Locked,
            other => on_condition_failed(other, MasterError::Db("commit transaction failed".into())),
        })?;

        // Now the leader knows where the new blocks are, without waiting for
        // the workers' next heartbeat.
        for block in &blocks {
            for worker in &block.worker_ids {
                self.cluster.add_location(&block.block_id, worker);
            }
        }

        Ok(FileInfo {
            path: path.to_string(),
            inode_id: inode_id.to_string(),
            version_no,
            size,
            sha256: sha256.to_vec(),
            block_count: blocks.len() as u32,
            modified_at: now_ms,
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn abort(
        &self,
        fence: &Fence,
        user_id: &str,
        path: &str,
        inode_id: &str,
        version_no: u64,
        lease_id: &str,
        now_ms: u64,
    ) -> Result<(), MasterError> {
        self.check_writer(fence, inode_id, version_no, lease_id, now_ms)
            .await?;
        let db = self.ns.db();

        let mut items = vec![
            retire_version_item(db, inode_id, version_no, now_ms)?,
            release_path_lease_item(db, inode_id, lease_id)?,
        ];

        // A file that never had a committed version would stay as a hidden
        // ghost entry; remove it so the name is free again.
        let (parent, name) = self.ns.resolve_parent(user_id, path).await?;
        if let Some(inode) = db.get_inode(&parent.inode_id, &name).await?
            && inode.inode_id == inode_id
            && inode.current_version == 0
        {
            items.push(delete_inode_item(db, &parent.inode_id, &name, inode_id)?);
        }

        db.fenced_write(fence, items)
            .await
            .map_err(|e| on_condition_failed(e, MasterError::Locked))
    }
}
