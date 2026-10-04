// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

//! Go `import_retry_test.go` — RangeController retry strategies with Mem SplitClient.
//!
//! 本文件覆盖 RangeController 重试策略的 Go `import_retry_test.go` 对等用例。
//! 使用内存 TestSplitClient 模拟 PD ScanRegions/GetRegionByID，不依赖真实集群。
//! 断言重点：扫描区间边界、NotLeader 切换、Busy 重试、gRPC code 分类、Epoch 整段重扫。
//! 夹具 region 划分固定为 [,aay),[aay,bba),[bba,bbh),[bbh,cca),[cca,)。
//! CreateRangeController 会对 end 做 PrefixNext，因此空 end 测试改用高位键 "zzz"。
//! 重试预算用 InitialRetryState 注入，单测通常把退避时长压到 ZERO。
//! 本文件只解释测试意图与断言依据，不改任何可执行逻辑。

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use crate::import_retry::{
    CreateRangeController, IsMemoryLimited, RPCResult, RPCResultFromError, RPCResultOK,
    RetryStrategy,
};
use crate::stubs::errorpb;
use crate::stubs::grpc_status::{self, Code};
use crate::stubs::metapb;
use crate::stubs::split_client::{RegionInfo, SplitClient};
use crate::stubs::utils_retry;
use crate::stubs::{Context, Error, Result};

/// 把可读字符串转成 region 键字节，便于测试书写。
fn rk(s: &str) -> Vec<u8> {
    // 测试键用 UTF-8 字节，足够区分字典序区间。
    s.as_bytes().to_vec()
}

/// 构造带 epoch=1 与指定 leader 的 RegionInfo，对齐 Go 测试夹具。
fn region(id: u64, start: &str, end: &str, leader: u64) -> RegionInfo {
    RegionInfo {
        Region: Some(metapb::Region {
            Id: id,
            StartKey: rk(start),
            // 空 end 表示最大 region，与 PD 空 EndKey 约定一致。
            EndKey: if end.is_empty() { vec![] } else { rk(end) },
            // 固定 epoch，便于 NotLeader 路径做 CheckRegionEpoch。
            RegionEpoch: Some(metapb::RegionEpoch {
                ConfVer: 1,
                Version: 1,
            }),
            ..Default::default()
        }),
        // leader id 与 region id 初始相同，便于观察切换。
        Leader: Some(metapb::Peer {
            Id: leader,
            StoreId: 1,
        }),
    }
}

/// 内存 SplitClient：按键范围过滤 Scan，并支持运行时改写 leader。
/// by_id 与 regions 双份存储，保证 GetRegionByID 与 Scan 看到一致 leader。
/// Key-range aware SplitClient mirroring Go `split.TestClient` scan behavior.
struct TestSplitClient {
    regions: Mutex<Vec<RegionInfo>>,
    by_id: Mutex<HashMap<u64, RegionInfo>>,
}

impl TestSplitClient {
    // 建立 id→region 索引，供 NotLeader 后 tryFindLeader 查询。
    fn new(regions: Vec<RegionInfo>) -> Arc<Self> {
        let mut by_id = HashMap::new();
        for r in &regions {
            if let Some(meta) = &r.Region {
                by_id.insert(meta.Id, r.clone());
            }
        }
        Arc::new(Self {
            regions: Mutex::new(regions),
            by_id: Mutex::new(by_id),
        })
    }

    // 同步更新 by_id 与 regions 两处，避免扫描与按 id 查询不一致。
    fn set_leader(&self, id: u64, leader: metapb::Peer) {
        let mut by_id = self.by_id.lock().unwrap();
        if let Some(r) = by_id.get_mut(&id) {
            r.Leader = Some(leader.clone());
        }
        let mut regions = self.regions.lock().unwrap();
        for r in regions.iter_mut() {
            if r.Region.as_ref().map(|m| m.Id) == Some(id) {
                r.Leader = Some(leader.clone());
            }
        }
    }
}

// 只实现测试用到的 GetRegionByID / ScanRegions，其它方法走默认。
impl SplitClient for TestSplitClient {
    // 找不到则报错，驱动 tryFindLeader 失败→整段重扫路径。
    fn GetRegionByID(&self, _ctx: &Context, id: u64) -> Result<RegionInfo> {
        self.by_id
            .lock()
            .unwrap()
            .get(&id)
            .cloned()
            .ok_or_else(|| Error::new(format!("region {id} not found")))
    }
    // 与 Go TestClient 相同的半开区间重叠判定。
    fn ScanRegions(
        &self,
        _ctx: &Context,
        start: &[u8],
        end: &[u8],
        _limit: i32,
    ) -> Result<Vec<RegionInfo>> {
        // 线性扫描即可：夹具只有五段。
        let regions = self.regions.lock().unwrap();
        let mut out = Vec::new();
        for r in regions.iter() {
            let meta = r.Region.as_ref().unwrap();
            // end 为空表示扫到正无穷；start/end 均为用户键空间。
            let overlaps = (end.is_empty() || meta.StartKey.as_slice() < end)
                && (meta.EndKey.is_empty() || meta.EndKey.as_slice() > start);
            if overlaps {
                out.push(r.clone());
            }
        }
        Ok(out)
    }
}

struct MissingRegionByIdClient {
    inner: Arc<TestSplitClient>,
    get_calls: AtomicUsize,
    scan_calls: AtomicUsize,
}

impl SplitClient for MissingRegionByIdClient {
    fn GetRegionByID(&self, _ctx: &Context, _id: u64) -> Result<RegionInfo> {
        self.get_calls.fetch_add(1, Ordering::SeqCst);
        Ok(RegionInfo::default())
    }

    fn ScanRegions(
        &self,
        ctx: &Context,
        start: &[u8],
        end: &[u8],
        limit: i32,
    ) -> Result<Vec<RegionInfo>> {
        self.scan_calls.fetch_add(1, Ordering::SeqCst);
        self.inner.ScanRegions(ctx, start, end, limit)
    }
}

/// 固定五段 region 拓扑，覆盖前缀、中间、尾部多种扫描窗口。
fn init_test_client() -> Arc<TestSplitClient> {
    // 边界字符串选自 Go 测试，保证跨语言用例键空间一致。
    // region: [, aay), [aay, bba), [bba, bbh), [bbh, cca), [cca, )
    TestSplitClient::new(vec![
        region(1, "", "aay", 1),
        region(2, "aay", "bba", 2),
        region(3, "bba", "bbh", 3),
        region(4, "bbh", "cca", 4),
        region(5, "cca", "", 5),
    ])
}

/// 校验扫描结果的 StartKey/EndKey 是否与连续边界完全一致。
/// `keys` are consecutive region boundary markers (Go assertRegions style):
/// starts = keys[0..n], last key is exclusive end sentinel.
fn assert_regions(regions: &[RegionInfo], keys: &[&str]) {
    assert_eq!(keys.len(), regions.len() + 1, "regions={regions:?}");
    for (index, region) in regions.iter().enumerate() {
        let meta = region.Region.as_ref().unwrap();
        assert_eq!(meta.StartKey, rk(keys[index]), "region {index} start");
        assert_eq!(meta.EndKey, rk(keys[index + 1]), "region {index} end");
    }
}

/// Go `TestScanSuccess`.
#[test]
// 验证不同 [start,end) 窗口扫到的 region 起点序列。
// 第一段：aa..aay 应命中从空键开始的 region。
fn test_scan_success() {
    let cli = init_test_client();
    // 成功路径只需 1 次预算；退避为 0 加快单测。
    let rs =
        utils_retry::InitialRetryState(1, std::time::Duration::ZERO, std::time::Duration::ZERO);
    let ctx = Context::Background();

    let mut ctl = CreateRangeController(rk("aa"), rk("aay"), cli.clone(), rs.clone());
    // 收集回调中的 region，断言起点序列。
    let collected = Arc::new(Mutex::new(Vec::new()));
    let c2 = collected.clone();
    let mut f = Box::new(move |_ctx: &Context, r: &mut RegionInfo| {
        c2.lock().unwrap().push(r.clone());
        RPCResultOK()
    }) as crate::import_retry::RegionFunc;
    ctl.ApplyFuncToRange(&ctx, &mut f).unwrap();
    // aa..aay 窗口应命中空起点与 aay 两段（断言含哨兵）。
    assert_regions(&collected.lock().unwrap(), &["", "aay", "bba"]);

    // 第二段：aaz..bb 应跨过 aay/bba/bbh 等多个 region。
    let mut ctl = CreateRangeController(rk("aaz"), rk("bb"), cli.clone(), rs.clone());
    let collected = Arc::new(Mutex::new(Vec::new()));
    let c2 = collected.clone();
    let mut f = Box::new(move |_ctx: &Context, r: &mut RegionInfo| {
        c2.lock().unwrap().push(r.clone());
        RPCResultOK()
    }) as crate::import_retry::RegionFunc;
    ctl.ApplyFuncToRange(&ctx, &mut f).unwrap();
    assert_regions(&collected.lock().unwrap(), &["aay", "bba", "bbh", "cca"]);

    // 第三段：不能用空 end（PrefixNext 会变成 [0]），改用 zzz 覆盖全表。
    // CreateRangeController PrefixNext's end; use high end instead of "" (PrefixNext("") => [0]).
    let mut ctl = CreateRangeController(rk("aa"), rk("zzz"), cli.clone(), rs.clone());
    let collected = Arc::new(Mutex::new(Vec::new()));
    let c2 = collected.clone();
    let mut f = Box::new(move |_ctx: &Context, r: &mut RegionInfo| {
        c2.lock().unwrap().push(r.clone());
        RPCResultOK()
    }) as crate::import_retry::RegionFunc;
    ctl.ApplyFuncToRange(&ctx, &mut f).unwrap();
    assert_regions(
        &collected.lock().unwrap(),
        &["", "aay", "bba", "bbh", "cca", ""],
    );

    // 第四段：再次全表扫描，确认幂等与夹具稳定。
    let mut ctl = CreateRangeController(rk("aa"), rk("zzz"), cli, rs);
    let collected = Arc::new(Mutex::new(Vec::new()));
    let c2 = collected.clone();
    let mut f = Box::new(move |_ctx: &Context, r: &mut RegionInfo| {
        c2.lock().unwrap().push(r.clone());
        RPCResultOK()
    }) as crate::import_retry::RegionFunc;
    ctl.ApplyFuncToRange(&ctx, &mut f).unwrap();
    assert_regions(
        &collected.lock().unwrap(),
        &["", "aay", "bba", "bbh", "cca", ""],
    );
}

/// Go `TestNotLeader`.
#[test]
// region 2 首次返回 NotLeader(新 leader=42)，随后成功。
// 断言：对该 region 至少访问两次，最终 leader 为 42，且五段都成功。
fn test_not_leader() {
    let cli = init_test_client();
    // NotLeader 需要额外重试预算。
    let rs =
        utils_retry::InitialRetryState(3, std::time::Duration::ZERO, std::time::Duration::ZERO);
    // 全表扫描以触发 region 2。
    let mut ctl = CreateRangeController(rk(""), rk("zzz"), cli.clone(), rs);
    let ctx = Context::Background();
    // meet=成功完成的 region；id2=对 region2 的所有访问（含失败）。
    let meet = Arc::new(Mutex::new(Vec::new()));
    let id2 = Arc::new(Mutex::new(Vec::new()));
    let meet2 = meet.clone();
    let id2b = id2.clone();
    let cli2 = cli.clone();
    let mut f = Box::new(move |_ctx: &Context, r: &mut RegionInfo| {
        let id = r.Region.as_ref().unwrap().Id;
        if id == 2 {
            id2b.lock().unwrap().push(r.clone());
            // 尚未切换时注入 StoreError::NotLeader，触发 FromThisRegion。
            if r.Leader.as_ref().map(|l| l.Id) != Some(42) {
                return RPCResult {
                    StoreError: Some(errorpb::Error {
                        Message: "not leader".into(),
                        NotLeader: Some(errorpb::NotLeader {
                            Leader: Some(metapb::Peer { Id: 42, StoreId: 1 }),
                        }),
                        ..Default::default()
                    }),
                    ..Default::default()
                };
            }
            // 成功路径同步更新 PD 视图，供后续 GetRegionByID 一致。
            cli2.set_leader(2, metapb::Peer { Id: 42, StoreId: 1 });
        }
        meet2.lock().unwrap().push(r.clone());
        RPCResultOK()
    }) as crate::import_retry::RegionFunc;
    ctl.ApplyFuncToRange(&ctx, &mut f).unwrap();
    let id2s = id2.lock().unwrap();
    // 至少一次失败 + 一次成功。
    assert!(id2s.len() >= 2, "id2 meetings={}", id2s.len());
    // 最终 leader 必须已被响应更新为 42。
    assert_eq!(id2s.last().unwrap().Leader.as_ref().unwrap().Id, 42);
    // 五段均成功 apply。
    assert_eq!(meet.lock().unwrap().len(), 5);
}

/// Go `TestServerIsBusy`.
#[test]
// 首次访问 region 2 返回 ServerIsBusy（非 memory limited），应本 region 重试后成功。
// meet 计数最终应为 5，说明五段都走完且未整段放弃。
fn test_server_is_busy() {
    let cli = init_test_client();
    let rs =
        utils_retry::InitialRetryState(3, std::time::Duration::ZERO, std::time::Duration::ZERO);
    let mut ctl = CreateRangeController(rk(""), rk("zzz"), cli, rs);
    let ctx = Context::Background();
    let first = Arc::new(Mutex::new(true));
    let meet = Arc::new(Mutex::new(0usize));
    let first2 = first.clone();
    let meet2 = meet.clone();
    let mut f = Box::new(move |_ctx: &Context, r: &mut RegionInfo| {
        let id = r.Region.as_ref().unwrap().Id;
        if *first2.lock().unwrap() && id == 2 {
            *first2.lock().unwrap() = false;
            // 普通 busy（非 memory limited）仍走 FromThisRegion。
            return RPCResult {
                StoreError: Some(errorpb::Error {
                    Message: "server is busy".into(),
                    ServerIsBusy: Some(errorpb::ServerIsBusy {
                        Message: "memory is out".into(),
                    }),
                    ..Default::default()
                }),
                ..Default::default()
            };
        }
        *meet2.lock().unwrap() += 1;
        RPCResultOK()
    }) as crate::import_retry::RegionFunc;
    ctl.ApplyFuncToRange(&ctx, &mut f).unwrap();
    // busy 重试后五段都应成功计数。
    assert_eq!(*meet.lock().unwrap(), 5);
}

/// Go `TestWrappedError` / StrategyForRetry.
#[test]
// stub 把含 "unavailable" 的错误识别为 gRPC Unavailable → FromThisRegion。
// 普通字符串无 code → GiveUp。
fn test_wrapped_error() {
    assert_eq!(
        // 错误文案含 unavailable → 映射为可重试。
        RPCResultFromError(Error::new("unavailable backend")).StrategyForRetry(),
        RetryStrategy::StrategyFromThisRegion
    );
    assert_eq!(
        // 无可识别 code → 放弃。
        RPCResultFromError(Error::new("plain")).StrategyForRetry(),
        RetryStrategy::StrategyGiveUp
    );
    let st = grpc_status::FromError(&Error::new("Unavailable")).unwrap();
    assert_eq!(st.Code(), Code::Unavailable);
}

/// Go `TestRetryRecognizeErrCode`.
#[test]
// 覆盖 Go 侧可重试的四类 gRPC code 字符串识别。
// Unknown 必须 GiveUp，防止误重试致命错误。
fn test_retry_recognize_err_code() {
    // 与 Go 测试字符串集合保持一致。
    for msg in [
        "Unavailable",
        "Aborted",
        "resource exhausted",
        "DeadlineExceeded",
    ] {
        let r = RPCResultFromError(Error::new(msg));
        assert_eq!(
            r.StrategyForRetry(),
            RetryStrategy::StrategyFromThisRegion,
            "{msg}"
        );
    }
    assert_eq!(
        // Unknown 不可重试。
        RPCResultFromError(Error::new("Unknown")).StrategyForRetry(),
        RetryStrategy::StrategyGiveUp
    );
}

/// Go `TestRetryBackoff` — retry state advances.
#[test]
// 确认 RetryState 在 ExponentialBackoff 后仍允许继续重试。
fn test_retry_backoff() {
    let mut rs = utils_retry::InitialRetryState(
        3,
        std::time::Duration::from_millis(1),
        std::time::Duration::from_millis(10),
    );
    // 初始允许重试。
    assert!(rs.ShouldRetry());
    // 消耗一次退避后仍应允许（预算=3）。
    let _ = rs.ExponentialBackoff();
    assert!(rs.ShouldRetry());
}

/// Go `TestEpochNotMatch` — StrategyFromStart.
#[test]
// 仅有 Message、无 Busy/NotLeader 的 store 错误 → FromStart。
fn test_epoch_not_match() {
    let r = RPCResult {
        StoreError: Some(errorpb::Error {
            // 无结构化 NotLeader/Busy 字段时默认 FromStart。
            Message: "epoch not match".into(),
            ..Default::default()
        }),
        ..Default::default()
    };
    assert_eq!(r.StrategyForRetry(), RetryStrategy::StrategyFromStart);
}

/// Go `TestRegionSplit` / PaginateScanRegion shape via SplitClient.
#[test]
// 直接测 ScanRegions 重叠：aaz..bbb 应命中从 aay 开始的段。
fn test_region_split_scan() {
    let cli = init_test_client();
    let ctx = Context::Background();
    // limit 参数在夹具中未严格截断，只需非空且首段正确。
    let regions = cli.ScanRegions(&ctx, b"aaz", b"bbb", 2).unwrap();
    // 重叠判定应至少命中 aay 段。
    assert!(!regions.is_empty());
    assert_eq!(regions[0].Region.as_ref().unwrap().StartKey, rk("aay"));
}

#[test]
fn missing_region_by_id_stops_leader_retry_and_rescans_range() {
    let client = Arc::new(MissingRegionByIdClient {
        inner: init_test_client(),
        get_calls: AtomicUsize::new(0),
        scan_calls: AtomicUsize::new(0),
    });
    let rs =
        utils_retry::InitialRetryState(3, std::time::Duration::ZERO, std::time::Duration::ZERO);
    let mut ctl = CreateRangeController(rk(""), rk("zzz"), client.clone(), rs);
    let first = Arc::new(Mutex::new(true));
    let first_for_callback = first.clone();
    let mut f = Box::new(move |_ctx: &Context, region: &mut RegionInfo| {
        if region.Region.as_ref().map(|meta| meta.Id) == Some(2)
            && *first_for_callback.lock().unwrap()
        {
            *first_for_callback.lock().unwrap() = false;
            return RPCResult {
                StoreError: Some(errorpb::Error {
                    Message: "leader not found".into(),
                    NotLeader: Some(errorpb::NotLeader::default()),
                    ..Default::default()
                }),
                ..Default::default()
            };
        }
        RPCResultOK()
    }) as crate::import_retry::RegionFunc;

    ctl.ApplyFuncToRange(&Context::Background(), &mut f)
        .unwrap();

    assert_eq!(client.get_calls.load(Ordering::SeqCst), 1);
    assert_eq!(client.scan_calls.load(Ordering::SeqCst), 2);
}

/// Go `TestPaginateScanLeader`.
#[test]
// PaginateScanRegion 空范围应返回全部五段，且每段都有 Leader。
fn test_paginate_scan_leader() {
    let cli = init_test_client();
    let ctx = Context::Background();
    let regions =
        // 空范围分页扫描应覆盖全部拓扑。
        crate::stubs::split_client::PaginateScanRegion(&ctx, cli.as_ref(), b"", b"", 128).unwrap();
    assert_eq!(regions.len(), 5);
    for r in &regions {
        // Paginate 路径应填充 Leader，供 Apply 直接使用。
        assert!(r.Leader.is_some());
    }
}

/// Go `TestServerIsBusyWithMemoryIsLimited` — strategy recognition (failpoint sleep skipped on darwin).
#[test]
// 策略层识别 memory is limited → FromThisRegion；不在此触发真实长睡眠。
fn test_server_is_busy_with_memory_is_limited() {
    let r = RPCResult {
        StoreError: Some(errorpb::Error {
            // 文案必须包含 memory is limited 子串才会走长睡眠分支。
            Message: "busy".into(),
            ServerIsBusy: Some(errorpb::ServerIsBusy {
                Message: "memory is limited".into(),
            }),
            ..Default::default()
        }),
        ..Default::default()
    };
    assert_eq!(r.StrategyForRetry(), RetryStrategy::StrategyFromThisRegion);
    // 双重确认：策略与消息字段都符合 memory limited 语义。
    assert!(
        r.StoreError
            .as_ref()
            .unwrap()
            .GetServerIsBusy()
            .unwrap()
            .Message
            .contains("memory is limited")
    );
}

/// Go checks the top-level `errorpb.Error.Message`, not the nested busy detail.
#[test]
fn test_memory_limited_uses_store_error_message() {
    let store_error = errorpb::Error {
        Message: "memory is limited".into(),
        ServerIsBusy: Some(errorpb::ServerIsBusy {
            Message: "unrelated nested detail".into(),
        }),
        ..Default::default()
    };
    assert!(IsMemoryLimited(&store_error));

    let nested_only = errorpb::Error {
        Message: "server is busy".into(),
        ServerIsBusy: Some(errorpb::ServerIsBusy {
            Message: "memory is limited".into(),
        }),
        ..Default::default()
    };
    assert!(!IsMemoryLimited(&nested_only));
}
