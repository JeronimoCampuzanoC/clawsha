//! The only door to DynamoDB. Every table, every read and every write of the
//! master goes through here, so the fencing rule lives in one place:
//!
//! > A write only goes through if the leader lease still names us with the
//! > fencing number we hold.
//!
//! That rule is what keeps a master that lost the lease (and does not know it
//! yet) from corrupting the metadata. Each write is a TransactWriteItems whose
//! first item is a ConditionCheck on the lease; DynamoDB rejects the whole
//! transaction if the check fails and we map that to MasterError::Fenced.
//!
//! Other components (the worker-side GC that B writes) do their deletes
//! through this same module for the same reason.

pub mod attr;
pub mod blocks;
pub mod inodes;
pub mod leader;
pub mod leases;
pub mod tables;
pub mod users;
pub mod versions;

use std::sync::Arc;

use aws_sdk_dynamodb::Client;
use aws_sdk_dynamodb::config::{Credentials, Region};
use aws_sdk_dynamodb::error::SdkError;
use aws_sdk_dynamodb::operation::transact_write_items::TransactWriteItemsError;
use aws_sdk_dynamodb::types::{ConditionCheck, TransactWriteItem};

use crate::error::MasterError;
use attr::{n, s};
use tables::Tables;

/// Proof of leadership that travels with every write: who we are and the
/// fencing number the lease had when we took it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fence {
    pub holder: String,
    pub fencing: u64,
}

/// Fenced transactions can carry 100 items; one of them is the fence check.
pub const MAX_ITEMS_PER_WRITE: usize = 99;

#[derive(Clone)]
pub struct Db {
    client: Client,
    tables: Arc<Tables>,
}

impl Db {
    /// With an endpoint (dynamodb-local) we use dummy credentials; without one
    /// the SDK follows the usual chain, which on EC2 is the instance profile.
    pub async fn connect(endpoint: Option<&str>, table_prefix: &str) -> Result<Db, MasterError> {
        let mut loader = aws_config::defaults(aws_config::BehaviorVersion::latest())
            .region(Region::new("us-east-1"));
        if let Some(endpoint) = endpoint {
            loader = loader
                .endpoint_url(endpoint)
                .credentials_provider(Credentials::new("local", "local", None, None, "dfsha-local"));
        }
        let shared = loader.load().await;

        Ok(Db {
            client: Client::new(&shared),
            tables: Arc::new(Tables::new(table_prefix)),
        })
    }

    pub fn tables(&self) -> &Tables {
        &self.tables
    }

    pub(crate) fn client(&self) -> &Client {
        &self.client
    }

    /// Runs `items` as one transaction guarded by the leader fence.
    ///
    /// Errors: Fenced when the lease check fails; ConditionFailed(i) when the
    /// condition of the caller's i-th item fails (callers turn that into their
    /// own error, e.g. AlreadyExists); Db for anything else.
    pub async fn fenced_write(
        &self,
        fence: &Fence,
        items: Vec<TransactWriteItem>,
    ) -> Result<(), MasterError> {
        if items.is_empty() {
            return Ok(());
        }
        if items.len() > MAX_ITEMS_PER_WRITE {
            return Err(MasterError::Db(format!(
                "a fenced write carries at most {MAX_ITEMS_PER_WRITE} items, got {}",
                items.len()
            )));
        }

        let check = ConditionCheck::builder()
            .table_name(self.tables.cluster())
            .key("pk", s(leader::LEADER_KEY))
            .condition_expression("holder = :fence_holder AND fencing = :fence_no")
            .expression_attribute_values(":fence_holder", s(&fence.holder))
            .expression_attribute_values(":fence_no", n(fence.fencing))
            .build()
            .map_err(build_error)?;

        let mut all = Vec::with_capacity(items.len() + 1);
        all.push(TransactWriteItem::builder().condition_check(check).build());
        all.extend(items);

        let result = self
            .client
            .transact_write_items()
            .set_transact_items(Some(all))
            .send()
            .await;

        match result {
            Ok(_) => Ok(()),
            Err(SdkError::ServiceError(err)) => match err.into_err() {
                TransactWriteItemsError::TransactionCanceledException(cancelled) => {
                    // One reason per item, in the same order we sent them.
                    let failed: Vec<usize> = cancelled
                        .cancellation_reasons()
                        .iter()
                        .enumerate()
                        .filter(|(_, r)| r.code() == Some("ConditionalCheckFailed"))
                        .map(|(i, _)| i)
                        .collect();
                    match failed.first() {
                        Some(0) => Err(MasterError::Fenced),
                        Some(i) => Err(MasterError::ConditionFailed(i - 1)),
                        None => Err(MasterError::Db(format!(
                            "transaction cancelled: {}",
                            cancelled
                                .cancellation_reasons()
                                .iter()
                                .filter_map(|r| r.code())
                                .collect::<Vec<_>>()
                                .join(",")
                        ))),
                    }
                }
                other => Err(MasterError::Db(other.to_string())),
            },
            Err(other) => Err(MasterError::Db(other.to_string())),
        }
    }
}

/// The SDK builders fail only when a required field is missing, which would
/// be a bug here, but we keep it as an error instead of a panic.
pub(crate) fn build_error(e: aws_sdk_dynamodb::error::BuildError) -> MasterError {
    MasterError::Db(format!("bad request: {e}"))
}

pub(crate) fn sdk_error<E: std::fmt::Display, R: std::fmt::Debug>(e: SdkError<E, R>) -> MasterError {
    match e {
        SdkError::ServiceError(err) => MasterError::Db(err.err().to_string()),
        other => MasterError::Db(other.to_string()),
    }
}

/// Turns ConditionFailed into the error the caller means, leaves the rest.
pub(crate) fn on_condition_failed(e: MasterError, mapped: MasterError) -> MasterError {
    match e {
        MasterError::ConditionFailed(_) => mapped,
        other => other,
    }
}
