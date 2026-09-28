// Copyright 2026 AsterSQL.
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

// AsterSQL 迁移补充：ppcpuusage CPU 用量合并与并发行为测试。
//
// 覆盖 Reset、sqlID 过滤、Set/Reset、ID 分配起点，以及多线程合并不丢更新。

use super::{CPUUsages, SQLCPUUsages};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

#[test]
/// 校验 CPUUsages::Reset 将两端时间清零。
fn cpu_usages_reset_matches_go() {
    let mut usage = CPUUsages {
        TidbCPUTime: Duration::from_millis(12),
        TikvCPUTime: Duration::from_millis(34),
    };

    usage.Reset();

    assert_eq!(usage, CPUUsages::default());
}

#[test]
/// 校验 TiDB 合并受 sqlID 过滤，而 TiKV 合并不受影响。
fn sql_id_filters_tidb_but_not_tikv_usage() {
    let usages = SQLCPUUsages::default();
    let sql_id = usages.AllocNewSQLID();

    usages.MergeTidbCPUTime(sql_id.wrapping_sub(1), Duration::from_millis(50));
    usages.MergeTidbCPUTime(sql_id, Duration::from_millis(7));
    usages.MergeTikvCPUTime(Duration::from_millis(11));

    assert_eq!(
        usages.GetCPUUsages(),
        CPUUsages {
            TidbCPUTime: Duration::from_millis(7),
            TikvCPUTime: Duration::from_millis(11)
        }
    );
}

#[test]
/// 校验 SetCPUUsages 整体替换，以及 ResetCPUTimes 清零。
fn set_and_reset_replace_both_cpu_times() {
    let usages = SQLCPUUsages::default();
    usages.SetCPUUsages(CPUUsages {
        TidbCPUTime: Duration::from_secs(2),
        TikvCPUTime: Duration::from_secs(3),
    });
    assert_eq!(
        usages.GetCPUUsages(),
        CPUUsages {
            TidbCPUTime: Duration::from_secs(2),
            TikvCPUTime: Duration::from_secs(3)
        }
    );

    usages.ResetCPUTimes();

    assert_eq!(usages.GetCPUUsages(), CPUUsages::default());
}

#[test]
/// 校验 sqlID 从 1 起递增分配。
fn allocation_starts_at_one_and_increments() {
    let usages = SQLCPUUsages::default();

    assert_eq!(usages.AllocNewSQLID(), 1);
    assert_eq!(usages.AllocNewSQLID(), 2);
}

#[test]
/// 校验多线程并发 Merge 后累计值不丢失。
fn concurrent_merges_are_not_lost() {
    const WORKERS: u32 = 8;
    const MERGES_PER_WORKER: u32 = 2_000;

    let usages = Arc::new(SQLCPUUsages::default());
    let sql_id = usages.AllocNewSQLID();
    let mut workers = Vec::new();
    for _ in 0..WORKERS {
        let usages = Arc::clone(&usages);
        workers.push(thread::spawn(move || {
            for _ in 0..MERGES_PER_WORKER {
                usages.MergeTidbCPUTime(sql_id, Duration::from_nanos(1));
                usages.MergeTikvCPUTime(Duration::from_nanos(2));
            }
        }));
    }
    for worker in workers {
        worker.join().expect("worker must finish");
    }

    assert_eq!(
        usages.GetCPUUsages(),
        CPUUsages {
            TidbCPUTime: Duration::from_nanos(u64::from(WORKERS * MERGES_PER_WORKER)),
            TikvCPUTime: Duration::from_nanos(u64::from(WORKERS * MERGES_PER_WORKER * 2)),
        }
    );
}
