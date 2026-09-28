// Copyright 2026 AsterSQL.
// Copyright 2015 PingCAP, Inc.
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

// 事务（txn）辅助逻辑的单元测试。
//
// 覆盖退避（BackOff）时长上界、重试次数耗尽错误，以及内部事务起始时间戳
// （start ts）盒子在并发场景下的最小可见版本查询。

use crate::test_fixtures::MockStorage;
use kv_dependency as kv;
use std::sync::atomic::Ordering;
use std::sync::{Arc, mpsc};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// 验证 BackOff 随重试次数指数增长，并在达到上限后钳制。
#[test]
fn test_back_off() {
    for (count, upper_ms) in [(1, 2), (2, 4), (3, 8), (100_000, 100)] {
        assert!(kv::BackOff(count) <= Duration::from_millis(upper_ms));
    }
}

/// 验证 RunInNewTxn 在可重试错误、不可重试错误与注入故障下均正确失败，且尊重 MaxRetryCnt。
#[test]
fn test_retry_exceed_count_error() {
    // 临时压低最大重试次数，加快耗尽路径；结束时还原。
    let previous = kv::MaxRetryCnt.swap(5, Ordering::SeqCst);
    let ctx = kv::WithInternalSourceType(kv::Context::todo(), kv::InternalTxnOthers);
    let storage = MockStorage::default();

    assert!(kv::RunInNewTxn(&ctx, &storage, true, |_ctx, _txn| Ok(())).is_err());
    assert!(
        kv::RunInNewTxn(&ctx, &storage, true, |_ctx, _txn| {
            Err(kv::ErrTxnRetryable.FastGenByArgs(&[]))
        })
        .is_err()
    );
    assert!(
        kv::RunInNewTxn(&ctx, &storage, true, |_ctx, _txn| {
            Err(kv::errors::New("do not retry"))
        })
        .is_err()
    );

    // 通过 InjectionConfig 注入 Get/Commit 错误，覆盖故障注入存储路径。
    let cfg = Arc::new(kv::InjectionConfig::default());
    let injected = kv::errors::New("foo");
    cfg.SetGetError(Some(injected.clone()));
    cfg.SetCommitError(Some(injected));
    let storage = kv::NewInjectedStore(Box::new(MockStorage::default()), cfg);
    assert!(kv::RunInNewTxn(&ctx, storage.as_ref(), true, |_ctx, _txn| Ok(())).is_err());

    kv::MaxRetryCnt.store(previous, Ordering::SeqCst);
}

/// 验证内部事务 start ts 盒子：活跃事务期间返回其 start ts，结束后回落到 current_min。
#[test]
fn test_inner_txn_start_ts_box() {
    let now = SystemTime::now();
    let now_ms = now.duration_since(UNIX_EPOCH).unwrap().as_millis() as u64;
    // 物理时间左移 18 位近似 TiDB 的 TSO（Timestamp Oracle）编码。
    let start_ts = (now_ms - 1_000) << 18;
    let lower_limit = (now_ms - 2_000) << 18;
    let current_min = now_ms << 18;
    let storage = Arc::new(MockStorage::with_start_ts(start_ts));
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();

    // 工作线程进入事务后阻塞，主线程在其存活期间查询最小内部 start ts。
    let worker_storage = Arc::clone(&storage);
    let worker = std::thread::spawn(move || {
        let ctx = kv::WithInternalSourceType(kv::Context::todo(), kv::InternalTxnOthers);
        kv::RunInNewTxn(&ctx, worker_storage.as_ref(), false, |_ctx, _txn| {
            entered_tx.send(()).unwrap();
            release_rx.recv().unwrap();
            Ok(())
        })
    });

    entered_rx.recv().unwrap();
    assert_eq!(
        start_ts,
        kv::GetMinInnerTxnStartTS(now, lower_limit, current_min)
    );
    release_tx.send(()).unwrap();
    assert!(worker.join().unwrap().is_err());
    assert_eq!(
        current_min,
        kv::GetMinInnerTxnStartTS(now, lower_limit, current_min)
    );

    // Mirror the Go test's lower-bound filtering with several simultaneously
    // active transactions: the oldest timestamp is excluded, so ts1 wins.
    let now = UNIX_EPOCH + Duration::from_secs(1_646_937_300);
    let physical_ms = |seconds: u64| seconds * 1_000 << 18;
    let timestamps = [
        physical_ms(1_646_764_201),
        physical_ms(1_646_937_001),
        physical_ms(1_646_937_243),
        physical_ms(1_646_937_245),
    ];
    let lower_limit = physical_ms(1_646_850_900);
    let current_min = physical_ms(1_646_937_300);
    let mut releases = Vec::new();
    let mut workers = Vec::new();

    for start_ts in timestamps {
        let storage = MockStorage::with_start_ts(start_ts);
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        workers.push(std::thread::spawn(move || {
            let ctx = kv::WithInternalSourceType(kv::Context::todo(), kv::InternalTxnOthers);
            kv::RunInNewTxn(&ctx, &storage, false, |_ctx, _txn| {
                entered_tx.send(()).unwrap();
                release_rx.recv().unwrap();
                Ok(())
            })
        }));
        entered_rx.recv().unwrap();
        releases.push(release_tx);
    }

    assert_eq!(
        timestamps[1],
        kv::GetMinInnerTxnStartTS(now, lower_limit, current_min)
    );
    for release in releases {
        release.send(()).unwrap();
    }
    for worker in workers {
        assert!(worker.join().unwrap().is_err());
    }
    assert_eq!(
        current_min,
        kv::GetMinInnerTxnStartTS(now, lower_limit, current_min)
    );
}
