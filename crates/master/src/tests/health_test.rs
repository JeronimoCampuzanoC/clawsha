use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

use rand::SeedableRng;
use rand::rngs::StdRng;
use tonic::Status;

use crate::cluster::{ClusterState, Heartbeat, WorkerInfo};
use crate::health::{Healer, MAX_STREAMS_PER_WORKER, REPAIR_GRACE_MS, plan_repairs};
use crate::pb;
use crate::workers::WorkerClient;

const S: u64 = 1_000; // one second, in ms
const FENCING: u64 = 7;

fn worker(id: &str, az: &str, last_heartbeat: u64) -> WorkerInfo {
    WorkerInfo {
        id: id.to_string(),
        addr: format!("{id}:50052"),
        az: az.to_string(),
        quota_bytes: 1_000,
        used_bytes: 0,
        data_dir: "/data".to_string(),
        last_heartbeat,
    }
}

fn hb(id: &str, az: &str, blocks: &[&str]) -> Heartbeat {
    Heartbeat {
        worker_id: id.to_string(),
        addr: format!("{id}:50052"),
        az: az.to_string(),
        quota_bytes: 1_000,
        used_bytes: 0,
        new_blocks: blocks.iter().map(|b| b.to_string()).collect(),
        removed_blocks: vec![],
        data_dir: "/data".to_string(),
    }
}

fn rng() -> StdRng {
    StdRng::seed_from_u64(1)
}

// --- plan_repairs: the rules --------------------------------------------------

#[test]
fn copies_from_the_survivor_to_the_az_that_lost_its_copy() {
    let survivor = worker("w1", "a", 0);
    let candidates = vec![survivor.clone(), worker("w2", "a", 0), worker("w3", "b", 0)];

    let repairs = plan_repairs(
        vec![("b1".into(), vec![survivor.clone()])],
        &candidates,
        &HashMap::new(),
        S,
        &mut rng(),
    );
    assert_eq!(repairs.len(), 1);
    assert_eq!(repairs[0].source.id, "w1");
    assert_eq!(
        repairs[0].target.id, "w3",
        "the AZ without a copy wins over w2"
    );
}

#[test]
fn falls_back_to_the_same_az_when_the_other_has_no_room() {
    // Three workers, the one in AZ b died: the copy stays in AZ a.
    let survivor = worker("w1", "a", 0);
    let candidates = vec![survivor.clone(), worker("w2", "a", 0)];

    let repairs = plan_repairs(
        vec![("b1".into(), vec![survivor])],
        &candidates,
        &HashMap::new(),
        S,
        &mut rng(),
    );
    assert_eq!(repairs.len(), 1);
    assert_eq!(repairs[0].target.id, "w2");
}

#[test]
fn never_targets_a_worker_that_already_has_the_block() {
    let survivor = worker("w1", "a", 0);
    let repairs = plan_repairs(
        vec![("b1".into(), vec![survivor.clone()])],
        &[survivor],
        &HashMap::new(),
        S,
        &mut rng(),
    );
    assert!(repairs.is_empty(), "nowhere to copy to: wait, do not fail");
}

#[test]
fn prefers_an_alive_source_over_a_suspect_one() {
    let now = 15 * S;
    let suspect = worker("slow", "a", 0); // 15 s of silence
    let alive = worker("fast", "b", now);
    let candidates = vec![worker("w3", "c", now)];

    let repairs = plan_repairs(
        vec![("b1".into(), vec![suspect, alive])],
        &candidates,
        &HashMap::new(),
        now,
        &mut rng(),
    );
    assert_eq!(repairs[0].source.id, "fast");
}

#[test]
fn a_suspect_source_is_used_if_it_is_the_only_copy() {
    let now = 15 * S;
    let repairs = plan_repairs(
        vec![("b1".into(), vec![worker("slow", "a", 0)])],
        &[worker("w2", "b", now)],
        &HashMap::new(),
        now,
        &mut rng(),
    );
    assert_eq!(repairs[0].source.id, "slow");
}

#[test]
fn caps_the_copies_each_worker_sends_at_once() {
    let survivor = worker("w1", "a", 0);
    let needy = (0..5)
        .map(|i| (format!("b{i}"), vec![survivor.clone()]))
        .collect();
    let candidates = vec![
        worker("w2", "b", 0),
        worker("w3", "b", 0),
        worker("w4", "b", 0),
    ];

    let repairs = plan_repairs(needy, &candidates, &HashMap::new(), S, &mut rng());
    assert_eq!(repairs.len(), MAX_STREAMS_PER_WORKER);
}

#[test]
fn counts_copies_already_in_flight() {
    let survivor = worker("w1", "a", 0);
    let busy = HashMap::from([("w1".to_string(), MAX_STREAMS_PER_WORKER)]);
    let repairs = plan_repairs(
        vec![("b1".into(), vec![survivor])],
        &[worker("w2", "b", 0)],
        &busy,
        S,
        &mut rng(),
    );
    assert!(repairs.is_empty());
}

#[test]
fn blocks_with_fewer_copies_go_first() {
    // R = 3 to have two levels of urgency: b-one has 1 copy, b-two has 2.
    let w1 = worker("w1", "a", 0);
    let w2 = worker("w2", "b", 0);
    let busy = HashMap::from([("w9".to_string(), MAX_STREAMS_PER_WORKER - 1)]);
    let repairs = plan_repairs(
        vec![
            ("b-two".into(), vec![w1.clone(), w2.clone()]),
            ("b-one".into(), vec![w2]),
        ],
        // A single target with room for one copy: only one block gets it.
        &[worker("w9", "c", 0)],
        &busy,
        S,
        &mut rng(),
    );
    assert_eq!(repairs.len(), 1);
    assert_eq!(repairs[0].block_id, "b-one");
}

// --- Healer: the pass against a fake worker ---------------------------------

#[derive(Clone, Default)]
struct FakeWorkers {
    /// Every ReplicateBlock sent, with the worker it was sent to.
    calls: Arc<Mutex<Vec<(String, pb::ReplicateBlockRequest)>>>,
    /// Answers to give, in order; Ok(16) once they run out.
    answers: Arc<Mutex<VecDeque<Result<u64, Status>>>>,
}

impl FakeWorkers {
    fn answer(&self, result: Result<u64, Status>) {
        self.answers.lock().unwrap().push_back(result);
    }

    fn calls(&self) -> Vec<(String, pb::ReplicateBlockRequest)> {
        self.calls.lock().unwrap().clone()
    }
}

impl WorkerClient for FakeWorkers {
    async fn replicate(
        &self,
        worker_addr: &str,
        request: pb::ReplicateBlockRequest,
    ) -> Result<u64, Status> {
        self.calls
            .lock()
            .unwrap()
            .push((worker_addr.to_string(), request));
        self.answers.lock().unwrap().pop_front().unwrap_or(Ok(16))
    }

    async fn delete(
        &self,
        _worker_addr: &str,
        _request: pb::DeleteBlockRequest,
    ) -> Result<u64, Status> {
        unreachable!("the health module never deletes")
    }
}

/// Three workers as in the local compose: w1 and w3 in AZ a, w2 in AZ b.
/// b1 lives on w1 and w2; at 31 s w2 is dead and the others keep talking.
fn cluster_where_w2_died() -> Arc<ClusterState> {
    let cluster = Arc::new(ClusterState::new());
    cluster.heartbeat(hb("w1", "a", &["b1"]), 0);
    cluster.heartbeat(hb("w2", "b", &["b1"]), 0);
    cluster.heartbeat(hb("w3", "a", &[]), 0);
    for t in [9, 18, 27, 36, 45] {
        cluster.heartbeat(hb("w1", "a", &[]), t * S);
        cluster.heartbeat(hb("w3", "a", &[]), t * S);
    }
    cluster
}

fn healer(cluster: &Arc<ClusterState>, workers: &FakeWorkers) -> Arc<Healer<FakeWorkers>> {
    Arc::new(Healer::new(cluster.clone(), workers.clone(), 2, 16))
}

async fn wait(handles: Vec<tokio::task::JoinHandle<()>>) {
    for h in handles {
        h.await.unwrap();
    }
}

#[tokio::test]
async fn a_dead_worker_is_replaced_by_a_copy_on_a_live_one() {
    let cluster = cluster_where_w2_died();
    let workers = FakeWorkers::default();
    let healer = healer(&cluster, &workers);
    let dead_at = 31 * S;

    assert_eq!(cluster.under_replicated(2, dead_at), 1);

    // Just detected: still inside the grace period.
    wait(healer.pass(dead_at, FENCING)).await;
    assert!(workers.calls().is_empty());

    wait(healer.pass(dead_at + REPAIR_GRACE_MS, FENCING)).await;
    let calls = workers.calls();
    assert_eq!(calls.len(), 1);
    let (sent_to, request) = &calls[0];
    assert_eq!(sent_to, "w1:50052", "the order goes to the surviving copy");
    assert_eq!(request.block_id, "b1");
    assert_eq!(request.target, "w3:50052");
    assert_eq!(request.fencing, FENCING);
    assert!(!request.overwrite);

    assert_eq!(cluster.under_replicated(2, dead_at + REPAIR_GRACE_MS), 0);
}

#[tokio::test]
async fn a_copy_in_flight_is_not_ordered_twice() {
    let cluster = cluster_where_w2_died();
    let workers = FakeWorkers::default();
    let healer = healer(&cluster, &workers);
    let now = 31 * S + REPAIR_GRACE_MS;
    healer.pass(31 * S, FENCING);

    // The single-threaded test runtime has not run the first copy yet.
    let first = healer.pass(now, FENCING);
    let second = healer.pass(now, FENCING);
    assert_eq!(first.len(), 1);
    assert!(second.is_empty());
    wait(first).await;
}

#[tokio::test]
async fn a_failed_copy_is_retried_on_the_next_pass() {
    let cluster = cluster_where_w2_died();
    let workers = FakeWorkers::default();
    workers.answer(Err(Status::unavailable("w3 is restarting")));
    let healer = healer(&cluster, &workers);
    let now = 31 * S + REPAIR_GRACE_MS;
    healer.pass(31 * S, FENCING);

    wait(healer.pass(now, FENCING)).await;
    assert_eq!(
        cluster.under_replicated(2, now),
        1,
        "a failure adds no location"
    );

    wait(healer.pass(now + 5 * S, FENCING)).await;
    assert_eq!(workers.calls().len(), 2);
    assert_eq!(cluster.under_replicated(2, now + 5 * S), 0);
}

#[tokio::test]
async fn an_unknown_copy_on_the_target_is_overwritten() {
    let cluster = cluster_where_w2_died();
    let workers = FakeWorkers::default();
    workers.answer(Err(Status::already_exists("block exists")));
    let healer = healer(&cluster, &workers);
    healer.pass(31 * S, FENCING);

    wait(healer.pass(31 * S + REPAIR_GRACE_MS, FENCING)).await;
    let calls = workers.calls();
    assert_eq!(calls.len(), 2);
    assert!(!calls[0].1.overwrite);
    assert!(calls[1].1.overwrite);
    assert_eq!(cluster.under_replicated(2, 31 * S + REPAIR_GRACE_MS), 0);
}

#[tokio::test]
async fn a_block_back_to_r_before_the_grace_ends_is_left_alone() {
    let cluster = cluster_where_w2_died();
    let workers = FakeWorkers::default();
    let healer = healer(&cluster, &workers);
    healer.pass(31 * S, FENCING);

    // w2 comes back and reports b1 again.
    cluster.heartbeat(hb("w2", "b", &[]), 35 * S);
    cluster.block_report("w2", vec!["b1".to_string()]);

    wait(healer.pass(31 * S + REPAIR_GRACE_MS, FENCING)).await;
    assert!(workers.calls().is_empty());
}
