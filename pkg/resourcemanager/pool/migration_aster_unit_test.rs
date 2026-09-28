// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// BasePool 迁移对照单测：哨兵错误文案、名称/时间戳语义、并发任务 ID。

use super::{BasePool, ERR_POOL_CLOSED, ERR_POOL_OVERLOAD, ERR_POOL_PARAMS_INVALID};
use std::{
    sync::Arc,
    thread,
    time::{Duration, SystemTime},
};

/// 编译期断言类型同时满足 `Send + Sync`（可跨线程共享）。
fn assert_send_sync<T: Send + Sync>() {}

/// 校验与 Go 侧一致的池关闭/过载/参数非法错误字符串。
#[test]
fn error_messages_match_go_sentinels() {
    assert_eq!(ERR_POOL_CLOSED, "this pool has been closed");
    assert_eq!(
        ERR_POOL_OVERLOAD,
        "the number of concurrency has reached the upper limit and Block is set"
    );
    assert_eq!(ERR_POOL_PARAMS_INVALID, "the pool params are invalid");
}

/// 校验默认空名称、构造时写入调谐时间戳，以及 set_name / set_last_tune_ts。
#[test]
fn new_pool_preserves_go_name_and_timestamp_behavior() {
    assert_send_sync::<BasePool>();

    let before = SystemTime::now();
    let mut pool = BasePool::new();
    let after = SystemTime::now();

    assert_eq!(pool.name(), "");
    assert!(pool.last_tuner_ts() >= before);
    assert!(pool.last_tuner_ts() <= after);

    pool.set_name("ingest".to_owned());
    assert_eq!(pool.name(), "ingest");

    let tuned_at = before.checked_sub(Duration::from_secs(60)).unwrap();
    pool.set_last_tune_ts(tuned_at);
    assert_eq!(pool.last_tuner_ts(), tuned_at);
}

/// 多线程并发生成任务 ID：应从 1 起且全局唯一、无空洞。
#[test]
fn task_ids_are_one_based_and_unique_under_concurrency() {
    const THREADS: usize = 8;
    const IDS_PER_THREAD: usize = 250;

    let pool = Arc::new(BasePool::new());
    let handles: Vec<_> = (0..THREADS)
        .map(|_| {
            let pool = Arc::clone(&pool);
            thread::spawn(move || {
                (0..IDS_PER_THREAD)
                    .map(|_| pool.gen_task_id())
                    .collect::<Vec<_>>()
            })
        })
        .collect();

    let mut ids: Vec<_> = handles
        .into_iter()
        .flat_map(|handle| handle.join().unwrap())
        .collect();
    ids.sort_unstable();

    assert_eq!(
        ids,
        (1..=(THREADS * IDS_PER_THREAD) as u64).collect::<Vec<_>>()
    );
}
