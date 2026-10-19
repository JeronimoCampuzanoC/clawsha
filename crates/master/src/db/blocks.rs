//! `blocks`: the ordered list of blocks of a version, keyed by
//! (version_id, idx). Rows are written at StartUpload (id and size) and
//! rewritten at CommitUpload with the hash the gateway computed. Where each
//! block lives is NOT here: the leader keeps that in memory (see cluster.rs).

use std::collections::HashSet;

use aws_sdk_dynamodb::types::{Delete, Put, TransactWriteItem};

use super::attr::{Item, b, get_b, get_n, get_s, n, s};
use super::{Db, Fence, MAX_ITEMS_PER_WRITE, build_error, sdk_error};
use crate::error::MasterError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockRecord {
    pub version_id: String,
    pub idx: u32,
    pub block_id: String,
    pub size: u64,
    /// Empty until the upload is committed.
    pub sha256: Vec<u8>,
}

impl Db {
    /// Writes (or overwrites) the rows, in fenced chunks of up to 99.
    pub async fn put_blocks(
        &self,
        fence: &Fence,
        records: &[BlockRecord],
    ) -> Result<(), MasterError> {
        for chunk in records.chunks(MAX_ITEMS_PER_WRITE) {
            let mut items = Vec::with_capacity(chunk.len());
            for rec in chunk {
                let mut put = Put::builder()
                    .table_name(self.tables.blocks())
                    .item("version_id", s(&rec.version_id))
                    .item("idx", n(rec.idx as u64))
                    .item("block_id", s(&rec.block_id))
                    .item("size", n(rec.size));
                if !rec.sha256.is_empty() {
                    put = put.item("sha256", b(&rec.sha256));
                }
                let put = put.build().map_err(build_error)?;
                items.push(TransactWriteItem::builder().put(put).build());
            }
            self.fenced_write(fence, items).await?;
        }
        Ok(())
    }

    /// All blocks of a version, by idx ascending.
    pub async fn list_blocks(&self, version_id: &str) -> Result<Vec<BlockRecord>, MasterError> {
        let mut found = Vec::new();
        let mut start_key = None;
        loop {
            let out = self
                .client
                .query()
                .table_name(self.tables.blocks())
                .key_condition_expression("version_id = :v")
                .expression_attribute_values(":v", s(version_id))
                .consistent_read(true)
                .set_exclusive_start_key(start_key)
                .send()
                .await
                .map_err(sdk_error)?;
            for item in out.items() {
                found.push(block_from_item(item)?);
            }
            start_key = out.last_evaluated_key().cloned();
            if start_key.is_none() {
                break;
            }
        }
        Ok(found)
    }

    /// Every block id some version still points to, PENDING ones included
    /// (their rows are written before any worker gets the data). A strongly
    /// consistent scan: the GC deletes from workers whatever is not here, so
    /// a stale read could lose data.
    pub async fn referenced_block_ids(&self) -> Result<HashSet<String>, MasterError> {
        let mut found = HashSet::new();
        let mut start_key = None;
        loop {
            let out = self
                .client
                .scan()
                .table_name(self.tables.blocks())
                .projection_expression("block_id")
                .consistent_read(true)
                .set_exclusive_start_key(start_key)
                .send()
                .await
                .map_err(sdk_error)?;
            for item in out.items() {
                found.insert(get_s(item, "block_id")?);
            }
            start_key = out.last_evaluated_key().cloned();
            if start_key.is_none() {
                break;
            }
        }
        Ok(found)
    }

    /// Removes every row of a version. For the GC, once the version is OLD
    /// and past its grace period.
    pub async fn delete_blocks(&self, fence: &Fence, version_id: &str) -> Result<(), MasterError> {
        let existing = self.list_blocks(version_id).await?;
        for chunk in existing.chunks(MAX_ITEMS_PER_WRITE) {
            let mut items = Vec::with_capacity(chunk.len());
            for rec in chunk {
                let delete = Delete::builder()
                    .table_name(self.tables.blocks())
                    .key("version_id", s(version_id))
                    .key("idx", n(rec.idx as u64))
                    .build()
                    .map_err(build_error)?;
                items.push(TransactWriteItem::builder().delete(delete).build());
            }
            self.fenced_write(fence, items).await?;
        }
        Ok(())
    }
}

fn block_from_item(item: &Item) -> Result<BlockRecord, MasterError> {
    Ok(BlockRecord {
        version_id: get_s(item, "version_id")?,
        idx: get_n(item, "idx")? as u32,
        block_id: get_s(item, "block_id")?,
        size: get_n(item, "size")?,
        sha256: if item.contains_key("sha256") {
            get_b(item, "sha256")?
        } else {
            Vec::new()
        },
    })
}
