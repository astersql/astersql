// Copyright 2026 AsterSQL.

use super::BasePool;
use std::{
    sync::{RwLock, atomic::AtomicU64},
    time::SystemTime,
};

#[test]
fn task_id_wraps_like_go_atomic_uint64() {
    let pool = BasePool {
        last_tune_ts: RwLock::new(SystemTime::now()),
        name: String::new(),
        generator: AtomicU64::new(u64::MAX),
    };

    assert_eq!(pool.gen_task_id(), 0);
    assert_eq!(pool.gen_task_id(), 1);
}
