//! P7.9 warm worker pool — integration tests (spawns the real `mock-worker`
//! binary; `CARGO_BIN_EXE_mock-worker` is only set for integration tests).

use agentcowork_core::worker_pool::WorkerPool;

fn worker_bin() -> String {
    std::env::var("CARGO_BIN_EXE_mock-worker")
        .unwrap_or_else(|_| format!("{}/debug/mock-worker", env!("CARGO_MANIFEST_DIR")))
}

#[test]
fn pre_spawns_warm_workers_with_profile_flags() {
    let mut pool = WorkerPool::spawn(&worker_bin(), "worker", Some("/tmp/w-scratch"), 2).unwrap();
    assert_eq!(pool.size(), 2);
    // Both pre-spawned workers report ready with their profile applied.
    let mut got = Vec::new();
    for w in &mut pool.workers {
        assert!(w.is_alive());
        got.push(format!(
            "{}:{}",
            w.profile,
            w.scratch.clone().unwrap_or_default()
        ));
    }
    got.sort();
    assert_eq!(got, vec!["worker:/tmp/w-scratch", "worker:/tmp/w-scratch"]);
    pool.shutdown();
}

#[test]
fn assigns_on_demand_and_releases() {
    let mut pool = WorkerPool::spawn(&worker_bin(), "worker", None, 2).unwrap();
    let w1 = pool.acquire("worker", None).unwrap().unwrap();
    let id1 = w1.id;
    assert!(w1.busy);
    // Second acquire takes the other worker.
    let w2 = pool.acquire("worker", None).unwrap().unwrap();
    assert_ne!(id1, w2.id);
    // All busy → None.
    assert!(pool.acquire("worker", None).unwrap().is_none());
    // Release frees a slot.
    pool.release(id1);
    let w3 = pool.acquire("worker", None).unwrap().unwrap();
    assert_eq!(w3.id, id1);
    // The assigned worker actually runs jobs.
    let reply = w3.run("job:convert").unwrap();
    assert!(reply.starts_with("ack:job:convert"), "got {reply}");
    pool.shutdown();
}

#[test]
fn grows_the_pool() {
    let mut pool = WorkerPool::spawn(&worker_bin(), "worker", None, 1).unwrap();
    pool.grow("worker", None, 2).unwrap();
    assert_eq!(pool.size(), 3);
    pool.shutdown();
}
