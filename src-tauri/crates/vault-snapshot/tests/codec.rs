#![allow(clippy::unwrap_used)]
use uuid::Uuid;
use vault_domain::*;
use vault_protocol::SnapshotCommit;
use vault_snapshot::*;

#[test]
fn full_snapshot_digest_context_and_truncation_are_checked() {
    let state = State::new(Uuid::from_u128(1));
    let owner = UserId::new(42).unwrap();
    let encoded = build(
        state,
        owner,
        Uuid::from_u128(2),
        0,
        MessageId::new(10).unwrap(),
    )
    .unwrap();
    let commit = SnapshotCommit {
        vault_id: encoded.snapshot.vault_id,
        snapshot_id: encoded.snapshot.snapshot_id,
        generation: 0,
        barrier_message_id: encoded.snapshot.barrier_message_id,
        snapshot_a_message_id: MessageId::new(20).unwrap(),
        snapshot_b_message_id: MessageId::new(30).unwrap(),
        sha256_compressed: encoded.sha256_compressed,
        sha256_uncompressed: encoded.sha256_uncompressed,
        previous_commit_message_id: None,
    };
    assert_eq!(
        decode(&encoded.compressed, &commit, owner).unwrap(),
        encoded.snapshot
    );
    assert!(
        decode(
            &encoded.compressed[..encoded.compressed.len() - 1],
            &commit,
            owner
        )
        .is_err()
    );
    assert!(decode(&encoded.compressed, &commit, UserId::new(43).unwrap()).is_err());
    let mut wrong = commit.clone();
    wrong.vault_id = Uuid::from_u128(3);
    assert!(decode(&encoded.compressed, &wrong, owner).is_err());
    let mut wrong = commit;
    wrong.generation = 1;
    assert!(decode(&encoded.compressed, &wrong, owner).is_err());
}
