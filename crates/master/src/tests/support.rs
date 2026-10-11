//! Shared setup for the tests that talk to DynamoDB.
//!
//! They need dynamodb-local running (see deploy/compose.local.yml) and the
//! endpoint in DFSHA_TEST_DYNAMO_ENDPOINT, e.g. http://localhost:8000. Without
//! the variable they print a note and pass, so `cargo test` still works on a
//! machine without Docker. CI sets the variable.

use crate::db::{Db, Fence};
use crate::time::now_ms;

pub struct TestDb {
    pub db: Db,
    pub fence: Fence,
}

/// Fresh, empty tables under a random prefix. Returns None when there is no
/// local DynamoDB to talk to.
pub async fn test_tables() -> Option<Db> {
    let Ok(endpoint) = std::env::var("DFSHA_TEST_DYNAMO_ENDPOINT") else {
        eprintln!("skipping: DFSHA_TEST_DYNAMO_ENDPOINT is not set");
        return None;
    };

    let prefix = format!("t{}_", &uuid::Uuid::new_v4().simple().to_string()[..8]);
    let db = Db::connect(Some(&endpoint), &prefix)
        .await
        .expect("connect to dynamodb-local");
    db.create_tables().await.expect("create tables");
    Some(db)
}

/// Fresh tables with the leader lease already taken by "test-master" so
/// fenced writes work.
pub async fn test_db() -> Option<TestDb> {
    let db = test_tables().await?;

    let lease = db
        .try_acquire_leader("test-master", "127.0.0.1:50051", now_ms(), 60_000)
        .await
        .expect("acquire leader lease")
        .expect("nobody else holds the lease in fresh tables");

    Some(TestDb {
        fence: Fence {
            holder: lease.holder,
            fencing: lease.fencing,
        },
        db,
    })
}

impl TestDb {
    /// Best effort: tables leak only if a test panics, and dynamodb-local in
    /// memory mode forgets them on restart anyway.
    pub async fn cleanup(self) {
        let _ = self.db.delete_tables().await;
    }
}

pub fn sha(data: &[u8]) -> Vec<u8> {
    use sha2::{Digest, Sha256};
    Sha256::digest(data).to_vec()
}
