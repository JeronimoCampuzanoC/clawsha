use super::{JwtError, JwtService};

#[test]
fn creates_and_verifies_token() {
    let service = JwtService::with_ttl("test-secret", 60).unwrap();
    let token = service.create_token("user-123").unwrap();

    let claims = service.verify_token(&token).unwrap();

    assert_eq!(claims.sub, "user-123");
    assert!(claims.exp > claims.iat);
}

#[test]
fn rejects_expired_token() {
    let service = JwtService::with_ttl("test-secret", 0).unwrap();
    let token = service.create_token("user-123").unwrap();
    std::thread::sleep(std::time::Duration::from_secs(1));

    assert!(matches!(
        service.verify_token(&token),
        Err(JwtError::Decode(_))
    ));
}

#[test]
fn rejects_token_signed_with_another_secret() {
    let issuer = JwtService::new("issuer-secret").unwrap();
    let verifier = JwtService::new("other-secret").unwrap();
    let token = issuer.create_token("user-123").unwrap();

    assert!(matches!(
        verifier.verify_token(&token),
        Err(JwtError::Decode(_))
    ));
}

#[test]
fn rejects_empty_secret() {
    assert!(matches!(JwtService::new(""), Err(JwtError::EmptySecret)));
}
