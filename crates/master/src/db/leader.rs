//! The leader lease, in the `cluster` table under a single row.
//!
//! DynamoDB is the arbiter between the two masters: whoever wins the
//! conditional update owns the row for `ttl` and must renew before it runs
//! out. Every takeover bumps `fencing`; that number travels with every write
//! (see fenced_write) and with every order to the workers, so a master that
//! lost the lease without noticing cannot do damage.
//!
//! These two operations are the only writes that do NOT go through
//! fenced_write: they are the fence.

use aws_sdk_dynamodb::error::SdkError;
use aws_sdk_dynamodb::operation::update_item::UpdateItemError;
use aws_sdk_dynamodb::types::ReturnValue;

use super::attr::{Item, get_n, get_s, n, s};
use super::{Db, Fence, sdk_error};
use crate::error::MasterError;

pub const LEADER_KEY: &str = "leader";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LeaderLease {
    pub holder: String,
    pub addr: String,
    pub fencing: u64,
    pub expires_at: u64,
}

impl Db {
    /// Tries to become leader. Succeeds only if nobody holds the lease or the
    /// current one expired. Returns the new lease (with fencing + 1) or None.
    pub async fn try_acquire_leader(
        &self,
        me: &str,
        addr: &str,
        now_ms: u64,
        ttl_ms: u64,
    ) -> Result<Option<LeaderLease>, MasterError> {
        let result = self
            .client
            .update_item()
            .table_name(self.tables.cluster())
            .key("pk", s(LEADER_KEY))
            .condition_expression("attribute_not_exists(holder) OR expires_at < :now")
            .update_expression(
                "SET holder = :me, addr = :addr, expires_at = :exp, fencing = if_not_exists(fencing, :zero) + :one",
            )
            .expression_attribute_values(":now", n(now_ms))
            .expression_attribute_values(":me", s(me))
            .expression_attribute_values(":addr", s(addr))
            .expression_attribute_values(":exp", n(now_ms + ttl_ms))
            .expression_attribute_values(":zero", n(0))
            .expression_attribute_values(":one", n(1))
            .return_values(ReturnValue::AllNew)
            .send()
            .await;

        match result {
            Ok(out) => match out.attributes() {
                Some(item) => Ok(Some(lease_from_item(item)?)),
                None => Err(MasterError::Db("update returned no attributes".into())),
            },
            Err(SdkError::ServiceError(err))
                if matches!(
                    err.err(),
                    UpdateItemError::ConditionalCheckFailedException(_)
                ) =>
            {
                Ok(None)
            }
            Err(e) => Err(sdk_error(e)),
        }
    }

    /// Extends the lease. False means we are no longer the holder with that
    /// fencing number: step down immediately.
    pub async fn renew_leader(
        &self,
        fence: &Fence,
        now_ms: u64,
        ttl_ms: u64,
    ) -> Result<bool, MasterError> {
        let result = self
            .client
            .update_item()
            .table_name(self.tables.cluster())
            .key("pk", s(LEADER_KEY))
            .condition_expression("holder = :me AND fencing = :f")
            .update_expression("SET expires_at = :exp")
            .expression_attribute_values(":me", s(&fence.holder))
            .expression_attribute_values(":f", n(fence.fencing))
            .expression_attribute_values(":exp", n(now_ms + ttl_ms))
            .send()
            .await;

        match result {
            Ok(_) => Ok(true),
            Err(SdkError::ServiceError(err))
                if matches!(
                    err.err(),
                    UpdateItemError::ConditionalCheckFailedException(_)
                ) =>
            {
                Ok(false)
            }
            Err(e) => Err(sdk_error(e)),
        }
    }

    /// Who holds the lease right now (for tests and the dashboard).
    #[cfg_attr(not(test), allow(dead_code))]
    pub async fn get_leader(&self) -> Result<Option<LeaderLease>, MasterError> {
        let out = self
            .client
            .get_item()
            .table_name(self.tables.cluster())
            .key("pk", s(LEADER_KEY))
            .consistent_read(true)
            .send()
            .await
            .map_err(sdk_error)?;
        out.item().map(lease_from_item).transpose()
    }
}

fn lease_from_item(item: &Item) -> Result<LeaderLease, MasterError> {
    Ok(LeaderLease {
        holder: get_s(item, "holder")?,
        addr: get_s(item, "addr")?,
        fencing: get_n(item, "fencing")?,
        expires_at: get_n(item, "expires_at")?,
    })
}
