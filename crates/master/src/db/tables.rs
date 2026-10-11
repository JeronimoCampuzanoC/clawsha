//! Table names and their definitions. On AWS Terraform creates the tables
//! (module data/); the definitions here are for dynamodb-local and tests, and
//! they are the reference for what Terraform must create.

use aws_sdk_dynamodb::error::SdkError;
use aws_sdk_dynamodb::operation::create_table::CreateTableError;
use aws_sdk_dynamodb::operation::describe_table::DescribeTableError;
use aws_sdk_dynamodb::types::{
    AttributeDefinition, BillingMode, GlobalSecondaryIndex, KeySchemaElement, KeyType, Projection,
    ProjectionType, ScalarAttributeType, TableStatus,
};
use std::time::Duration;

use super::{Db, build_error};
use crate::error::MasterError;

pub struct Tables {
    prefix: String,
}

impl Tables {
    pub fn new(prefix: &str) -> Self {
        Self {
            prefix: prefix.to_string(),
        }
    }

    fn name(&self, base: &str) -> String {
        format!("{}{}", self.prefix, base)
    }

    pub fn users(&self) -> String {
        self.name("users")
    }
    pub fn usernames(&self) -> String {
        self.name("usernames")
    }
    pub fn inodes(&self) -> String {
        self.name("inodes")
    }
    pub fn versions(&self) -> String {
        self.name("versions")
    }
    pub fn blocks(&self) -> String {
        self.name("blocks")
    }
    pub fn cluster(&self) -> String {
        self.name("cluster")
    }
    pub fn path_leases(&self) -> String {
        self.name("path_leases")
    }

    pub fn all(&self) -> Vec<String> {
        vec![
            self.users(),
            self.usernames(),
            self.inodes(),
            self.versions(),
            self.blocks(),
            self.cluster(),
            self.path_leases(),
        ]
    }
}

/// Name of the index on `versions` that lists versions by status, oldest
/// first. The GC uses it to find OLD versions past the grace period and
/// PENDING uploads that never finished.
pub const VERSIONS_BY_STATUS_INDEX: &str = "by_status";

/// One key attribute: name and type.
type KeyDef = (&'static str, ScalarAttributeType);
/// A global secondary index: name, hash key, range key.
type IndexDef = (&'static str, KeyDef, KeyDef);

struct TableDef {
    name: String,
    hash: KeyDef,
    range: Option<KeyDef>,
    gsi: Option<IndexDef>,
}

fn definitions(t: &Tables) -> Vec<TableDef> {
    use ScalarAttributeType::{N, S};
    vec![
        TableDef {
            name: t.users(),
            hash: ("user_id", S),
            range: None,
            gsi: None,
        },
        TableDef {
            name: t.usernames(),
            hash: ("username", S),
            range: None,
            gsi: None,
        },
        TableDef {
            name: t.inodes(),
            hash: ("parent_id", S),
            range: Some(("name", S)),
            gsi: None,
        },
        TableDef {
            name: t.versions(),
            hash: ("inode_id", S),
            range: Some(("version_no", N)),
            gsi: Some((VERSIONS_BY_STATUS_INDEX, ("status", S), ("updated_at", N))),
        },
        TableDef {
            name: t.blocks(),
            hash: ("version_id", S),
            range: Some(("idx", N)),
            gsi: None,
        },
        TableDef {
            name: t.cluster(),
            hash: ("pk", S),
            range: None,
            gsi: None,
        },
        TableDef {
            name: t.path_leases(),
            hash: ("inode_id", S),
            range: None,
            gsi: None,
        },
    ]
}

impl Db {
    /// Creates every table that does not exist yet and waits until all are
    /// ACTIVE. Safe to call on every start.
    pub async fn create_tables(&self) -> Result<(), MasterError> {
        for def in definitions(&self.tables) {
            if self.table_exists(&def.name).await? {
                continue;
            }
            self.create_table(&def).await?;
        }
        for name in self.tables.all() {
            self.wait_active(&name).await?;
        }
        Ok(())
    }

    /// Drops every table. Only tests use this.
    #[cfg_attr(not(test), allow(dead_code))]
    pub async fn delete_tables(&self) -> Result<(), MasterError> {
        for name in self.tables.all() {
            self.client
                .delete_table()
                .table_name(&name)
                .send()
                .await
                .map_err(super::sdk_error)?;
        }
        Ok(())
    }

    async fn table_exists(&self, name: &str) -> Result<bool, MasterError> {
        match self.client.describe_table().table_name(name).send().await {
            Ok(_) => Ok(true),
            Err(SdkError::ServiceError(err))
                if matches!(err.err(), DescribeTableError::ResourceNotFoundException(_)) =>
            {
                Ok(false)
            }
            Err(e) => Err(super::sdk_error(e)),
        }
    }

    async fn create_table(&self, def: &TableDef) -> Result<(), MasterError> {
        let attr = |name: &str, kind: &ScalarAttributeType| {
            AttributeDefinition::builder()
                .attribute_name(name)
                .attribute_type(kind.clone())
                .build()
                .map_err(build_error)
        };
        let key = |name: &str, kind: KeyType| {
            KeySchemaElement::builder()
                .attribute_name(name)
                .key_type(kind)
                .build()
                .map_err(build_error)
        };

        let mut attrs = vec![attr(def.hash.0, &def.hash.1)?];
        let mut keys = vec![key(def.hash.0, KeyType::Hash)?];
        if let Some((name, kind)) = &def.range {
            attrs.push(attr(name, kind)?);
            keys.push(key(name, KeyType::Range)?);
        }

        let mut request = self
            .client
            .create_table()
            .table_name(&def.name)
            .billing_mode(BillingMode::PayPerRequest);

        if let Some((index, hash, range)) = &def.gsi {
            attrs.push(attr(hash.0, &hash.1)?);
            attrs.push(attr(range.0, &range.1)?);
            let gsi = GlobalSecondaryIndex::builder()
                .index_name(*index)
                .key_schema(key(hash.0, KeyType::Hash)?)
                .key_schema(key(range.0, KeyType::Range)?)
                .projection(
                    Projection::builder()
                        .projection_type(ProjectionType::All)
                        .build(),
                )
                .build()
                .map_err(build_error)?;
            request = request.global_secondary_indexes(gsi);
        }

        let result = request
            .set_attribute_definitions(Some(attrs))
            .set_key_schema(Some(keys))
            .send()
            .await;
        match result {
            Ok(_) => Ok(()),
            // The other master won the race between describe and create.
            Err(SdkError::ServiceError(err))
                if matches!(err.err(), CreateTableError::ResourceInUseException(_)) =>
            {
                Ok(())
            }
            Err(e) => Err(super::sdk_error(e)),
        }
    }

    async fn wait_active(&self, name: &str) -> Result<(), MasterError> {
        // dynamodb-local answers ACTIVE at once; on AWS it takes a few seconds.
        for _ in 0..60 {
            let out = self
                .client
                .describe_table()
                .table_name(name)
                .send()
                .await
                .map_err(super::sdk_error)?;
            let status = out.table().and_then(|t| t.table_status().cloned());
            if status == Some(TableStatus::Active) {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        Err(MasterError::Db(format!("table {name} never became ACTIVE")))
    }
}
