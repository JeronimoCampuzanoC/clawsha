//! The usage metric the workers' ASG scales on.
//!
//! Every minute the leader publishes `<namespace>/ClusterUsedPercent`: the
//! bytes used over the quota of every worker that is not dead. Terraform's
//! alarm adds a worker when it stays above the threshold for 2 minutes
//! (infra/modules/workers). Only the leader publishes, so the standby does
//! not halve the average with zeros from its empty map.
//!
//! Off unless MASTER_CLOUDWATCH_NAMESPACE is set: locally there is no
//! CloudWatch to talk to.

use std::sync::Arc;
use std::time::Duration;

use aws_sdk_cloudwatch::types::{MetricDatum, StandardUnit};
use tokio::time::MissedTickBehavior;
use tracing::warn;

use crate::cluster::{ClusterState, WorkerState, WorkerView};
use crate::leader::Leadership;

pub const METRIC_NAME: &str = "ClusterUsedPercent";
/// Matches the alarm's 60 s period.
const PUBLISH_EVERY: Duration = Duration::from_secs(60);

/// Used bytes over quota, 0..=100, across the workers that are not dead.
/// None when there is no capacity at all (no workers yet).
pub fn cluster_used_percent(workers: &[WorkerView]) -> Option<f64> {
    let (used, quota) = workers
        .iter()
        .filter(|w| w.state != WorkerState::Dead)
        .fold((0u64, 0u64), |(used, quota), w| {
            (used + w.info.used_bytes, quota + w.info.quota_bytes)
        });
    (quota > 0).then(|| (used as f64 * 100.0 / quota as f64).min(100.0))
}

pub async fn run(cluster: Arc<ClusterState>, leadership: Arc<Leadership>, namespace: String) {
    // Region and credentials from the environment: AWS_REGION and the
    // instance profile on EC2 (see infra/scripts/master.sh.tftpl).
    let config = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
    let client = aws_sdk_cloudwatch::Client::new(&config);

    let mut interval = tokio::time::interval(PUBLISH_EVERY);
    interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
    loop {
        interval.tick().await;
        let now = crate::time::now_ms();
        if !leadership.is_leader_at(now) || leadership.safe_mode(now) {
            continue;
        }
        let Some(percent) = cluster_used_percent(&cluster.workers(now)) else {
            continue;
        };
        let datum = MetricDatum::builder()
            .metric_name(METRIC_NAME)
            .value(percent)
            .unit(StandardUnit::Percent)
            .build();
        if let Err(e) = client
            .put_metric_data()
            .namespace(&namespace)
            .metric_data(datum)
            .send()
            .await
        {
            warn!(error = %e, namespace, "could not publish {METRIC_NAME}");
        }
    }
}
