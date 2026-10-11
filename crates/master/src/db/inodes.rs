//! `inodes`: one row per directory entry, keyed by (parent_id, name).
//!
//! The tree is only ever walked one level at a time, so this is all we need:
//! `ls` is a Query by parent_id, resolving a path is one GetItem per level,
//! and `mv` is moving one row. A user's root is the virtual inode
//! "root:<user_id>"; it has no row of its own.

use aws_sdk_dynamodb::types::{Delete, Put, TransactWriteItem, Update};

use super::attr::{Item, get_n, get_n_or, get_s, n, s};
use super::{Db, Fence, build_error, on_condition_failed, sdk_error};
use crate::error::MasterError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InodeType {
    Dir,
    File,
}

impl InodeType {
    fn as_str(self) -> &'static str {
        match self {
            InodeType::Dir => "DIR",
            InodeType::File => "FILE",
        }
    }

    fn parse(value: &str) -> Result<Self, MasterError> {
        match value {
            "DIR" => Ok(InodeType::Dir),
            "FILE" => Ok(InodeType::File),
            other => Err(MasterError::Db(format!("unknown inode type {other:?}"))),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Inode {
    pub parent_id: String,
    pub name: String,
    pub inode_id: String,
    pub owner_id: String,
    pub kind: InodeType,
    /// Version readers see. 0 means "no committed version yet" (a file whose
    /// first upload is still running) and such files are hidden from ls.
    pub current_version: u64,
    pub size: u64,
    pub modified_at: u64,
}

/// The id of a user's root directory. Never stored, only used as parent_id.
pub fn root_id(user_id: &str) -> String {
    format!("root:{user_id}")
}

impl Db {
    pub async fn get_inode(&self, parent_id: &str, name: &str) -> Result<Option<Inode>, MasterError> {
        let out = self
            .client
            .get_item()
            .table_name(self.tables.inodes())
            .key("parent_id", s(parent_id))
            .key("name", s(name))
            .consistent_read(true)
            .send()
            .await
            .map_err(sdk_error)?;
        out.item().map(inode_from_item).transpose()
    }

    /// Every entry under `parent_id`, sorted by name (DynamoDB sorts by the
    /// range key for us).
    pub async fn list_children(&self, parent_id: &str) -> Result<Vec<Inode>, MasterError> {
        let mut found = Vec::new();
        let mut start_key = None;
        loop {
            let out = self
                .client
                .query()
                .table_name(self.tables.inodes())
                .key_condition_expression("parent_id = :p")
                .expression_attribute_values(":p", s(parent_id))
                .consistent_read(true)
                .set_exclusive_start_key(start_key)
                .send()
                .await
                .map_err(sdk_error)?;
            for item in out.items() {
                found.push(inode_from_item(item)?);
            }
            start_key = out.last_evaluated_key().cloned();
            if start_key.is_none() {
                break;
            }
        }
        Ok(found)
    }

    pub async fn has_children(&self, parent_id: &str) -> Result<bool, MasterError> {
        let out = self
            .client
            .query()
            .table_name(self.tables.inodes())
            .key_condition_expression("parent_id = :p")
            .expression_attribute_values(":p", s(parent_id))
            .consistent_read(true)
            .limit(1)
            .send()
            .await
            .map_err(sdk_error)?;
        Ok(out.count() > 0)
    }

    /// Creates the entry; fails with AlreadyExists if the name is taken.
    pub async fn put_inode(&self, fence: &Fence, inode: &Inode) -> Result<(), MasterError> {
        self.fenced_write(fence, vec![put_inode_item(self, inode)?])
            .await
            .map_err(|e| on_condition_failed(e, MasterError::AlreadyExists))
    }

    /// Deletes the entry only if it still is the inode the caller looked at.
    pub async fn delete_inode(
        &self,
        fence: &Fence,
        parent_id: &str,
        name: &str,
        inode_id: &str,
    ) -> Result<(), MasterError> {
        self.fenced_write(fence, vec![delete_inode_item(self, parent_id, name, inode_id)?])
            .await
            .map_err(|e| on_condition_failed(e, MasterError::NotFound))
    }

    /// Moves `inode` to (new_parent, new_name) in one transaction: the new row
    /// must not exist and the old one must still be the same inode.
    pub async fn move_inode(
        &self,
        fence: &Fence,
        inode: &Inode,
        new_parent: &str,
        new_name: &str,
        now_ms: u64,
    ) -> Result<(), MasterError> {
        if inode.parent_id == new_parent && inode.name == new_name {
            return Ok(());
        }
        let moved = Inode {
            parent_id: new_parent.to_string(),
            name: new_name.to_string(),
            modified_at: now_ms,
            ..inode.clone()
        };
        let items = vec![
            put_inode_item(self, &moved)?,
            delete_inode_item(self, &inode.parent_id, &inode.name, &inode.inode_id)?,
        ];
        self.fenced_write(fence, items).await.map_err(|e| match e {
            MasterError::ConditionFailed(0) => MasterError::AlreadyExists,
            MasterError::ConditionFailed(_) => MasterError::NotFound,
            other => other,
        })
    }
}

pub(crate) fn put_inode_item(db: &Db, inode: &Inode) -> Result<TransactWriteItem, MasterError> {
    let put = Put::builder()
        .table_name(db.tables.inodes())
        .item("parent_id", s(&inode.parent_id))
        .item("name", s(&inode.name))
        .item("inode_id", s(&inode.inode_id))
        .item("owner_id", s(&inode.owner_id))
        .item("kind", s(inode.kind.as_str()))
        .item("current_version", n(inode.current_version))
        .item("size", n(inode.size))
        .item("modified_at", n(inode.modified_at))
        .condition_expression("attribute_not_exists(parent_id)")
        .build()
        .map_err(build_error)?;
    Ok(TransactWriteItem::builder().put(put).build())
}

pub(crate) fn delete_inode_item(
    db: &Db,
    parent_id: &str,
    name: &str,
    inode_id: &str,
) -> Result<TransactWriteItem, MasterError> {
    let delete = Delete::builder()
        .table_name(db.tables.inodes())
        .key("parent_id", s(parent_id))
        .key("name", s(name))
        .condition_expression("inode_id = :id")
        .expression_attribute_values(":id", s(inode_id))
        .build()
        .map_err(build_error)?;
    Ok(TransactWriteItem::builder().delete(delete).build())
}

/// Points a file at a newly committed version. Part of the commit transaction.
pub(crate) fn set_current_version_item(
    db: &Db,
    parent_id: &str,
    name: &str,
    inode_id: &str,
    version_no: u64,
    size: u64,
    now_ms: u64,
) -> Result<TransactWriteItem, MasterError> {
    // "size" is a reserved word in DynamoDB expressions, hence the alias.
    let update = Update::builder()
        .table_name(db.tables.inodes())
        .key("parent_id", s(parent_id))
        .key("name", s(name))
        .condition_expression("inode_id = :id")
        .update_expression("SET current_version = :v, #size = :size, modified_at = :now")
        .expression_attribute_names("#size", "size")
        .expression_attribute_values(":id", s(inode_id))
        .expression_attribute_values(":v", n(version_no))
        .expression_attribute_values(":size", n(size))
        .expression_attribute_values(":now", n(now_ms))
        .build()
        .map_err(build_error)?;
    Ok(TransactWriteItem::builder().update(update).build())
}

fn inode_from_item(item: &Item) -> Result<Inode, MasterError> {
    Ok(Inode {
        parent_id: get_s(item, "parent_id")?,
        name: get_s(item, "name")?,
        inode_id: get_s(item, "inode_id")?,
        owner_id: get_s(item, "owner_id")?,
        kind: InodeType::parse(&get_s(item, "kind")?)?,
        current_version: get_n_or(item, "current_version", 0)?,
        size: get_n_or(item, "size", 0)?,
        modified_at: get_n(item, "modified_at")?,
    })
}
