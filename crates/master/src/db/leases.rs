//! `path_leases`: one row per inode that somebody is writing. Taken by
//! StartUpload (and by the explicit lock() of RF3), renewed while the upload
//! runs, released on commit/abort. If the gateway dies mid-upload nobody
//! releases it, so it carries an expiry and the next writer just waits it out.

use aws_sdk_dynamodb::types::{Delete, Put, TransactWriteItem};

use super::attr::{n, s};
use super::{Db, Fence, build_error, on_condition_failed};
use crate::error::MasterError;

impl Db {
    /// Takes or renews the lease. Fails with Locked while another holder's
    /// lease is still valid.
    pub async fn acquire_path_lease(
        &self,
        fence: &Fence,
        inode_id: &str,
        lease_id: &str,
        now_ms: u64,
        ttl_ms: u64,
    ) -> Result<u64, MasterError> {
        let expires_at = now_ms + ttl_ms;
        let put = Put::builder()
            .table_name(self.tables.path_leases())
            .item("inode_id", s(inode_id))
            .item("holder", s(lease_id))
            .item("expires_at", n(expires_at))
            .condition_expression(
                "attribute_not_exists(inode_id) OR expires_at < :now OR holder = :me",
            )
            .expression_attribute_values(":now", n(now_ms))
            .expression_attribute_values(":me", s(lease_id))
            .build()
            .map_err(build_error)?;

        self.fenced_write(fence, vec![TransactWriteItem::builder().put(put).build()])
            .await
            .map_err(|e| on_condition_failed(e, MasterError::Locked))?;
        Ok(expires_at)
    }

    /// Releases the lease if we hold it; Locked if somebody else does.
    pub async fn release_path_lease(
        &self,
        fence: &Fence,
        inode_id: &str,
        lease_id: &str,
    ) -> Result<(), MasterError> {
        self.fenced_write(
            fence,
            vec![release_path_lease_item(self, inode_id, lease_id)?],
        )
        .await
        .map_err(|e| on_condition_failed(e, MasterError::Locked))
    }
}

/// Release as a transaction item, so CommitUpload can free the lease in the
/// same transaction that publishes the version.
pub(crate) fn release_path_lease_item(
    db: &Db,
    inode_id: &str,
    lease_id: &str,
) -> Result<TransactWriteItem, MasterError> {
    let delete = Delete::builder()
        .table_name(db.tables.path_leases())
        .key("inode_id", s(inode_id))
        .condition_expression("holder = :me")
        .expression_attribute_values(":me", s(lease_id))
        .build()
        .map_err(build_error)?;
    Ok(TransactWriteItem::builder().delete(delete).build())
}
