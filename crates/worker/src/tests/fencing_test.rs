use tonic::Code;

use crate::fencing::Fencing;

#[test]
fn a_fresh_worker_accepts_token_zero() {
    let fencing = Fencing::new();

    assert!(fencing.check(0).is_ok());
}

#[test]
fn the_first_token_is_adopted() {
    let fencing = Fencing::new();

    fencing.check(7).unwrap();

    assert_eq!(fencing.current(), 7);
}

#[test]
fn the_same_token_is_accepted_again() {
    let fencing = Fencing::new();
    fencing.check(7).unwrap();

    assert!(fencing.check(7).is_ok());
}

#[test]
fn a_newer_token_is_adopted() {
    let fencing = Fencing::new();
    fencing.check(5).unwrap();

    fencing.check(6).unwrap();

    assert_eq!(fencing.current(), 6);
}

#[test]
fn an_older_token_is_rejected_and_does_not_lower_the_current_one() {
    let fencing = Fencing::new();
    fencing.check(5).unwrap();

    let status = fencing.check(4).unwrap_err();

    assert_eq!(status.code(), Code::FailedPrecondition);
    assert_eq!(fencing.current(), 5);
}
