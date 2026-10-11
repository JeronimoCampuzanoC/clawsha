use crate::auth::{hash_password, validate_password, validate_username, verify_password};
use crate::error::MasterError;

#[test]
fn hash_is_argon2id_in_phc_format() {
    let hash = hash_password("correct horse battery").unwrap();
    assert!(hash.starts_with("$argon2id$"), "got {hash}");
}

#[test]
fn right_password_verifies() {
    let hash = hash_password("correct horse battery").unwrap();
    assert!(verify_password("correct horse battery", &hash));
}

#[test]
fn wrong_password_does_not_verify() {
    let hash = hash_password("correct horse battery").unwrap();
    assert!(!verify_password("correct horse batter", &hash));
}

#[test]
fn garbage_hash_does_not_verify() {
    assert!(!verify_password("anything", "not-a-phc-string"));
}

#[test]
fn same_password_gets_a_different_salt_each_time() {
    let a = hash_password("correct horse battery").unwrap();
    let b = hash_password("correct horse battery").unwrap();
    assert_ne!(a, b);
}

#[test]
fn short_passwords_are_rejected() {
    assert!(matches!(
        validate_password("1234567"),
        Err(MasterError::InvalidArgument(_))
    ));
    assert!(validate_password("12345678").is_ok());
}

#[test]
fn absurdly_long_passwords_are_rejected() {
    assert!(validate_password(&"x".repeat(129)).is_err());
    assert!(validate_password(&"x".repeat(128)).is_ok());
}

#[test]
fn usernames_follow_simple_rules() {
    for ok in ["jero", "ana.maria", "user_01", "a-b"] {
        assert!(validate_username(ok).is_ok(), "should accept {ok:?}");
    }
    for bad in ["", "ab", "has space", "ñandú", "a/b", &"x".repeat(33)] {
        assert!(
            matches!(validate_username(bad), Err(MasterError::InvalidArgument(_))),
            "should reject {bad:?}"
        );
    }
}
