use crate::cluster::WorkerInfo;
use crate::error::MasterError;
use crate::placement::{pick_one, pick_pair};
use rand::SeedableRng;
use rand::rngs::StdRng;

fn worker(id: &str, az: &str, used_pct: u64) -> WorkerInfo {
    WorkerInfo {
        id: id.to_string(),
        addr: format!("{id}:50052"),
        az: az.to_string(),
        quota_bytes: 100,
        used_bytes: used_pct,
        data_dir: String::new(),
        last_heartbeat: 0,
    }
}

#[test]
fn pair_takes_one_worker_from_each_az() {
    let mut rng = StdRng::seed_from_u64(1);
    let workers = vec![
        worker("a1", "a", 0),
        worker("a2", "a", 0),
        worker("b1", "b", 0),
        worker("b2", "b", 0),
    ];

    for _ in 0..50 {
        let (first, second) = pick_pair(&workers, &mut rng).unwrap();
        assert_ne!(first.az, second.az);
        assert_ne!(first.id, second.id);
    }
}

#[test]
fn single_az_still_gives_two_different_hosts() {
    let mut rng = StdRng::seed_from_u64(1);
    let workers = vec![worker("a1", "a", 0), worker("a2", "a", 0)];

    for _ in 0..50 {
        let (first, second) = pick_pair(&workers, &mut rng).unwrap();
        assert_ne!(first.id, second.id);
    }
}

#[test]
fn one_worker_is_not_enough() {
    let mut rng = StdRng::seed_from_u64(1);
    let workers = vec![worker("a1", "a", 0)];

    assert!(matches!(
        pick_pair(&workers, &mut rng),
        Err(MasterError::NoWorkers(_))
    ));
    assert!(matches!(
        pick_pair(&[], &mut rng),
        Err(MasterError::NoWorkers(_))
    ));
}

#[test]
fn emptier_worker_wins_about_sixty_percent_when_usage_differs() {
    let mut rng = StdRng::seed_from_u64(7);
    let workers = vec![worker("empty", "a", 10), worker("fuller", "a", 40)];

    let trials = 2_000;
    let empty_wins = (0..trials)
        .filter(|_| pick_one(&workers, &mut rng).unwrap().id == "empty")
        .count();

    // 0.6 of 2000 is 1200; leave room for randomness but rule out 0.5 and 1.0.
    assert!(
        (1_100..1_300).contains(&empty_wins),
        "empty won {empty_wins} of {trials}"
    );
}

#[test]
fn similar_usage_is_a_coin_flip() {
    let mut rng = StdRng::seed_from_u64(7);
    let workers = vec![worker("x", "a", 30), worker("y", "a", 33)];

    let trials = 2_000;
    let x_wins = (0..trials)
        .filter(|_| pick_one(&workers, &mut rng).unwrap().id == "x")
        .count();

    assert!((900..1_100).contains(&x_wins), "x won {x_wins} of {trials}");
}

#[test]
fn pick_one_with_a_single_candidate_returns_it() {
    let mut rng = StdRng::seed_from_u64(1);
    let workers = vec![worker("only", "a", 0)];
    assert_eq!(pick_one(&workers, &mut rng).unwrap().id, "only");
}
