use crate::cluster::{WorkerInfo, WorkerState, WorkerView};
use crate::metrics::cluster_used_percent;

fn view(id: &str, used: u64, quota: u64, state: WorkerState) -> WorkerView {
    WorkerView {
        info: WorkerInfo {
            id: id.to_string(),
            addr: format!("{id}:50052"),
            az: "a".to_string(),
            quota_bytes: quota,
            used_bytes: used,
            data_dir: "/data".to_string(),
            last_heartbeat: 0,
        },
        state,
        block_count: 0,
    }
}

#[test]
fn usage_is_bytes_over_quota_of_the_workers_that_count() {
    let workers = vec![
        view("w1", 600, 1_000, WorkerState::Alive),
        view("w2", 100, 1_000, WorkerState::Suspect),
        // A dead worker's disk is not capacity we can write to.
        view("w3", 0, 1_000, WorkerState::Dead),
    ];
    assert_eq!(cluster_used_percent(&workers), Some(35.0));
}

#[test]
fn no_workers_means_no_metric() {
    assert_eq!(cluster_used_percent(&[]), None);
    let dead = vec![view("w1", 0, 1_000, WorkerState::Dead)];
    assert_eq!(cluster_used_percent(&dead), None);
}
