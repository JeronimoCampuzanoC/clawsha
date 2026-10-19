use super::*;

const HEX_KEY: &str = "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f";

fn service() -> CipherService {
    CipherService::new(HEX_KEY).expect("valid hex key")
}

#[test]
fn round_trip_recovers_plaintext() {
    let svc = service();
    let block_id = "11111111-1111-1111-1111-111111111111";
    let plaintext = Bytes::from_static(b"the quick brown fox jumps over the lazy dog");

    let ciphertext = svc.encrypt(block_id, plaintext.clone());
    let recovered = svc.decrypt(block_id, ciphertext);

    assert_eq!(recovered, plaintext);
}

#[test]
fn ciphertext_differs_from_plaintext() {
    let svc = service();
    let block_id = "22222222-2222-2222-2222-222222222222";
    let plaintext = Bytes::from_static(b"sensitive data that must not be stored in the clear");

    let ciphertext = svc.encrypt(block_id, plaintext.clone());

    assert_ne!(ciphertext, plaintext);
}

#[test]
fn preserves_length() {
    let svc = service();
    let block_id = "33333333-3333-3333-3333-333333333333";
    let plaintext = Bytes::from(vec![0xABu8; 16 * 1024 * 1024]); // one full 16 MiB block

    let ciphertext = svc.encrypt(block_id, plaintext.clone());

    assert_eq!(ciphertext.len(), plaintext.len());
}

#[test]
fn different_block_ids_produce_different_ciphertext() {
    let svc = service();
    let plaintext = Bytes::from_static(b"same content, different blocks");

    let a = svc.encrypt("aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa", plaintext.clone());
    let b = svc.encrypt("bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb", plaintext.clone());

    // Same plaintext under different nonces must not collide.
    assert_ne!(a, b);
}

#[test]
fn empty_block_round_trips() {
    let svc = service();
    let block_id = "44444444-4444-4444-4444-444444444444";
    let plaintext = Bytes::new();

    let ciphertext = svc.encrypt(block_id, plaintext.clone());
    let recovered = svc.decrypt(block_id, ciphertext.clone());

    assert_eq!(ciphertext.len(), 0);
    assert_eq!(recovered, plaintext);
}

#[test]
fn accepts_base64_key() {
    // Same 32 bytes as HEX_KEY, base64-encoded.
    let b64 = "AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8=";
    let svc = CipherService::new(b64).expect("valid base64 key");

    let block_id = "55555555-5555-5555-5555-555555555555";
    let plaintext = Bytes::from_static(b"keyed by base64");
    let recovered = svc.decrypt(block_id, svc.encrypt(block_id, plaintext.clone()));
    assert_eq!(recovered, plaintext);
}

#[test]
fn rejects_short_key() {
    let err = CipherService::new("00010203").unwrap_err();
    assert!(matches!(err, CipherError::BadKeyLength(_)));
}

#[test]
fn hex_and_base64_keys_agree() {
    // The hex key and its base64 form must decode to the same bytes, so they
    // produce identical ciphertext for the same block.
    let hex_svc = CipherService::new(HEX_KEY).unwrap();
    let b64_svc = CipherService::new("AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8=").unwrap();

    let block_id = "66666666-6666-6666-6666-666666666666";
    let plaintext = Bytes::from_static(b"agreement check");
    assert_eq!(
        hex_svc.encrypt(block_id, plaintext.clone()),
        b64_svc.encrypt(block_id, plaintext),
    );
}
