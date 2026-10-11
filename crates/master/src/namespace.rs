//! The hierarchical view: ls, mkdir, rm, rmdir, mv and stat over the `inodes`
//! table. Everything is scoped to one user: paths are resolved from that
//! user's root, so another user's files simply do not exist from here
//! (NotFound, never "forbidden", so nothing leaks).

use crate::db::inodes::{Inode, InodeType, root_id};
use crate::db::versions::retire_version_item;
use crate::db::inodes::delete_inode_item;
use crate::db::{Db, Fence};
use crate::error::MasterError;
use crate::path;

#[derive(Clone)]
pub struct Namespace {
    db: Db,
}

impl Namespace {
    pub fn new(db: Db) -> Self {
        Self { db }
    }

    pub fn db(&self) -> &Db {
        &self.db
    }

    /// The virtual root of a user. It has no row; we make it up.
    fn root(user_id: &str) -> Inode {
        Inode {
            parent_id: String::new(),
            name: String::new(),
            inode_id: root_id(user_id),
            owner_id: user_id.to_string(),
            kind: InodeType::Dir,
            current_version: 0,
            size: 0,
            modified_at: 0,
        }
    }

    /// Walks `components` from the user's root, one GetItem per level, and
    /// returns every inode on the way (root first). Every component but the
    /// last must be a directory.
    async fn walk(&self, user_id: &str, components: &[String]) -> Result<Vec<Inode>, MasterError> {
        let mut chain = vec![Self::root(user_id)];
        for (i, name) in components.iter().enumerate() {
            let parent = chain.last().expect("chain starts with the root");
            if parent.kind != InodeType::Dir {
                return Err(MasterError::NotFound);
            }
            let inode = self
                .db
                .get_inode(&parent.inode_id, name)
                .await?
                .ok_or(MasterError::NotFound)?;
            let is_last = i + 1 == components.len();
            if !is_last && inode.kind != InodeType::Dir {
                return Err(MasterError::NotFound);
            }
            chain.push(inode);
        }
        Ok(chain)
    }

    /// Resolves a path to its inode. Files whose first upload has not been
    /// committed yet do not exist for readers.
    pub async fn resolve(&self, user_id: &str, path: &str) -> Result<Inode, MasterError> {
        let components = path::parse(path)?;
        let mut chain = self.walk(user_id, &components).await?;
        let inode = chain.pop().expect("chain is never empty");
        if !visible(&inode) {
            return Err(MasterError::NotFound);
        }
        Ok(inode)
    }

    /// Resolves the directory that would contain `path` and returns it with
    /// the final name. Used by everything that creates or moves an entry.
    pub async fn resolve_parent(
        &self,
        user_id: &str,
        path: &str,
    ) -> Result<(Inode, String), MasterError> {
        let (parent_components, name) = path::split_parent(path)?;
        let mut chain = self.walk(user_id, &parent_components).await?;
        let parent = chain.pop().expect("chain is never empty");
        if parent.kind != InodeType::Dir {
            return Err(MasterError::NotFound);
        }
        Ok((parent, name))
    }

    pub async fn stat(&self, user_id: &str, path: &str) -> Result<Inode, MasterError> {
        self.resolve(user_id, path).await
    }

    pub async fn list(&self, user_id: &str, path: &str) -> Result<Vec<Inode>, MasterError> {
        let dir = self.resolve(user_id, path).await?;
        if dir.kind != InodeType::Dir {
            return Err(MasterError::NotFound);
        }
        let children = self.db.list_children(&dir.inode_id).await?;
        Ok(children.into_iter().filter(visible).collect())
    }

    pub async fn mkdir(
        &self,
        fence: &Fence,
        user_id: &str,
        path: &str,
        now_ms: u64,
    ) -> Result<Inode, MasterError> {
        let (parent, name) = self.resolve_parent(user_id, path).await?;
        let dir = Inode {
            parent_id: parent.inode_id,
            name,
            inode_id: uuid::Uuid::new_v4().to_string(),
            owner_id: user_id.to_string(),
            kind: InodeType::Dir,
            current_version: 0,
            size: 0,
            modified_at: now_ms,
        };
        self.db.put_inode(fence, &dir).await?;
        Ok(dir)
    }

    /// rm and rmdir. A directory with content needs `recursive`; then its
    /// subtree is deleted bottom-up, one entry per transaction, so a crash in
    /// the middle leaves a smaller but still consistent tree.
    pub async fn remove(
        &self,
        fence: &Fence,
        user_id: &str,
        path: &str,
        recursive: bool,
        now_ms: u64,
    ) -> Result<(), MasterError> {
        // split_parent rejects "/" before we do anything else.
        path::split_parent(path)?;
        let inode = self.resolve(user_id, path).await?;

        if inode.kind == InodeType::Dir && !recursive && self.db.has_children(&inode.inode_id).await? {
            return Err(MasterError::DirNotEmpty);
        }
        self.remove_inode(fence, &inode, now_ms).await
    }

    async fn remove_inode(&self, fence: &Fence, inode: &Inode, now_ms: u64) -> Result<(), MasterError> {
        if inode.kind == InodeType::Dir {
            // Box::pin because async recursion needs a known-size future.
            for child in self.db.list_children(&inode.inode_id).await? {
                Box::pin(self.remove_inode(fence, &child, now_ms)).await?;
            }
            return self
                .db
                .delete_inode(fence, &inode.parent_id, &inode.name, &inode.inode_id)
                .await;
        }

        // A file: the entry goes away and its current version becomes OLD so
        // the GC frees the blocks after the grace period. Both or neither.
        let mut items = vec![delete_inode_item(
            &self.db,
            &inode.parent_id,
            &inode.name,
            &inode.inode_id,
        )?];
        if inode.current_version > 0 {
            items.push(retire_version_item(
                &self.db,
                &inode.inode_id,
                inode.current_version,
                now_ms,
            )?);
        }
        self.db
            .fenced_write(fence, items)
            .await
            .map_err(|e| crate::db::on_condition_failed(e, MasterError::NotFound))
    }

    /// Renames or relocates an entry. Only metadata moves: blocks stay put.
    pub async fn mv(
        &self,
        fence: &Fence,
        user_id: &str,
        from: &str,
        to: &str,
        now_ms: u64,
    ) -> Result<(), MasterError> {
        path::split_parent(from)?;
        let source = self.resolve(user_id, from).await?;

        let (to_parent_components, to_name) = path::split_parent(to)?;
        let to_chain = self.walk(user_id, &to_parent_components).await?;
        let to_parent = to_chain.last().expect("chain is never empty");
        if to_parent.kind != InodeType::Dir {
            return Err(MasterError::NotFound);
        }
        // "mv /a /a/b/a" would detach /a from the tree forever.
        if to_chain.iter().any(|i| i.inode_id == source.inode_id) {
            return Err(MasterError::InvalidArgument(
                "cannot move a directory into itself".to_string(),
            ));
        }

        self.db
            .move_inode(fence, &source, &to_parent.inode_id, &to_name, now_ms)
            .await
    }
}

/// Directories always; files only once they have a committed version.
fn visible(inode: &Inode) -> bool {
    inode.kind == InodeType::Dir || inode.current_version > 0
}
