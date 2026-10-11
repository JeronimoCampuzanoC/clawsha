//! Where to put the two replicas of a new block.
//!
//! This is HDFS's AvailableSpaceBlockPlacementPolicy with Hadoop's default
//! knobs: take two random candidates and, if their usage differs by more than
//! 5 points, pick the emptier one 60 % of the time. New workers from the ASG
//! arrive at 0 % and win almost every comparison, so they fill up without a
//! balancer; the 0.6 (instead of 1.0) keeps them from becoming a hot spot.
//!
//! The second replica goes to another AZ, so a whole zone can go down and
//! every block still has a copy. If the other AZ has no candidates we settle
//! for a different host in the same AZ rather than refuse the write.

use rand::{Rng, RngExt};

use crate::cluster::WorkerInfo;
use crate::error::MasterError;

/// Hadoop: dfs.namenode.available-space-block-placement-policy.balanced-space-preference-fraction
const PREFER_EMPTIER_PROBABILITY: f64 = 0.6;
/// Hadoop: dfs.namenode.available-space-block-placement-policy.balanced-space-tolerance (percent points)
const USAGE_TOLERANCE_POINTS: f64 = 5.0;

/// Picks one worker out of `candidates` with the AvailableSpace rule.
pub fn pick_one<'a, R: Rng>(
    candidates: &'a [WorkerInfo],
    rng: &mut R,
) -> Result<&'a WorkerInfo, MasterError> {
    match candidates.len() {
        0 => Err(MasterError::NoWorkers("no worker with free space".into())),
        1 => Ok(&candidates[0]),
        n => {
            let i = rng.random_range(0..n);
            // A second index different from the first.
            let j = (i + rng.random_range(1..n)) % n;
            let (a, b) = (&candidates[i], &candidates[j]);

            let diff = a.used_percent() - b.used_percent();
            if diff.abs() <= USAGE_TOLERANCE_POINTS {
                // About the same: either one.
                return Ok(if rng.random_bool(0.5) { a } else { b });
            }
            let (emptier, fuller) = if diff < 0.0 { (a, b) } else { (b, a) };
            Ok(if rng.random_bool(PREFER_EMPTIER_PROBABILITY) {
                emptier
            } else {
                fuller
            })
        }
    }
}

/// Picks the two workers for a block: first anywhere, second preferably in
/// another AZ, never the same host twice.
pub fn pick_pair<R: Rng>(
    candidates: &[WorkerInfo],
    rng: &mut R,
) -> Result<(WorkerInfo, WorkerInfo), MasterError> {
    if candidates.len() < 2 {
        return Err(MasterError::NoWorkers(format!(
            "need 2 workers with free space, have {}",
            candidates.len()
        )));
    }

    let first = pick_one(candidates, rng)?.clone();

    let other_az: Vec<WorkerInfo> = candidates
        .iter()
        .filter(|w| w.az != first.az)
        .cloned()
        .collect();
    let pool = if other_az.is_empty() {
        // Degraded: one AZ is empty or down. Same AZ, different host.
        candidates
            .iter()
            .filter(|w| w.id != first.id)
            .cloned()
            .collect()
    } else {
        other_az
    };

    let second = pick_one(&pool, rng)?.clone();
    Ok((first, second))
}
