// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc. Licensed under Apache-2.0.
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

//! 对应 Go `subscription_test.go` 的 FlushSubscriber 契约测试。
//! fake cluster 保留了 store/region 拓扑、实时 flush stream、连接错误与空闲超时，
//! 因此测试直接验证异步事件、错误处理、拓扑收敛和轮询补齐语义。

use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use astersql_br_pkg_streamhelper_spans::{Full, NewFullWith, Sorted, Span, Valued};

use crate::basic_lib_for_test::{
    create_fake_cluster, install_subscribe_support, install_subscribe_support_for_random_n,
    many_regions, one_store_failure,
};
use crate::collector::NewClusterCollector;
use crate::flush_subscriber::{NewSubscriber, WithSubscriptionIdleTimeout};
use crate::regioniter::IterateRegion;
use crate::stubs::KeyRange;

/// 从真实 fake flush stream 收集事件，直到全键空间检查点达到 `cp`。
fn collect_checkpoint_spans(
    rx: &Receiver<Valued>,
    cp: u64,
) -> astersql_br_pkg_streamhelper_spans::ValueSortedFull {
    let mut observed = Sorted(NewFullWith(&Full(), 1));
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    while observed.MinValue().unwrap_or(0) < cp {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        assert!(
            !remaining.is_zero(),
            "timed out waiting for checkpoint {cp}"
        );
        let event = rx
            .recv_timeout(remaining.min(Duration::from_millis(100)))
            .unwrap_or_else(|error| panic!("waiting for checkpoint {cp}: {error}"));
        observed.Merge(event);
    }
    observed
}

/// 基本路径：拓扑订阅数等于 store 数；推送后 MinValue 不低于最终 cp。
/// 对应 Go 侧最常见的“全 store 可订阅且 flush 正常”场景。
#[test]
fn test_sub_basic() {
    // 4 store 假集群，开启 region 分裂散射。
    let c = create_fake_cluster(4, true);
    c.cluster
        .split_and_scatter(&["0001", "0002", "0003", "0008", "0009"]);
    // 为全部 store 安装订阅支持标志与客户端钩子。
    install_subscribe_support(&c);
    let mut sub = NewSubscriber(c.clone(), Vec::new());
    let rx = sub.TakeEventsRx().expect("events rx");
    // 按当前 store 列表建立实时 fake stream 订阅。
    sub.UpdateStoreTopology().unwrap();
    assert_eq!(sub.SubscriptionCount(), c.cluster.store_list().len());
    let mut cp = 0u64;
    // 多次 advance+flush，抬高集群检查点。
    for _ in 0..10 {
        cp = c.cluster.advance_checkpoints();
        c.cluster.flush_all();
    }
    // 消费各 store stream 异步转发的 region flush 事件。
    sub.HandleErrors();
    assert!(sub.PendingErrors().is_ok());
    let s = collect_checkpoint_spans(&rx, cp);
    // Clear 释放订阅资源，避免测试间泄漏。
    sub.Clear();
    // 所有推送事件 Value=cp，故最小值应 >= cp。
    assert!(
        s.MinValue().unwrap_or(0) >= cp,
        "min={:?} cp={cp}",
        s.MinValue()
    );
}

/// 注入 GetLogBackupClient 失败后应能恢复，拓扑与推送仍可用。
/// 对应 Go：瞬时取客户端错误不应永久破坏订阅拓扑。
#[test]
fn test_normal_error() {
    let c = create_fake_cluster(4, true);
    c.cluster
        .split_and_scatter(&["0001", "0002", "0003", "0008", "0009"]);
    install_subscribe_support(&c);
    // one_store_failure：对特定 store 的 GetLogBackupClient 返回错误。
    c.cluster.set_on_get_client(Some(one_store_failure()));
    let mut sub = NewSubscriber(c.clone(), Vec::new());
    let rx = sub.TakeEventsRx().expect("events rx");
    sub.UpdateStoreTopology().unwrap();
    assert!(sub.PendingErrors().is_err());
    c.cluster.set_on_get_client(None);
    sub.HandleErrors();
    assert!(sub.PendingErrors().is_ok());
    let mut cp = 0u64;
    for _ in 0..10 {
        cp = c.cluster.advance_checkpoints();
        c.cluster.flush_all();
    }
    let s = collect_checkpoint_spans(&rx, cp);
    sub.Clear();
    // 恢复后推送完整，MinValue 应精确等于 cp。
    assert_eq!(s.MinValue().unwrap_or(0), cp);
}

/// 部分 store 不支持订阅时，Unimplemented 应保留为不可重试错误。
#[test]
fn test_has_failure_stores() {
    let c = create_fake_cluster(4, true);
    c.cluster
        .split_and_scatter(&["0001", "0002", "0003", "0008", "0009"]);
    // 仅随机 3 个 store 安装订阅支持，留下 1 个 unsupported。
    install_subscribe_support_for_random_n(&c, 3);
    let unsupported: Vec<_> = c
        .cluster
        .store_list()
        .into_iter()
        .filter(|s| !s.supports_sub)
        .collect();
    assert_eq!(unsupported.len(), 1);
    let mut sub = NewSubscriber(c.clone(), Vec::new());
    sub.UpdateStoreTopology().unwrap();
    assert_eq!(sub.SubscriptionCount(), 4);
    let error = sub.PendingErrors().unwrap_err();
    assert!(error.to_ascii_lowercase().contains("unimplemented"));
    sub.HandleErrors();
    assert!(sub.PendingErrors().is_err(), "unsupported is not retryable");
}

/// store 客户端暂时不可用时拓扑仍可建立；恢复后 GetLogBackupClient 成功。
/// 不强制断言订阅计数变化，重点验证钩子开关对取客户端的影响。
#[test]
fn test_store_offline() {
    let c = create_fake_cluster(4, true);
    c.cluster
        .split_and_scatter(&["0001", "0002", "0003", "0008", "0009"]);
    install_subscribe_support(&c);
    // 模拟“部分数据逃离数据集”类瞬断错误文案（与 Go 测试风格一致）。
    c.cluster.set_on_get_client(Some(Arc::new(|_| {
        Err("upon an eclipsed night, some of data (not all data) have fled from the dataset".into())
    })));
    let mut sub = NewSubscriber(c.clone(), Vec::new());
    sub.UpdateStoreTopology().unwrap();
    assert!(sub.PendingErrors().is_err());
    c.cluster.set_on_get_client(None);
    sub.HandleErrors();
    assert!(sub.PendingErrors().is_ok());
}

/// 移除一个 store 后拓扑订阅数减一，剩余 region 推送仍可达最终 cp。
/// 模拟运维缩容：UpdateStoreTopology 必须收敛到存活 store 集合。
#[test]
fn test_store_removed() {
    let c = create_fake_cluster(4, true);
    // 更多切分点，保证删除 store 后仍有足够 region 覆盖。
    c.cluster.split_and_scatter(&[
        "0001", "0002", "0003", "0008", "0009", "0010", "0100", "0956", "1000",
    ]);
    install_subscribe_support(&c);
    let mut sub = NewSubscriber(c.clone(), Vec::new());
    let rx = sub.TakeEventsRx().expect("events rx");
    sub.UpdateStoreTopology().unwrap();
    let before = sub.SubscriptionCount();
    let mut cp = 0u64;
    for _ in 0..10 {
        cp = c.cluster.advance_checkpoints();
        c.cluster.flush_all();
    }
    // 删除首个 store，期望订阅计数下降。
    let removed = c.cluster.store_list()[0].id;
    c.cluster.remove_store(removed);
    // 再次拓扑同步应摘掉已删除 store 的订阅。
    sub.UpdateStoreTopology().unwrap();
    assert_eq!(sub.SubscriptionCount(), before - 1);
    for _ in 0..10 {
        cp = c.cluster.advance_checkpoints();
        c.cluster.flush_all();
    }
    sub.HandleErrors();
    assert!(sub.PendingErrors().is_ok());
    let s = collect_checkpoint_spans(&rx, cp);
    sub.Clear();
    assert!(s.MinValue().unwrap_or(0) >= cp);
}

/// 仅支持订阅的 Leader region 被推送；落后区间再经 ClusterCollector 补齐到 cp。
/// 对应 Go：unsupported store 上的 region 依赖轮询收集而非 flush 事件。
#[test]
fn test_some_of_store_unsupported() {
    let c = create_fake_cluster(4, true);
    c.cluster.split_and_scatter(&[
        "0001", "0002", "0003", "0008", "0009", "0010", "0100", "0956", "1000",
    ]);
    let mut sub = NewSubscriber(c.clone(), Vec::new());
    install_subscribe_support_for_random_n(&c, 3);
    let rx = sub.TakeEventsRx().expect("events rx");
    sub.UpdateStoreTopology().unwrap();
    // 统计 Leader 落在支持订阅 store 上的 region，作为每次 flush 期望事件数。
    let supported: std::collections::HashSet<u64> = c
        .cluster
        .store_list()
        .into_iter()
        .filter(|s| s.supports_sub)
        .map(|s| s.id)
        .collect();
    let expected_events_per_flush = c
        .cluster
        .region_list()
        .into_iter()
        .filter(|r| supported.contains(&r.leader))
        .count();
    const FLUSH_ROUNDS: usize = 10;
    let mut cp = 0u64;
    for _ in 0..FLUSH_ROUNDS {
        cp = c.cluster.advance_checkpoints();
        c.cluster.flush_all();
    }
    let mut s = Sorted(NewFullWith(&Full(), 1));
    let regions: Vec<_> = c
        .cluster
        .region_list()
        .into_iter()
        .filter(|r| supported.contains(&r.leader))
        .map(|r| (r.start, r.end))
        .collect();
    assert_eq!(regions.len(), expected_events_per_flush);
    for _ in 0..expected_events_per_flush * FLUSH_ROUNDS {
        s.Merge(
            rx.recv_timeout(Duration::from_secs(3))
                .expect("flush event"),
        );
    }
    sub.Clear();

    // Value < cp 的区间视为尚未被订阅覆盖，需走 collector 轮询补齐。
    // 初值 1 的空隙也会出现在 LessThan(cp) 结果中。
    let mut rngs = Vec::new();
    s.TraverseValuesLessThan(cp, |v| {
        rngs.push(v.Key.clone());
        true
    });
    // 串行化 hook，避免并发 Merge 干扰断言。
    let m = Mutex::new(());
    let mut coll = NewClusterCollector(c.clone());
    let s_shared = Arc::new(Mutex::new(s));
    let s_hook = s_shared.clone();
    // 成功收集到的检查点写回同一值序集合。
    coll.SetOnSuccessHook(Arc::new(move |u, kr: KeyRange| {
        let _g = m.lock().unwrap();
        s_hook.lock().unwrap().Merge(Valued {
            Key: Span {
                StartKey: kr.StartKey,
                EndKey: kr.EndKey,
            },
            Value: u,
        });
    }));
    let mut ld = 0u64;
    for rng in rngs {
        // 按落后 span 迭代 region，并收集其检查点。
        let mut iter = IterateRegion(c.as_ref(), &rng.StartKey, &rng.EndKey);
        while !iter.Done() {
            let rs = iter.Next().unwrap();
            for r in rs {
                // 落后区间的 Leader 应来自同一“未支持订阅”的 store。
                // 若混入已支持 store，说明有事件未被 Push，覆盖缺口归因错误。
                if ld == 0 {
                    ld = r.Leader.StoreId;
                } else {
                    assert_eq!(
                        r.Leader.StoreId, ld,
                        "the leader is from different store: some of events not pushed"
                    );
                }
                coll.CollectRegion(r).unwrap();
            }
        }
    }
    // Finish 冲刷 collector 缓冲，触发 OnSuccessHook。
    let _ = coll.Finish().unwrap();
    // 补齐后全局最小检查点应达到 cp。
    assert_eq!(s_shared.lock().unwrap().MinValue().unwrap_or(0), cp);
}

/// 后台 stream 错误应进入 PendingErrors，HandleErrors 应能清除并重连。
#[test]
fn test_encounter_error() {
    let c = create_fake_cluster(4, true);
    c.cluster.split_and_scatter(&[
        "0001", "0002", "0003", "0008", "0009", "0010", "0100", "0956", "1000",
    ]);
    install_subscribe_support(&c);
    let mut sub = NewSubscriber(
        c.clone(),
        vec![WithSubscriptionIdleTimeout(Duration::from_millis(20))],
    );
    sub.UpdateStoreTopology().unwrap();
    std::thread::sleep(Duration::from_millis(80));
    assert!(sub.PendingErrors().is_err());
    sub.HandleErrors();
    assert!(sub.PendingErrors().is_ok());
}

/// 配置空闲超时时，addSubscription 路径会先 ClearCache 再重试。
/// 单 store 集群使 ClearCache 目标唯一，便于断言 store_id。
#[test]
fn test_subscription_idle_timeout_clears_cache_before_retry() {
    let c = create_fake_cluster(1, true);
    install_subscribe_support(&c);
    let (tx, rx) = mpsc::channel();
    // 钩住 ClearCache，确认被调用的 store_id。
    c.cluster.set_on_clear_cache(Some(Arc::new(move |store_id| {
        let _ = tx.send(store_id);
        Ok(())
    })));
    // 200ms 空闲超时：触发重连前清缓存的行为。
    let mut sub = NewSubscriber(
        c.clone(),
        vec![WithSubscriptionIdleTimeout(Duration::from_millis(200))],
    );
    sub.UpdateStoreTopology().unwrap();
    std::thread::sleep(Duration::from_millis(250));
    let error = sub.PendingErrors().unwrap_err();
    assert!(error.contains("no activity"), "{error}");
    sub.HandleErrors();
    assert!(sub.PendingErrors().is_ok());
    let cleared = rx
        .recv_timeout(Duration::from_secs(3))
        .expect("cleared cache");
    assert_eq!(cleared, c.cluster.store_list()[0].id);
    let cp = c.cluster.advance_checkpoints();
    c.cluster.flush_all();
    let events = sub.TakeEventsRx().expect("events rx");
    let s = collect_checkpoint_spans(&events, cp);
    assert_eq!(s.MinValue().unwrap_or(0), cp);
    sub.Clear();
}

/// 大拓扑发送事件后，在截止时间内轮询并报告订阅空闲超时。
#[test]
fn test_subscription_idle_timeout_while_sending_events() {
    let c = create_fake_cluster(4, true);
    // 大量 region 切分，确保订阅拓扑非平凡。
    // many_regions 生成密集键前缀，逼近 Go 压力用例规模。
    let keys: Vec<String> = many_regions(0, 1500);
    let key_refs: Vec<&str> = keys.iter().map(|s| s.as_str()).collect();
    c.cluster.split_and_scatter(&key_refs);
    install_subscribe_support(&c);
    let mut sub = NewSubscriber(
        c.clone(),
        vec![WithSubscriptionIdleTimeout(Duration::from_millis(200))],
    );
    sub.UpdateStoreTopology().unwrap();
    c.cluster.advance_checkpoints();
    c.cluster.flush_all();
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    loop {
        let last_error = sub.PendingErrors().err();
        if last_error
            .as_ref()
            .is_some_and(|error| error.contains("has no activity"))
        {
            break;
        }
        assert!(
            std::time::Instant::now() <= deadline,
            "pending errors did not contain {:?} within {:?}; last error: {:?}",
            "has no activity",
            Duration::from_secs(3),
            last_error
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    sub.Drop();
}
