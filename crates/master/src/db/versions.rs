//! `versions`: every upload of a file is a new version. A version is PENDING
//! while the gateway is still writing blocks, COMMITTED once it is the one
//! readers get, and OLD after a newer one replaced it (or the file was
//! removed). OLD and abandoned PENDING versions are what the GC collects.

use aws_sdk_dynamodb::types::{Put, TransactWriteItem, Update};

use super::attr::{Item, b, get_b, get_n, get_n_or, get_s, n, s};
use super::tables::VERSIONS_BY_STATUS_INDEX;
use super::{Db, Fence, build_error, on_condition_failed, sdk_error};
use crate::error::MasterError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VersionStatus {
    Pending,
    Committed,
    Old,
}

impl VersionStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            VersionStatus::Pending => "PENDING",
            VersionStatus::Committed => "COMMITTED",
            VersionStatus::Old => "OLD",
        }
    }

    fn parse(value: &str) -> Result<Self, MasterError> {
        match value {
            "PENDING" => Ok(VersionStatus::Pending),
            "COMMITTED" => Ok(VersionStatus::Committed),
            "OLD" => Ok(VersionStatus::Old),
            other => Err(MasterError::Db(format!("unknown version status {other:?}"))),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Version {
    pub inode_id: String,
    pub version_no: u64,
    pub status: VersionStatus,
    pub size: u64,
    /// SHA-256 of the whole file; empty while PENDING.
    pub sha256: Vec<u8>,
    pub block_count: u64,
    pub updated_at: u64,
}

/// Key of the `blocks` rows that belong to a version.
pub fn version_id(inode_id: &str, version_no: u64) -> String {
    format!("{inode_id}#{version_no}")
}

impl Db {
    pub async fn get_version(
        &self,
        inode_id: &str,
        version_no: u64,
    ) -> Result<Option<Version>, MasterError> {
        let out = self
            .client
            .get_item()
            .table_name(self.tables.versions())
            .key("inode_id", s(inode_id))
            .key("version_no", n(version_no))
            .consistent_read(true)
            .send()
            .await
            .map_err(sdk_error)?;
        out.item().map(version_from_item).transpose()
    }

    /// Highest version number ever given to this inode, 0 if none.
    pub async fn latest_version_no(&self, inode_id: &str) -> Result<u64, MasterError> {
        let out = self
            .client
            .query()
            .table_name(self.tables.versions())
            .key_condition_expression("inode_id = :i")
            .expression_attribute_values(":i", s(inode_id))
            .scan_index_forward(false)
            .limit(1)
            .consistent_read(true)
            .send()
            .await
            .map_err(sdk_error)?;
        match out.items().first() {
            Some(item) => get_n(item, "version_no"),
            None => Ok(0),
        }
    }

    pub async fn put_pending_version(
        &self,
        fence: &Fence,
        inode_id: &str,
        version_no: u64,
        size: u64,
        now_ms: u64,
    ) -> Result<(), MasterError> {
        let put = Put::builder()
            .table_name(self.tables.versions())
            .item("inode_id", s(inode_id))
            .item("version_no", n(version_no))
            .item("status", s(VersionStatus::Pending.as_str()))
            .item("size", n(size))
            .item("block_count", n(0))
            .item("updated_at", n(now_ms))
            .condition_expression("attribute_not_exists(inode_id)")
            .build()
            .map_err(build_error)?;
        self.fenced_write(fence, vec![TransactWriteItem::builder().put(put).build()])
            .await
            .map_err(|e| on_condition_failed(e, MasterError::AlreadyExists))
    }

    /// Versions in `status`, oldest update first, updated before `before_ms`.
    /// Reads the by_status index (eventually consistent, fine for a GC).
    /// For the GC (B): OLD versions past the grace period and PENDING
    /// uploads nobody finished.
    #[allow(dead_code)]
    pub async fn versions_by_status(
        &self,
        status: VersionStatus,
        before_ms: u64,
        limit: i32,
    ) -> Result<Vec<Version>, MasterError> {
        let out = self
            .client
            .query()
            .table_name(self.tables.versions())
            .index_name(VERSIONS_BY_STATUS_INDEX)
            .key_condition_expression("#status = :s AND updated_at < :t")
            .expression_attribute_names("#status", "status")
            .expression_attribute_values(":s", s(status.as_str()))
            .expression_attribute_values(":t", n(before_ms))
            .limit(limit)
            .send()
            .await
            .map_err(sdk_error)?;
        out.items().iter().map(version_from_item).collect()
    }
}

/// PENDING -> COMMITTED with the final size, hash and block count. Fails if
/// the version is not PENDING any more (double commit, or aborted meanwhile).
pub(crate) fn commit_version_item(
    db: &Db,
    inode_id: &str,
    version_no: u64,
    size: u64,
    sha256: &[u8],
    block_count: u64,
    now_ms: u64,
) -> Result<TransactWriteItem, MasterError> {
    let update = Update::builder()
        .table_name(db.tables.versions())
        .key("inode_id", s(inode_id))
        .key("version_no", n(version_no))
        .condition_expression("#status = :pending")
        .update_expression(
            "SET #status = :committed, #size = :size, sha256 = :sha, block_count = :count, updated_at = :now",
        )
        .expression_attribute_names("#status", "status")
        .expression_attribute_names("#size", "size")
        .expression_attribute_values(":pending", s(VersionStatus::Pending.as_str()))
        .expression_attribute_values(":committed", s(VersionStatus::Committed.as_str()))
        .expression_attribute_values(":size", n(size))
        .expression_attribute_values(":sha", b(sha256))
        .expression_attribute_values(":count", n(block_count))
        .expression_attribute_values(":now", n(now_ms))
        .build()
        .map_err(build_error)?;
    Ok(TransactWriteItem::builder().update(update).build())
}

/// Any status -> OLD, so the GC picks the version up after the grace period.
pub(crate) fn retire_version_item(
    db: &Db,
    inode_id: &str,
    version_no: u64,
    now_ms: u64,
) -> Result<TransactWriteItem, MasterError> {
    let update = Update::builder()
        .table_name(db.tables.versions())
        .key("inode_id", s(inode_id))
        .key("version_no", n(version_no))
        .condition_expression("attribute_exists(inode_id)")
        .update_expression("SET #status = :old, updated_at = :now")
        .expression_attribute_names("#status", "status")
        .expression_attribute_values(":old", s(VersionStatus::Old.as_str()))
        .expression_attribute_values(":now", n(now_ms))
        .build()
        .map_err(build_error)?;
    Ok(TransactWriteItem::builder().update(update).build())
}

fn version_from_item(item: &Item) -> Result<Version, MasterError> {
    Ok(Version {
        inode_id: get_s(item, "inode_id")?,
        version_no: get_n(item, "version_no")?,
        status: VersionStatus::parse(&get_s(item, "status")?)?,
        size: get_n_or(item, "size", 0)?,
        sha256: if item.contains_key("sha256") {
            get_b(item, "sha256")?
        } else {
            Vec::new()
        },
        block_count: get_n_or(item, "block_count", 0)?,
        updated_at: get_n(item, "updated_at")?,
    })
}
