//! Everything the RPC handlers need, built once in main and shared.

use std::sync::Arc;

use crate::cluster::ClusterState;
use crate::db::Db;
use crate::leader::Leadership;
use crate::namespace::Namespace;
use crate::read_plan::ReadPlanner;
use crate::upload::UploadCoordinator;

pub struct App {
    pub db: Db,
    pub cluster: Arc<ClusterState>,
    pub leadership: Arc<Leadership>,
    pub namespace: Namespace,
    pub uploads: UploadCoordinator,
    pub reads: ReadPlanner,
    pub block_size: u64,
    /// Replication factor: how many copies a block should have.
    pub replication: usize,
}

impl App {
    pub fn new(
        db: Db,
        master_id: &str,
        advertise_addr: &str,
        block_size: u64,
        replication: usize,
    ) -> Self {
        let cluster = Arc::new(ClusterState::new());
        let leadership = Arc::new(Leadership::new(
            db.clone(),
            master_id,
            advertise_addr,
            cluster.clone(),
        ));
        let namespace = Namespace::new(db.clone());
        Self {
            uploads: UploadCoordinator::new(
                namespace.clone(),
                cluster.clone(),
                block_size,
                replication,
            ),
            reads: ReadPlanner::new(namespace.clone(), cluster.clone()),
            namespace,
            cluster,
            leadership,
            db,
            block_size,
            replication,
        }
    }
}
