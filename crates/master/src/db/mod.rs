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

/// Two transactions touching the same item at the same time cancel one of
/// them with TransactionConflict. Every fenced write touches the `leader`
/// row, so under load (a gateway writing 4 blocks in parallel, the 3 s
/// renewal) this happens now and then. Nothing is written when a
/// transaction cancels, so retrying is safe.
const CONFLICT_RETRIES: usize = 3;

/// Why DynamoDB cancelled a fenced transaction, from its CancellationReasons
/// (one per item, in the order sent; item 0 is the fence check).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cancellation {
    /// The fence check failed: another master holds the lease.
    Fenced,
    /// The condition of the caller's i-th item failed.
    ConditionFailed(usize),
    /// Only conflicts with concurrent transactions: retry.
    Conflict,
    Other,
}

pub fn classify_cancellation(codes: &[Option<&str>]) -> Cancellation {
    if let Some(i) = codes
        .iter()
        .position(|c| *c == Some("ConditionalCheckFailed"))
    {
        return if i == 0 {
            Cancellation::Fenced
        } else {
            Cancellation::ConditionFailed(i - 1)
        };
    }
    if codes.contains(&Some("TransactionConflict")) {
        return Cancellation::Conflict;
    }
    Cancellation::Other
}

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
                .credentials_provider(Credentials::new(
                    "local",
                    "local",
                    None,
                    None,
                    "dfsha-local",
                ));
        }
        let shared = loader.load().await;

        Ok(Db {
            client: Client::new(&shared),
            tables: Arc::new(Tables::new(table_prefix)),
        })
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

        for attempt in 1..=CONFLICT_RETRIES {
            let result = self
                .client
                .transact_write_items()
                .set_transact_items(Some(all.clone()))
                .send()
                .await;

            let cancelled = match result {
                Ok(_) => return Ok(()),
                Err(SdkError::ServiceError(err)) => match err.into_err() {
                    TransactWriteItemsError::TransactionCanceledException(c) => c,
                    other => return Err(MasterError::Db(other.to_string())),
                },
                Err(other) => return Err(MasterError::Db(other.to_string())),
            };

            let codes: Vec<Option<&str>> = cancelled
                .cancellation_reasons()
                .iter()
                .map(|r| r.code())
                .collect();
            match classify_cancellation(&codes) {
                Cancellation::Fenced => return Err(MasterError::Fenced),
                Cancellation::ConditionFailed(i) => return Err(MasterError::ConditionFailed(i)),
                Cancellation::Conflict if attempt < CONFLICT_RETRIES => {
                    // A little random wait so the two sides do not collide again.
                    let wait = 20 + rand::random_range(0..60u64);
                    tokio::time::sleep(std::time::Duration::from_millis(wait)).await;
                }
                Cancellation::Conflict => {
                    return Err(MasterError::Db(
                        "transaction conflict after retries, try again".to_string(),
                    ));
                }
                Cancellation::Other => {
                    return Err(MasterError::Db(format!(
                        "transaction cancelled: {}",
                        codes
                            .iter()
                            .flatten()
                            .copied()
                            .collect::<Vec<_>>()
                            .join(",")
                    )));
                }
            }
        }
        unreachable!("the loop returns on every path")
    }
}

/// The SDK builders fail only when a required field is missing, which would
/// be a bug here, but we keep it as an error instead of a panic.
pub(crate) fn build_error(e: aws_sdk_dynamodb::error::BuildError) -> MasterError {
    MasterError::Db(format!("bad request: {e}"))
}

pub(crate) fn sdk_error<E: std::fmt::Display, R: std::fmt::Debug>(
    e: SdkError<E, R>,
) -> MasterError {
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
