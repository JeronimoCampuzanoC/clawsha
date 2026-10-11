//! `users` (by user_id) and `usernames` (by username). Two tables because
//! DynamoDB only enforces uniqueness on the key: the username lives as the
//! key of its own table and both rows are written in one transaction, so two
//! people cannot register the same name.

use aws_sdk_dynamodb::types::{Put, TransactWriteItem};

use super::attr::{Item, get_n, get_s, n, s};
use super::{Db, Fence, build_error, on_condition_failed, sdk_error};
use crate::error::MasterError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserRecord {
    pub user_id: String,
    pub username: String,
    pub password_hash: String,
    pub created_at: u64,
}

impl Db {
    pub async fn create_user(
        &self,
        fence: &Fence,
        user_id: &str,
        username: &str,
        password_hash: &str,
        now_ms: u64,
    ) -> Result<(), MasterError> {
        let user = Put::builder()
            .table_name(self.tables.users())
            .item("user_id", s(user_id))
            .item("username", s(username))
            .item("password_hash", s(password_hash))
            .item("created_at", n(now_ms))
            .condition_expression("attribute_not_exists(user_id)")
            .build()
            .map_err(build_error)?;
        let name = Put::builder()
            .table_name(self.tables.usernames())
            .item("username", s(username))
            .item("user_id", s(user_id))
            .condition_expression("attribute_not_exists(username)")
            .build()
            .map_err(build_error)?;

        self.fenced_write(
            fence,
            vec![
                TransactWriteItem::builder().put(user).build(),
                TransactWriteItem::builder().put(name).build(),
            ],
        )
        .await
        .map_err(|e| on_condition_failed(e, MasterError::AlreadyExists))
    }

    pub async fn find_user_by_username(
        &self,
        username: &str,
    ) -> Result<Option<UserRecord>, MasterError> {
        let out = self
            .client
            .get_item()
            .table_name(self.tables.usernames())
            .key("username", s(username))
            .consistent_read(true)
            .send()
            .await
            .map_err(sdk_error)?;
        let Some(item) = out.item() else {
            return Ok(None);
        };
        let user_id = get_s(item, "user_id")?;
        self.get_user(&user_id).await
    }

    pub async fn get_user(&self, user_id: &str) -> Result<Option<UserRecord>, MasterError> {
        let out = self
            .client
            .get_item()
            .table_name(self.tables.users())
            .key("user_id", s(user_id))
            .consistent_read(true)
            .send()
            .await
            .map_err(sdk_error)?;
        out.item().map(user_from_item).transpose()
    }
}

fn user_from_item(item: &Item) -> Result<UserRecord, MasterError> {
    Ok(UserRecord {
        user_id: get_s(item, "user_id")?,
        username: get_s(item, "username")?,
        password_hash: get_s(item, "password_hash")?,
        created_at: get_n(item, "created_at")?,
    })
}
