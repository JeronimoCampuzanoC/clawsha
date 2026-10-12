use crate::cluster::{ClusterState, Heartbeat, WorkerState};

const S: u64 = 1_000; // one second, in ms

fn hb(id: &str, az: &str) -> Heartbeat {
    Heartbeat {
        worker_id: id.to_string(),
        addr: format!("{id}:50052"),
        az: az.to_string(),
        quota_bytes: 1_000,
        used_bytes: 0,
        new_blocks: vec![],
        removed_blocks: vec![],
        data_dir: "/data".to_string(),
    }
}

fn hb_with_blocks(id: &str, az: &str, blocks: &[&str]) -> Heartbeat {
    let mut h = hb(id, az);
    h.new_blocks = blocks.iter().map(|b| b.to_string()).collect();
    h
}

#[test]
fn first_heartbeat_asks_for_a_full_report() {
    let cluster = ClusterState::new();
    assert!(cluster.heartbeat(hb("w1", "a"), 0).full_report_requested);
    assert!(!cluster.heartbeat(hb("w1", "a"), S).full_report_requested);
}

#[test]
fn heartbeat_registers_the_worker_as_alive() {
    let cluster = ClusterState::new();
    cluster.heartbeat(hb("w1", "a"), 0);

    let workers = cluster.workers(S);
    assert_eq!(workers.len(), 1);
    assert_eq!(workers[0].info.id, "w1");
    assert_eq!(workers[0].info.az, "a");
    assert_eq!(workers[0].state, WorkerState::Alive);
}

#[test]
fn worker_state_degrades_with_silence() {
    let cluster = ClusterState::new();
    cluster.heartbeat(hb("w1", "a"), 0);

    assert_eq!(cluster.workers(9 * S)[0].state, WorkerState::Alive);
    assert_eq!(cluster.workers(10 * S)[0].state, WorkerState::Suspect);
    assert_eq!(cluster.workers(29 * S)[0].state, WorkerState::Suspect);
    assert_eq!(cluster.workers(30 * S)[0].state, WorkerState::Dead);
}

#[test]
fn a_new_heartbeat_revives_a_dead_worker() {
    let cluster = ClusterState::new();
    cluster.heartbeat(hb("w1", "a"), 0);
    cluster.heartbeat(hb("w1", "a"), 60 * S);

    assert_eq!(cluster.workers(61 * S)[0].state, WorkerState::Alive);
}

#[test]
fn incremental_report_adds_locations() {
    let cluster = ClusterState::new();
    cluster.heartbeat(hb_with_blocks("w1", "a", &["b1", "b2"]), 0);
    cluster.heartbeat(hb_with_blocks("w2", "b", &["b1"]), 0);

    let mut where_b1: Vec<String> = cluster
        .locations("b1", S)
        .into_iter()
        .map(|w| w.id)
        .collect();
    where_b1.sort();
    assert_eq!(where_b1, vec!["w1", "w2"]);
    assert_eq!(cluster.locations("b2", S).len(), 1);
    assert!(cluster.locations("nope", S).is_empty());
}

#[test]
fn removed_blocks_drop_the_location() {
    let cluster = ClusterState::new();
    cluster.heartbeat(hb_with_blocks("w1", "a", &["b1"]), 0);

    let mut h = hb("w1", "a");
    h.removed_blocks = vec!["b1".to_string()];
    cluster.heartbeat(h, S);

    assert!(cluster.locations("b1", 2 * S).is_empty());
}

#[test]
fn full_block_report_replaces_what_we_knew() {
    let cluster = ClusterState::new();
    cluster.heartbeat(hb_with_blocks("w1", "a", &["b1", "b2"]), 0);

    cluster.block_report("w1", vec!["b2".to_string(), "b3".to_string()]);

    assert!(cluster.locations("b1", S).is_empty());
    assert_eq!(cluster.locations("b2", S).len(), 1);
    assert_eq!(cluster.locations("b3", S).len(), 1);
}

#[test]
fn block_report_from_unknown_worker_is_kept_until_it_heartbeats() {
    let cluster = ClusterState::new();
    cluster.block_report("w1", vec!["b1".to_string()]);

    // No address yet, so we cannot hand it out as a replica...
    assert!(cluster.locations("b1", S).is_empty());
    // ...but once it says hello the blocks are there.
    cluster.heartbeat(hb("w1", "a"), S);
    assert_eq!(cluster.locations("b1", 2 * S).len(), 1);
}

#[test]
fn commit_acks_add_locations() {
    let cluster = ClusterState::new();
    cluster.heartbeat(hb("w1", "a"), 0);

    cluster.add_location("b9", "w1");

    assert_eq!(cluster.locations("b9", S)[0].id, "w1");
}

#[test]
fn corrupt_replica_is_forgotten() {
    let cluster = ClusterState::new();
    cluster.heartbeat(hb_with_blocks("w1", "a", &["b1"]), 0);
    cluster.heartbeat(hb_with_blocks("w2", "b", &["b1"]), 0);

    cluster.remove_location("b1", "w1");

    let left = cluster.locations("b1", S);
    assert_eq!(left.len(), 1);
    assert_eq!(left[0].id, "w2");
}

#[test]
fn dead_workers_are_not_offered_as_replicas() {
    let cluster = ClusterState::new();
    cluster.heartbeat(hb_with_blocks("w1", "a", &["b1"]), 0);
    cluster.heartbeat(hb_with_blocks("w2", "b", &["b1"]), 25 * S);

    // At t=31s w1 is dead and w2 is still alive.
    let left = cluster.locations("b1", 31 * S);
    assert_eq!(left.len(), 1);
    assert_eq!(left[0].id, "w2");
}

#[test]
fn write_candidates_need_to_be_alive_and_have_room() {
    let cluster = ClusterState::new();
    cluster.heartbeat(hb("fresh", "a"), 0);
    let mut full = hb("full", "a");
    full.used_bytes = 950;
    cluster.heartbeat(full, 0);
    cluster.heartbeat(hb("late", "b"), 0);
    cluster.heartbeat(hb("late", "b"), 0); // last heartbeat at t=0

    // At t=12s "late" is suspect; "full" has only 50 bytes free.
    cluster.heartbeat(hb("fresh", "a"), 11 * S);
    let mut f = hb("full", "a");
    f.used_bytes = 950;
    cluster.heartbeat(f, 11 * S);

    let ids: Vec<String> = cluster
        .write_candidates(12 * S, 100)
        .into_iter()
        .map(|w| w.id)
        .collect();
    assert_eq!(ids, vec!["fresh"]);
}

#[test]
fn under_replicated_counts_blocks_with_a_single_copy() {
    let cluster = ClusterState::new();
    cluster.heartbeat(hb_with_blocks("w1", "a", &["b1", "b2"]), 0);
    cluster.heartbeat(hb_with_blocks("w2", "b", &["b1"]), 0);

    assert_eq!(cluster.total_blocks(), 2);
    assert_eq!(cluster.under_replicated(2), 1);
}
