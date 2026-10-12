//! Validation of the paths the gateway sends us.
//!
//! A path is absolute inside the user's own tree, e.g. "/docs/2026/a.pdf".
//! We only ever walk it one component at a time against the `inodes` table,
//! so the only thing that matters here is that every component is a sane
//! directory-entry name. Nothing from here ever touches a real filesystem
//! (blocks live on the workers by UUID), but we still refuse "." and ".." so
//! that nobody can build confusing names.

use crate::error::MasterError;

/// Longest name a single component may have, in bytes. Same limit as ext4.
const MAX_NAME_LEN: usize = 255;

/// Splits an absolute path into its components. "/" gives an empty list.
pub fn parse(path: &str) -> Result<Vec<String>, MasterError> {
    if !path.starts_with('/') {
        return Err(MasterError::InvalidPath(format!(
            "{path:?} must start with '/'"
        )));
    }

    let mut components = Vec::new();
    for raw in path.split('/') {
        // "/docs/" and "//docs" are tolerated: empty pieces are just skipped.
        if raw.is_empty() {
            continue;
        }
        check_name(raw)?;
        components.push(raw.to_string());
    }
    Ok(components)
}

/// Splits a path into (parent components, last name). The root has no parent.
pub fn split_parent(path: &str) -> Result<(Vec<String>, String), MasterError> {
    let mut components = parse(path)?;
    match components.pop() {
        Some(name) => Ok((components, name)),
        None => Err(MasterError::InvalidPath(
            "the root directory has no name".to_string(),
        )),
    }
}

/// One directory-entry name: no dots-only names, no control characters,
/// not longer than MAX_NAME_LEN bytes.
pub fn check_name(name: &str) -> Result<(), MasterError> {
    if name == "." || name == ".." {
        return Err(MasterError::InvalidPath(format!(
            "{name:?} is not a valid name"
        )));
    }
    if name.chars().any(char::is_control) {
        return Err(MasterError::InvalidPath(
            "names cannot contain control characters".to_string(),
        ));
    }
    if name.len() > MAX_NAME_LEN {
        return Err(MasterError::InvalidPath(format!(
            "name longer than {MAX_NAME_LEN} bytes"
        )));
    }
    Ok(())
}
