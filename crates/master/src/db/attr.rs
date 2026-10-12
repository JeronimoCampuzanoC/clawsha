//! Small helpers to build and read DynamoDB attribute values without
//! repeating the same match everywhere.

use std::collections::HashMap;

use aws_sdk_dynamodb::primitives::Blob;
use aws_sdk_dynamodb::types::AttributeValue;

use crate::error::MasterError;

pub type Item = HashMap<String, AttributeValue>;

pub fn s(value: &str) -> AttributeValue {
    AttributeValue::S(value.to_string())
}

pub fn n(value: u64) -> AttributeValue {
    AttributeValue::N(value.to_string())
}

pub fn b(value: &[u8]) -> AttributeValue {
    AttributeValue::B(Blob::new(value))
}

pub fn get_s(item: &Item, name: &str) -> Result<String, MasterError> {
    item.get(name)
        .and_then(|v| v.as_s().ok())
        .cloned()
        .ok_or_else(|| missing(name, "string"))
}

pub fn get_n(item: &Item, name: &str) -> Result<u64, MasterError> {
    item.get(name)
        .and_then(|v| v.as_n().ok())
        .and_then(|v| v.parse().ok())
        .ok_or_else(|| missing(name, "number"))
}

/// Like get_n but treats a missing attribute as `default`.
pub fn get_n_or(item: &Item, name: &str, default: u64) -> Result<u64, MasterError> {
    match item.get(name) {
        None => Ok(default),
        Some(_) => get_n(item, name),
    }
}

pub fn get_b(item: &Item, name: &str) -> Result<Vec<u8>, MasterError> {
    item.get(name)
        .and_then(|v| v.as_b().ok())
        .map(|blob| blob.as_ref().to_vec())
        .ok_or_else(|| missing(name, "binary"))
}

fn missing(name: &str, kind: &str) -> MasterError {
    MasterError::Db(format!("item has no {kind} attribute {name:?}"))
}
