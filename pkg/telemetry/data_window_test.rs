// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
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

// telemetry 数据窗口相关测试：内置函数用量与 TiFlash 扫描计数。
//
// 覆盖 `GlobalBuiltinFunctionsUsage` 的 Dump/Collect，以及
// [`withMockTiFlash`] 构造的 mock TiFlash store（TiFlash 为列存加速引擎）
// 与窗口计数器（下推、表扫、fast-scan）的累加语义。

use crate::{
    BuiltinFunctionsUsage, BuiltinUsageExt, CurrentTiFlashPushDownCount,
    CurrentTiflashTableScanCount, CurrentTiflashTableScanWithFastScanCount,
    GlobalBuiltinFunctionsUsage,
};
use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, UNIX_EPOCH};

/// 串行化本文件测试，避免全局用量计数器互相干扰。
static TEST_LOCK: Mutex<()> = Mutex::new(());

/// 获取测试互斥锁；毒化时仍取出内部守卫以继续执行。
fn lock_tests() -> MutexGuard<'static, ()> {
    TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

/// Mock TiFlash store descriptor produced by [`withMockTiFlash`], matching Go's
/// unistore cluster labels (`engine=tiflash`, addr `tiflashN`).
/// [`withMockTiFlash`] 产出的 mock TiFlash store 描述，标签与 Go unistore 一致。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MockTiFlashStore {
    /// Store 标识。
    pub store_id: u64,
    /// Region peer 标识（Region 为键空间分片）。
    pub peer_id: u64,
    /// 地址名，形如 `tiflash0`。
    pub addr: String,
    /// 引擎标签，固定为 `tiflash`。
    pub engine: String,
}

/// [`withMockTiFlash`] 的返回选项：节点数、store 列表与 Region ID。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MockTiFlashOption {
    /// TiFlash 节点个数。
    pub nodes: i32,
    /// 构造出的 mock store 列表。
    pub stores: Vec<MockTiFlashStore>,
    /// 关联的 Region ID。
    pub region_id: u64,
}

/// withMockTiFlash sets the mockStore to have N TiFlash stores (naming as tiflash0, tiflash1, ...).
/// 为 mockStore 配置 N 个 TiFlash store，地址依次为 tiflash0、tiflash1…
pub fn withMockTiFlash(nodes: i32) -> MockTiFlashOption {
    let mut stores = Vec::with_capacity(nodes as usize);
    // BootstrapWithSingleStore 后 peer 1 已占用，自 2 起分配 store/peer ID。
    let mut next_id = 2u64; // region peer 1 reserved after BootstrapWithSingleStore
    let region_id = 1u64;
    let mut tiflash_idx = 0;
    while tiflash_idx < nodes {
        let store_id = next_id;
        next_id += 1;
        let peer_id = next_id;
        next_id += 1;
        stores.push(MockTiFlashStore {
            store_id,
            peer_id,
            addr: format!("tiflash{tiflash_idx}"),
            engine: "tiflash".into(),
        });
        tiflash_idx += 1;
    }
    MockTiFlashOption {
        nodes,
        stores,
        region_id,
    }
}

/// 验证内置函数用量：清空后为空，Collect 会话用量后 Dump 得到计数。
#[test]
fn TestBuiltinFunctionsUsage() {
    let _guard = lock_tests();
    // Clear builtin functions usage (Go Dump twice => second dump empty).
    // 对应 Go：连续 Dump 两次，第二次应为空。
    let _ = GlobalBuiltinFunctionsUsage.Dump();
    let usage = GlobalBuiltinFunctionsUsage.Dump();
    assert_eq!(usage, HashMap::<String, u32>::new());

    // Session close reports PlusInt/MinusInt from `select id + 1 - 2`.
    // 模拟会话关闭时上报的算术内置函数用量。
    let mut session_usage = BuiltinFunctionsUsage::new();
    session_usage.Inc("PlusInt");
    session_usage.Inc("MinusInt");
    GlobalBuiltinFunctionsUsage.Collect(session_usage);

    let usage = GlobalBuiltinFunctionsUsage.Dump();
    assert_eq!(
        usage,
        HashMap::from([("PlusInt".into(), 1u32), ("MinusInt".into(), 1u32)])
    );
}

/// Go 的 uint32 内置函数计数在溢出时回绕，Rust 必须保持相同行为。
#[test]
fn TestBuiltinFunctionsUsageWrapsLikeGo() {
    let mut usage = BuiltinFunctionsUsage::from([("PlusInt".into(), u32::MAX)]);
    usage.Inc("PlusInt");
    assert_eq!(usage["PlusInt"], 0);

    usage.Merge(&BuiltinFunctionsUsage::from([("PlusInt".into(), u32::MAX)]));
    assert_eq!(usage["PlusInt"], u32::MAX);
}

/// Go 的 uint64 窗口聚合在溢出时回绕，所有计数字段均遵循该契约。
#[test]
fn TestWindowMergeWrapsLikeGo() {
    let mut left = crate::windowData {
        BeginAt: UNIX_EPOCH,
        ExecuteCount: u64::MAX,
        TiFlashUsage: crate::tiFlashUsageData {
            PushDown: u64::MAX,
            ExchangePushDown: u64::MAX,
            TableScan: u64::MAX,
            TableScanWithFastScan: u64::MAX,
        },
        CoprCacheUsage: crate::coprCacheUsageData {
            GTE0: u64::MAX,
            GTE1: u64::MAX,
            GTE10: u64::MAX,
            GTE20: u64::MAX,
            GTE40: u64::MAX,
            GTE80: u64::MAX,
            GTE100: u64::MAX,
        },
        BuiltinFunctionsUsage: BuiltinFunctionsUsage::new(),
    };
    let right = crate::windowData {
        BeginAt: UNIX_EPOCH,
        ExecuteCount: 1,
        TiFlashUsage: crate::tiFlashUsageData {
            PushDown: 1,
            ExchangePushDown: 1,
            TableScan: 1,
            TableScanWithFastScan: 1,
        },
        CoprCacheUsage: crate::coprCacheUsageData {
            GTE0: 1,
            GTE1: 1,
            GTE10: 1,
            GTE20: 1,
            GTE40: 1,
            GTE80: 1,
            GTE100: 1,
        },
        BuiltinFunctionsUsage: BuiltinFunctionsUsage::new(),
    };

    crate::data_window::merge(&mut left, &right);
    assert_eq!(left.ExecuteCount, 0);
    assert_eq!(left.TiFlashUsage.PushDown, 0);
    assert_eq!(left.TiFlashUsage.ExchangePushDown, 0);
    assert_eq!(left.TiFlashUsage.TableScan, 0);
    assert_eq!(left.TiFlashUsage.TableScanWithFastScan, 0);
    assert_eq!(left.CoprCacheUsage.GTE0, 0);
    assert_eq!(left.CoprCacheUsage.GTE1, 0);
    assert_eq!(left.CoprCacheUsage.GTE10, 0);
    assert_eq!(left.CoprCacheUsage.GTE20, 0);
    assert_eq!(left.CoprCacheUsage.GTE40, 0);
    assert_eq!(left.CoprCacheUsage.GTE80, 0);
    assert_eq!(left.CoprCacheUsage.GTE100, 0);
}

/// 验证 TiFlash 窗口计数：表扫与 fast-scan 原子计数累加。
#[test]
fn TestTiflashUsage() {
    let _guard = lock_tests();
    let opt = withMockTiFlash(1);
    assert_eq!(opt.nodes, 1);
    assert_eq!(opt.stores.len(), 1);
    assert_eq!(opt.stores[0].addr, "tiflash0");
    assert_eq!(opt.stores[0].engine, "tiflash");
    assert_eq!(opt.region_id, 1);

    CurrentTiFlashPushDownCount.store(0, Ordering::SeqCst);
    CurrentTiflashTableScanCount.store(0, Ordering::SeqCst);
    CurrentTiflashTableScanWithFastScanCount.store(0, Ordering::SeqCst);

    assert_eq!(
        CurrentTiflashTableScanCount
            .load(Ordering::SeqCst)
            .to_string(),
        "0"
    );
    assert_eq!(
        CurrentTiflashTableScanWithFastScanCount
            .load(Ordering::SeqCst)
            .to_string(),
        "0"
    );

    // Simulate isolation_read_engines=tiflash queries and one fast-scan query,
    // then Session.Close() reporting into the window counters.
    CurrentTiflashTableScanCount.fetch_add(1, Ordering::SeqCst); // select count(*) from t
    CurrentTiflashTableScanCount.fetch_add(1, Ordering::SeqCst); // select count(*) from test.t
    CurrentTiflashTableScanWithFastScanCount.fetch_add(1, Ordering::SeqCst);

    assert_eq!(
        CurrentTiflashTableScanCount
            .load(Ordering::SeqCst)
            .to_string(),
        "2"
    );
    assert_eq!(
        CurrentTiflashTableScanWithFastScanCount
            .load(Ordering::SeqCst)
            .to_string(),
        "1"
    );
}

#[test]
/// windowData 的 JSON 字段必须与 Go 的 json tags 和时间编码一致。
fn TestWindowDataMarshalPreservesGoFields() {
    let mut builtin = BuiltinFunctionsUsage::new();
    builtin.Inc("PlusInt");
    let window = crate::windowData {
        BeginAt: UNIX_EPOCH + Duration::from_nanos(123_000_000),
        ExecuteCount: 2,
        TiFlashUsage: crate::tiFlashUsageData {
            PushDown: 3,
            ..Default::default()
        },
        CoprCacheUsage: crate::coprCacheUsageData {
            GTE100: 4,
            ..Default::default()
        },
        BuiltinFunctionsUsage: builtin,
    };

    let json = window.Marshal();
    assert!(json.contains("\"beginAt\":\"1970-01-01T00:00:00.123Z\""));
    assert!(json.contains("\"builtinFunctionsUsage\":{\"PlusInt\":1}"));
    assert!(json.contains("\"executeCount\":2"));
    assert!(json.contains("\"gte100\":4"));
}
