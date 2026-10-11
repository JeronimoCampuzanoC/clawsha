//! Passwords and usernames. The master only stores argon2id hashes; the
//! gateway is the one that mints the JWT once Login says the user_id.

use argon2::password_hash::phc::PasswordHash;
use argon2::{Argon2, PasswordHasher, PasswordVerifier};

use crate::error::MasterError;

const MIN_PASSWORD_LEN: usize = 8;
/// argon2 would happily hash a 10 MB password; the gateway should not send one.
const MAX_PASSWORD_LEN: usize = 128;
const MIN_USERNAME_LEN: usize = 3;
const MAX_USERNAME_LEN: usize = 32;

pub fn validate_username(username: &str) -> Result<(), MasterError> {
    let len = username.chars().count();
    if !(MIN_USERNAME_LEN..=MAX_USERNAME_LEN).contains(&len) {
        return Err(MasterError::InvalidArgument(format!(
            "username must have between {MIN_USERNAME_LEN} and {MAX_USERNAME_LEN} characters"
        )));
    }
    let allowed = |c: char| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-');
    if !username.chars().all(allowed) {
        return Err(MasterError::InvalidArgument(
            "username may only contain letters, digits, '.', '_' and '-'".to_string(),
        ));
    }
    Ok(())
}

pub fn validate_password(password: &str) -> Result<(), MasterError> {
    let len = password.chars().count();
    if len < MIN_PASSWORD_LEN {
        return Err(MasterError::InvalidArgument(format!(
            "password must have at least {MIN_PASSWORD_LEN} characters"
        )));
    }
    if len > MAX_PASSWORD_LEN {
        return Err(MasterError::InvalidArgument(format!(
            "password cannot have more than {MAX_PASSWORD_LEN} characters"
        )));
    }
    Ok(())
}

/// argon2id with the library defaults and a fresh random salt. The result is a
/// PHC string ("$argon2id$v=19$m=...,t=...,p=...$salt$hash") that also records
/// the parameters, so they can be raised later without breaking old users.
pub fn hash_password(password: &str) -> Result<String, MasterError> {
    Argon2::default()
        .hash_password(password.as_bytes())
        .map(|hash| hash.to_string())
        .map_err(|e| MasterError::InvalidArgument(format!("cannot hash password: {e}")))
}

/// False on a wrong password and also on a malformed stored hash: either way
/// the login fails, and the caller does not need to know which.
pub fn verify_password(password: &str, stored: &str) -> bool {
    let Ok(parsed) = PasswordHash::new(stored) else {
        return false;
    };
    Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .is_ok()
}
