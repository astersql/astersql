// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

use crate::ddl_workerpool::{JobType, Worker, WorkerPool};

/// 验证 worker 池的关闭语义：关闭后归还被丢弃、关闭幂等。
#[test]
fn test_ddl_worker_pool() {
    let pool = WorkerPool::new(JobType::Reorg, 1);
    assert_eq!(pool.job_type(), JobType::Reorg);
    assert_eq!(pool.available(), 1);
    assert_eq!(pool.borrowed(), 0);

    let worker = pool.get().expect("open pool must not fail").unwrap();
    assert_eq!(worker.id, 0);
    assert_eq!(worker.job_type, JobType::Reorg);
    assert_eq!(pool.available(), 0);
    assert_eq!(pool.borrowed(), 1);

    // Go's ResourcePool.TryGet is non-blocking and returns nil when exhausted.
    assert_eq!(pool.get().expect("exhaustion is not an error"), None);

    pool.put(worker);
    assert_eq!(pool.available(), 1);
    assert_eq!(pool.borrowed(), 0);

    pool.close();
    assert_eq!(pool.available(), 0);
    assert_eq!(pool.borrowed(), 0);
    assert_eq!(pool.get().unwrap_err(), "workerPool is closed");

    // This is the Go regression's put(nil) equivalent: returning a resource
    // after close must not make it available again. Close is also idempotent.
    pool.put(Worker {
        id: 1,
        job_type: JobType::Reorg,
    });
    pool.close();
    assert_eq!(pool.available(), 0);
    assert_eq!(pool.borrowed(), 0);
}
