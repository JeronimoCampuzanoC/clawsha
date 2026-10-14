use std::time::{SystemTime, UNIX_EPOCH};

use jsonwebtoken::{Algorithm, DecodingKey, EncodingKey, Header, Validation, decode, encode};
use serde::{Deserialize, Serialize};
use thiserror::Error;

const DEFAULT_TOKEN_TTL_SECONDS: u64 = 60 * 60;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Claims {
	pub sub: String,
	pub iat: u64,
	pub exp: u64,
}

#[derive(Debug, Error)]
pub enum JwtError {
	#[error("JWT secret cannot be empty")]
	EmptySecret,
	#[error("could not read system time")]
	SystemTime(#[from] std::time::SystemTimeError),
	#[error("could not encode JWT: {0}")]
	Encode(#[from] jsonwebtoken::errors::Error),
	#[error("could not decode JWT: {0}")]
	Decode(jsonwebtoken::errors::Error),
}

#[derive(Clone)]
pub struct JwtService {
	secret: Vec<u8>,
	ttl_seconds: u64,
}

impl JwtService {
	pub fn new(secret: impl Into<String>) -> Result<Self, JwtError> {
		Self::with_ttl(secret, DEFAULT_TOKEN_TTL_SECONDS)
	}

	pub fn with_ttl(secret: impl Into<String>, ttl_seconds: u64) -> Result<Self, JwtError> {
		let secret = secret.into();
		if secret.is_empty() {
			return Err(JwtError::EmptySecret);
		}

		Ok(Self {
			secret: secret.into_bytes(),
			ttl_seconds,
		})
	}

	pub fn create_token(&self, user_id: impl Into<String>) -> Result<String, JwtError> {
		let issued_at = now_seconds()?;
		let claims = Claims {
			sub: user_id.into(),
			iat: issued_at,
			exp: issued_at.saturating_add(self.ttl_seconds),
		};

		Ok(encode(
			&Header::new(Algorithm::HS256),
			&claims,
			&EncodingKey::from_secret(&self.secret),
		)?)
	}

	pub fn verify_token(&self, token: &str) -> Result<Claims, JwtError> {
		let mut validation = Validation::new(Algorithm::HS256);
		validation.validate_exp = true;
		validation.leeway = 0;

		decode::<Claims>(
			token,
			&DecodingKey::from_secret(&self.secret),
			&validation,
		)
		.map(|data| data.claims)
		.map_err(JwtError::Decode)
	}
}

fn now_seconds() -> Result<u64, JwtError> {
	Ok(SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs())
}

#[cfg(test)]
#[path = "jwt_service_tests.rs"]
mod jwt_service_tests;
