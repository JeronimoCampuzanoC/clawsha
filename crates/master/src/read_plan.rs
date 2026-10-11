//! The read path. A read plan is the ordered list of blocks of a version with
//! the hash the gateway must verify and the workers believed to hold each
//! block. The gateway picks replicas by cost, fetches in parallel and falls
//! back to the other copy on a bad hash; none of that needs the master again.

use std::sync::Arc;

use crate::cluster::{ClusterState, WorkerInfo};
use crate::db::inodes::{Inode, InodeType};
use crate::db::versions::{Version, VersionStatus, version_id};
use crate::error::MasterError;
use crate::namespace::Namespace;

#[derive(Debug, Clone)]
pub struct BlockLocation {
    pub idx: u32,
    pub block_id: String,
    pub size: u64,
    pub sha256: Vec<u8>,
    pub replicas: Vec<WorkerInfo>,
}

#[derive(Debug, Clone)]
pub struct ReadPlan {
    pub inode_id: String,
    pub version_no: u64,
    pub size: u64,
    pub sha256: Vec<u8>,
    pub blocks: Vec<BlockLocation>,
}

#[derive(Clone)]
pub struct ReadPlanner {
    ns: Namespace,
    cluster: Arc<ClusterState>,
}

impl ReadPlanner {
    pub fn new(ns: Namespace, cluster: Arc<ClusterState>) -> Self {
        Self { ns, cluster }
    }

    /// Resolves the file and the version to read: the current one, or the
    /// one an open() handle pinned (an OLD version is still readable while
    /// the GC grace period lasts).
    async fn file_version(
        &self,
        user_id: &str,
        path: &str,
        version_no: Option<u64>,
    ) -> Result<(Inode, Version), MasterError> {
        let inode = self.ns.resolve(user_id, path).await?;
        if inode.kind != InodeType::File {
            return Err(MasterError::NotFound);
        }
        let wanted = version_no.filter(|v| *v > 0).unwrap_or(inode.current_version);
        let version = self
            .ns
            .db()
            .get_version(&inode.inode_id, wanted)
            .await?
            .ok_or(MasterError::NotFound)?;
        if version.status == VersionStatus::Pending {
            return Err(MasterError::NotFound);
        }
        Ok((inode, version))
    }

    /// Every block with the replicas we can offer. `allow_missing` keeps
    /// blocks without a live replica (for stat, which only displays); a read
    /// plan refuses them instead, because the gateway could not serve them.
    async fn locate(
        &self,
        inode_id: &str,
        version_no: u64,
        now_ms: u64,
        allow_missing: bool,
    ) -> Result<Vec<BlockLocation>, MasterError> {
        let rows = self
            .ns
            .db()
            .list_blocks(&version_id(inode_id, version_no))
            .await?;
        let mut blocks = Vec::with_capacity(rows.len());
        for row in rows {
            let replicas = self.cluster.locations(&row.block_id, now_ms);
            if replicas.is_empty() && !allow_missing {
                return Err(MasterError::NoReplica(row.block_id));
            }
            blocks.push(BlockLocation {
                idx: row.idx,
                block_id: row.block_id,
                size: row.size,
                sha256: row.sha256,
                replicas,
            });
        }
        Ok(blocks)
    }

    pub async fn plan(
        &self,
        user_id: &str,
        path: &str,
        version_no: Option<u64>,
        now_ms: u64,
    ) -> Result<ReadPlan, MasterError> {
        let (inode, version) = self.file_version(user_id, path, version_no).await?;
        let blocks = self
            .locate(&inode.inode_id, version.version_no, now_ms, false)
            .await?;
        Ok(ReadPlan {
            inode_id: inode.inode_id,
            version_no: version.version_no,
            size: version.size,
            sha256: version.sha256,
            blocks,
        })
    }

    /// For stat: the current version and its blocks, replicas included even
    /// when there are none left (that is exactly what the panel should show).
    pub async fn detail(
        &self,
        user_id: &str,
        path: &str,
        now_ms: u64,
    ) -> Result<(Inode, Option<(Version, Vec<BlockLocation>)>), MasterError> {
        let inode = self.ns.resolve(user_id, path).await?;
        if inode.kind != InodeType::File {
            return Ok((inode, None));
        }
        let version = self
            .ns
            .db()
            .get_version(&inode.inode_id, inode.current_version)
            .await?
            .ok_or(MasterError::NotFound)?;
        let blocks = self
            .locate(&inode.inode_id, version.version_no, now_ms, true)
            .await?;
        Ok((inode, Some((version, blocks))))
    }
}
