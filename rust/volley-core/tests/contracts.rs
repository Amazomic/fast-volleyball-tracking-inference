use volley_core::contracts::{job_id, player_id, sha256};

#[test]
fn opaque_identifiers_reject_uppercase_traversal_and_wrong_lengths() {
    assert!(job_id(&"a".repeat(32)));
    assert!(!job_id(&"A".repeat(32)));
    assert!(!job_id(&"a".repeat(31)));
    assert!(!job_id("../../../../../../../../../../.."));
    assert!(player_id(&"f".repeat(12)));
    assert!(!player_id(&"f".repeat(32)));
    assert!(sha256(&"0".repeat(64)));
    assert!(!sha256(&"g".repeat(64)));
}
