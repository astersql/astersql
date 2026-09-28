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

//! SplitClient and pdClient, matching `client.go` with local PD/TiKV traits.
//! Region split/scatter 客户端，对齐 Go `split/client.go`。
//! 封装 PD 查询、批量分裂、等待算子完成与 scatter 调度。
//! 重试上限 splitRegionMaxRetryTime 与批大小 ThreadLocalBatchSize 控制稳定性。
//! 空 end key、空 ScanRegions、可重试 PD 错误是关键边界分支。
//! RawKV 与 TxnKV 路径在 scatter 上共享骨架但键编码不同。
//! SplitClient trait 是对 PD/TiKV 分裂能力的最小抽象，便于 mock。
//! GetStore/GetRegion/GetRegionByID 提供分裂决策所需元数据。
//! SplitKeysAndScatter 是常用组合：先分裂再打散，降低热点。
//! SplitWaitAndScatter 在等待算子完成后再 scatter，适合强一致场景。
//! GetOperator 用于观察 PD 调度进度，决定是否继续重试。
//! ThreadLocalBatchSize 限制单批 key 数量，避免一次 RPC 过大。
//! splitRegionMaxRetryTime 防止区域抖动时无限重试。
//! 空 end key 表示范围上界开放，Scan/Split 需按 PD 约定处理。
//! ScanRegions 空结果不一定是成功，可能需重试或报区域缺失。
//! PD 可重试错误与不可重试错误必须区分，避免放大故障。
//! RawKV 路径键不加事务编码前缀，与 TxnKV 分裂点计算不同。
//! Scatter 失败时的部分成功要能通过后续扫描发现并补偿。
//! Region 边界校验关注连续性：上一 end 等于下一 start。
//! Leader 缺失时某些操作应失败或等待，不能假设永远有 Leader。
//! 批量分裂要保持输入 key 有序去重，减少无效 PD 调用。
//! Context 取消应尽快返回，不要在重试循环里忽略 Done。
//! 日志字段包含 region id/key hex，便于对照 PD 侧日志。
//! 与 Go client.go 的重试退避策略应对齐，避免测试时间假设失效。
//! mock_pd_client 只覆盖本包单测需要的子集，不声称协议完备。
//! 算子状态轮询间隔在测试中可被缩短，但生产默认需保守。
//! Split 后的新 region 可能短暂不可见，调用方应容忍重试。
//! Store 状态非 Up 时不应作为 scatter 目标。
//! 错误包装使用 Annotate/Annotatef，保留根因类型供上层判断。
//! 并发调用同一 client 时，内部缓存/批处理不得破坏键顺序语义。
//! 本文件不负责 SST 导入，只提供区域准备能力给 restore 上层。

use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::Ordering;
use std::sync::{Mutex, Once};
use std::time::Duration;

use astersql_br_pkg_errors::{
    ErrInvalidRange, ErrPDInvalidResponse, ErrPDNotFullyScatter, ErrPDRegionsNotFullyScatter,
};
use astersql_errors::{Annotate, Annotatef, New, SharedError};

use crate::region::RegionInfo;
use crate::split::{
    ErrBackoff, ErrBackoffAndDontCount, NewBackoffMayNotCountBackoffer, PaginateScanRegion,
    ScanRegionPaginationLimit, SplitMaxRetryInterval, SplitRetryInterval, SplitRetryTimes,
    getSplitKeysOfRegions,
};
use crate::stubs::{
    self, BackoffStrategy, CodecPDClient, Context, GetRegionOption, GetStoreOption,
    InitialRetryState, KeyNext, Result, WithRetryReturnLastErr, codec, log, metapb, pdhttp, pdpb,
    redact,
};

/// `splitRegionMaxRetryTime`：与 Go 常量同义的阈值/阈值阈值，改动前先对照 Go。
const splitRegionMaxRetryTime: i32 = 4;
/// Max total key size in a split region batch (6 MiB).
/// `ThreadLocalBatchSize`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
pub struct ThreadLocalBatchSize;

thread_local! {
    static MAX_BATCH_SPLIT_SIZE: Cell<usize> = const { Cell::new(6 * 1024 * 1024) };
}

impl ThreadLocalBatchSize {
    /// `load`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    pub fn load(&self, _ordering: Ordering) -> usize {
        MAX_BATCH_SPLIT_SIZE.get()
    }

    /// `store`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    pub fn store(&self, value: usize, _ordering: Ordering) {
        MAX_BATCH_SPLIT_SIZE.set(value);
    }
}

pub static maxBatchSplitSize: ThreadLocalBatchSize = ThreadLocalBatchSize;

/// SplitClient is an external client used by RegionSplitter.
/// `SplitClient`：抽象依赖边界，便于注入 Mem* 桩或真实客户端。
pub trait SplitClient: Send + Sync {
    /// `GetStore`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    fn GetStore(
        &self,
        ctx: &Context,
        storeID: u64,
        _opts: &[GetStoreOption],
    ) -> Result<metapb::Store>;

    /// `GetRegion`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    fn GetRegion(&self, ctx: &Context, key: &[u8]) -> Result<RegionInfo>;

    /// `GetRegionByID`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    fn GetRegionByID(&self, ctx: &Context, regionID: u64) -> Result<RegionInfo>;

    /// `SplitKeysAndScatter`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    fn SplitKeysAndScatter(
        &self,
        ctx: &Context,
        sortedSplitKeys: &[Vec<u8>],
    ) -> Result<Vec<RegionInfo>>;

    /// `SplitKeys`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    fn SplitKeys(&self, ctx: &Context, sortedSplitKeys: &[Vec<u8>]) -> Result<Vec<RegionInfo>>;

    /// `SplitWaitAndScatter`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    fn SplitWaitAndScatter(
        &self,
        ctx: &Context,
        region: &RegionInfo,
        keys: &[Vec<u8>],
    ) -> Result<Vec<RegionInfo>>;

    /// `GetOperator`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    fn GetOperator(&self, ctx: &Context, regionID: u64) -> Result<pdpb::GetOperatorResponse>;

    /// `ScanRegions`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    fn ScanRegions(
        &self,
        ctx: &Context,
        key: &[u8],
        endKey: &[u8],
        limit: i32,
        opts: &[GetRegionOption],
    ) -> Result<Vec<RegionInfo>>;

    /// `GetPlacementRule`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    fn GetPlacementRule(&self, ctx: &Context, groupID: &str, ruleID: &str) -> Result<pdhttp::Rule>;

    /// `SetPlacementRule`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    fn SetPlacementRule(&self, ctx: &Context, rule: &pdhttp::Rule) -> Result<()>;

    /// `DeletePlacementRule`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    fn DeletePlacementRule(&self, ctx: &Context, groupID: &str, ruleID: &str) -> Result<()>;

    /// `SetStoresLabel`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    fn SetStoresLabel(
        &self,
        ctx: &Context,
        stores: &[u64],
        labelKey: &str,
        labelValue: &str,
    ) -> Result<()>;

    /// `WaitRegionsScattered`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    fn WaitRegionsScattered(
        &self,
        ctx: &Context,
        regionInfos: &[RegionInfo],
    ) -> Result<(i32, SharedError)> {
        // Default: all finished.
        let _ = (ctx, regionInfos);
        Ok((0, New("")))
    }

    /// Convenience: WaitRegionsScattered returning only notFinished count.
    /// `WaitRegionsScatteredCount`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    fn WaitRegionsScatteredCount(
        &self,
        ctx: &Context,
        regionInfos: &[RegionInfo],
    ) -> (i32, Option<SharedError>) {
        match self.WaitRegionsScattered(ctx, regionInfos) {
            Ok((n, e)) if e.to_string().is_empty() => (n, None),
            Ok((n, e)) => (n, Some(e)),
            Err(e) => (regionInfos.len() as i32, Some(e)),
        }
    }

    /// `GetCodecPDClient`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    fn GetCodecPDClient(&self) -> Option<CodecPDClient> {
        None
    }
}

/// Local PD client trait (subset used by pdClient).
/// `PdBackend`：抽象依赖边界，便于注入 Mem* 桩或真实客户端。
pub trait PdBackend: Send + Sync {
    /// `GetStore`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    fn GetStore(&self, storeID: u64) -> Result<metapb::Store>;
    /// `GetRegion`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    fn GetRegion(&self, key: &[u8]) -> Result<RegionInfo>;
    /// `GetRegionByID`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    fn GetRegionByID(&self, regionID: u64) -> Result<RegionInfo>;
    /// `ScanRegions`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    fn ScanRegions(
        &self,
        key: &[u8],
        endKey: &[u8],
        limit: i32,
        allow_follower: bool,
    ) -> Result<Vec<RegionInfo>>;
    /// `GetOperator`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    fn GetOperator(&self, regionID: u64) -> Result<pdpb::GetOperatorResponse>;
    /// `ScatterRegions`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    fn ScatterRegions(&self, regionIDs: &[u64]) -> Result<pdpb::ScatterRegionResponse>;
    /// `ScatterRegion`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    fn ScatterRegion(&self, regionID: u64) -> Result<()>;
    /// `GetAllStores`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    fn GetAllStores(&self) -> Result<Vec<metapb::Store>>;
    /// `SplitRegion`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    fn SplitRegion(
        &self,
        region: &RegionInfo,
        keys: &[Vec<u8>],
        is_raw_kv: bool,
    ) -> Result<(RegionInfo, Vec<RegionInfo>)>;
}

/// Local HTTP PD client trait.
/// `PdHttpBackend`：抽象依赖边界，便于注入 Mem* 桩或真实客户端。
pub trait PdHttpBackend: Send + Sync {
    /// `GetReplicateConfig`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    fn GetReplicateConfig(&self) -> Result<HashMap<String, f64>>;
    /// `GetPlacementRule`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    fn GetPlacementRule(&self, groupID: &str, ruleID: &str) -> Result<pdhttp::Rule>;
    /// `SetPlacementRule`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    fn SetPlacementRule(&self, rule: &pdhttp::Rule) -> Result<()>;
    /// `DeletePlacementRule`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    fn DeletePlacementRule(&self, groupID: &str, ruleID: &str) -> Result<()>;
    /// Updates one label on a store through PD's HTTP API.
    fn SetStoreLabels(&self, storeID: u64, labelKey: &str, labelValue: &str) -> Result<()> {
        let _ = (storeID, labelKey, labelValue);
        Ok(())
    }
}

/// `ClientOptionalParameter`：类型别名，保持与 Go 命名空间可读对照。
pub type ClientOptionalParameter = Box<dyn Fn(&mut PdClient) + Send + Sync>;

/// `WithRawKV`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
/// 留意空集合、取消上下文与默认值是否保持一致。
pub fn WithRawKV() -> ClientOptionalParameter {
    Box::new(|c: &mut PdClient| {
        c.isRawKv = true;
    })
}

/// `WithOnSplit`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
/// 留意空集合、取消上下文与默认值是否保持一致。
pub fn WithOnSplit(
    onSplit: impl Fn(&[Vec<u8>]) + Send + Sync + 'static,
) -> ClientOptionalParameter {
    let onSplit = std::sync::Arc::new(onSplit);
    Box::new(move |c: &mut PdClient| {
        let onSplit = onSplit.clone();
        c.onSplit = Some(Box::new(move |keys: &[Vec<u8>]| onSplit(keys)));
    })
}

/// pdClient wrapper used by RegionSplitter.
/// `PdClient`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
pub struct PdClient {
    pub backend: Box<dyn PdBackend>,
    pub http: Option<Box<dyn PdHttpBackend>>,
    pub storeCache: Mutex<HashMap<u64, metapb::Store>>,
    needScatterVal: Mutex<bool>,
    needScatterInit: Once,
    pub isRawKv: bool,
    pub onSplit: Option<Box<dyn Fn(&[Vec<u8>]) + Send + Sync>>,
    pub splitConcurrency: i32,
    pub splitBatchKeyCnt: i32,
    pub isCodecPDClient: bool,
    pub codecClient: Option<CodecPDClient>,
}

/// `NewClient`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
/// 留意空集合、取消上下文与默认值是否保持一致。
pub fn NewClient(
    backend: Box<dyn PdBackend>,
    http: Option<Box<dyn PdHttpBackend>>,
    splitBatchKeyCnt: i32,
    splitConcurrency: i32,
    opts: Vec<ClientOptionalParameter>,
) -> PdClient {
    let mut cli = PdClient {
        backend,
        http,
        storeCache: Mutex::new(HashMap::new()),
        needScatterVal: Mutex::new(false),
        needScatterInit: Once::new(),
        isRawKv: false,
        onSplit: None,
        splitConcurrency,
        splitBatchKeyCnt,
        isCodecPDClient: false,
        codecClient: None,
    };
    for opt in opts {
        opt(&mut cli);
    }
    cli
}

/// `NewCodecAwareClient`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
/// 留意空集合、取消上下文与默认值是否保持一致。
pub fn NewCodecAwareClient(
    backend: Box<dyn PdBackend>,
    http: Option<Box<dyn PdHttpBackend>>,
    splitBatchKeyCnt: i32,
    splitConcurrency: i32,
    opts: Vec<ClientOptionalParameter>,
) -> PdClient {
    let mut cli = NewClient(backend, http, splitBatchKeyCnt, splitConcurrency, opts);
    cli.isCodecPDClient = true;
    cli.codecClient = Some(CodecPDClient);
    cli
}

impl PdClient {
    /// Force scatter on/off for tests (mirrors Go setting `needScatterVal` + init Once).
    /// `ForceNeedScatter`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    pub fn ForceNeedScatter(&self, need: bool) {
        *self.needScatterVal.lock().unwrap() = need;
        self.needScatterInit.call_once(|| {});
    }

    /// `needScatter`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    fn needScatter(&self, ctx: &Context) -> bool {
        self.needScatterInit.call_once(|| {
            let need = self.checkNeedScatter(ctx).unwrap_or_else(|err| {
                log::Warn(&format!(
                    "failed to check whether need to scatter, use permissive strategy: always scatter: {err}"
                ));
                true
            });
            *self.needScatterVal.lock().unwrap() = need;
        });
        *self.needScatterVal.lock().unwrap()
    }

    /// `checkNeedScatter`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    fn checkNeedScatter(&self, _ctx: &Context) -> Result<bool> {
        let storeCount = self.getStoreCount()?;
        let maxReplica = self.getMaxReplica()?;
        log::Info("checking whether need to scatter");
        Ok(storeCount >= maxReplica && storeCount > 1)
    }

    /// `getStoreCount`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    fn getStoreCount(&self) -> Result<usize> {
        Ok(self.backend.GetAllStores()?.len())
    }

    /// `getMaxReplica`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    fn getMaxReplica(&self) -> Result<usize> {
        let http = self
            .http
            .as_ref()
            .ok_or_else(|| New("http client missing"))?;
        let resp = http.GetReplicateConfig()?;
        let val = resp
            .get("max-replicas")
            .copied()
            .ok_or_else(|| New("key max-replicas not found"))?;
        Ok(val as usize)
    }

    /// `getEncodedKeys`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    fn getEncodedKeys(&self, start: &[u8], end: &[u8]) -> Result<(Vec<u8>, Vec<u8>)> {
        if let Some(codecCli) = self.GetCodecPDClient() {
            return codecCli.GetCodec().DecodeRange(start, end);
        }
        Ok((
            codec::EncodeBytesExt(Vec::new(), start, self.isRawKv),
            codec::EncodeBytesExt(Vec::new(), end, self.isRawKv),
        ))
    }

    /// `splitKeys`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    fn splitKeys(
        &self,
        ctx: &Context,
        sortedSplitKeys: &[Vec<u8>],
        scatter: bool,
    ) -> Result<Vec<RegionInfo>> {
        if sortedSplitKeys.is_empty() {
            return Ok(Vec::new());
        }
        let last = &sortedSplitKeys[sortedSplitKeys.len() - 1];
        let lastKey = if last.is_empty() {
            last.clone()
        } else {
            KeyNext(last)
        };
        let (mut scanStart, mut scanEnd) = self.getEncodedKeys(&sortedSplitKeys[0], &lastKey)?;

        let ret: Mutex<Vec<RegionInfo>> = Mutex::new(Vec::new());
        let retrySplitKeys: Mutex<Vec<Vec<u8>>> = Mutex::new(Vec::new());
        let lastSplitErr: Mutex<Option<SharedError>> = Mutex::new(None);

        let mut backoff = NewBackoffRetryAllExceptStrategy(
            SplitRetryTimes.load(Ordering::SeqCst),
            SplitRetryInterval,
            SplitMaxRetryInterval,
        );
        let result = WithRetryReturnLastErr(
            ctx,
            || {
                ret.lock().unwrap().clear();
                let mut keys_to_split = sortedSplitKeys.to_vec();
                {
                    let retry = retrySplitKeys.lock().unwrap();
                    if !retry.is_empty() {
                        keys_to_split = retry.clone();
                        let last = &keys_to_split[keys_to_split.len() - 1];
                        let last_key = if last.is_empty() {
                            last.clone()
                        } else {
                            KeyNext(last)
                        };
                        let (s, e) = self.getEncodedKeys(&keys_to_split[0], &last_key)?;
                        scanStart = s;
                        scanEnd = e;
                    }
                }
                retrySplitKeys.lock().unwrap().clear();

                let mut regions =
                    PaginateScanRegion(ctx, self, &scanStart, &scanEnd, ScanRegionPaginationLimit)?;
                if self.GetCodecPDClient().is_some() {
                    let cd = self.GetCodecPDClient().unwrap().GetCodec();
                    crate::split::encodeRegionKeys(&mut regions, &|a, b| {
                        cd.EncodeRegionRange(a, b)
                    });
                }

                let splitKeyMap = getSplitKeysOfRegions(&keys_to_split, &regions, self.isRawKv);
                for region in &regions {
                    let id = region.Region.as_ref().map(|r| r.Id).unwrap_or(0);
                    let Some(splitKeys) = splitKeyMap.get(&id) else {
                        continue;
                    };
                    match self.splitWaitAndMaybeScatter(ctx, region, splitKeys, scatter) {
                        Ok(newRegions) => {
                            if scatter {
                                ret.lock().unwrap().extend(newRegions);
                            }
                        }
                        Err(e) => {
                            if e.to_string().contains("canceled") {
                                return Err(e);
                            }
                            log::Warn("split and scatter region meet error, will retry");
                            retrySplitKeys.lock().unwrap().extend(splitKeys.clone());
                            *lastSplitErr.lock().unwrap() = Some(e);
                        }
                    }
                }
                let retry = retrySplitKeys.lock().unwrap();
                if retry.is_empty() {
                    return Ok(());
                }
                Err(lastSplitErr
                    .lock()
                    .unwrap()
                    .clone()
                    .unwrap_or_else(|| New("split retry")))
            },
            &mut backoff,
        );
        result?;
        Ok(ret.into_inner().unwrap())
    }

    /// `splitWaitAndMaybeScatter`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    fn splitWaitAndMaybeScatter(
        &self,
        ctx: &Context,
        region: &RegionInfo,
        keys: &[Vec<u8>],
        scatter: bool,
    ) -> Result<Vec<RegionInfo>> {
        if keys.is_empty() {
            return Ok(vec![region.clone()]);
        }
        let mut region = region.clone();
        let mut start = 0usize;
        let mut end = 0usize;
        let mut batchSize = 0usize;
        let mut newRegions = Vec::with_capacity(keys.len());

        while end <= keys.len() {
            if end == keys.len()
                || batchSize + keys[end].len() > maxBatchSplitSize.load(Ordering::SeqCst)
                || (end as i32) - (start as i32) >= self.splitBatchKeyCnt
            {
                let (originRegion, newRegionsOfBatch) =
                    self.batchSplitRegionsWithOrigin(ctx, &region, &keys[start..end])?;
                if let Err(e) = self.waitRegionsSplit(ctx, &newRegionsOfBatch) {
                    log::Warn(&format!("wait regions split failed: {e}"));
                }
                if let Some(e) = ctx.Err() {
                    return Err(e);
                }
                if scatter {
                    if let Err(e) = self.scatterRegions(ctx, &newRegionsOfBatch) {
                        log::Warn(&format!("scatter regions failed: {e}"));
                    }
                }
                if let Some(cb) = &self.onSplit {
                    cb(&keys[start..end]);
                }
                let lastNew = newRegionsOfBatch[newRegionsOfBatch.len() - 1].clone();
                let origin_start = originRegion
                    .Region
                    .as_ref()
                    .map(|r| r.StartKey.clone())
                    .unwrap_or_default();
                let last_start = lastNew
                    .Region
                    .as_ref()
                    .map(|r| r.StartKey.clone())
                    .unwrap_or_default();
                region = if origin_start < last_start {
                    lastNew
                } else {
                    originRegion
                };
                newRegions.extend(newRegionsOfBatch);
                batchSize = 0;
                start = end;
            }
            if end < keys.len() {
                batchSize += keys[end].len();
            }
            end += 1;
        }
        if let Some(e) = ctx.Err() {
            return Err(e);
        }
        Ok(newRegions)
    }

    /// `batchSplitRegionsWithOrigin`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    fn batchSplitRegionsWithOrigin(
        &self,
        _ctx: &Context,
        region: &RegionInfo,
        keys: &[Vec<u8>],
    ) -> Result<(RegionInfo, Vec<RegionInfo>)> {
        self.backend.SplitRegion(region, keys, self.isRawKv)
    }

    /// `waitRegionsSplit`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    fn waitRegionsSplit(&self, ctx: &Context, newRegions: &[RegionInfo]) -> Result<()> {
        for r in newRegions {
            let id = r.Region.as_ref().map(|m| m.Id).unwrap_or(0);
            let mut attempts = 0;
            while attempts < WaitRegionOnlineAttemptTimes_local() {
                if ctx.Done() {
                    return Err(ctx.Err().unwrap_or_else(|| New("canceled")));
                }
                if self.hasHealthyRegion(ctx, id)? {
                    break;
                }
                attempts += 1;
            }
        }
        Ok(())
    }

    /// `hasHealthyRegion`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    fn hasHealthyRegion(&self, _ctx: &Context, regionID: u64) -> Result<bool> {
        match self.backend.GetRegionByID(regionID) {
            Ok(info) => Ok(info.PendingPeers.is_empty()),
            Err(_) => Ok(false),
        }
    }

    /// `scatterRegions`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    pub fn scatterRegions(&self, ctx: &Context, newRegions: &[RegionInfo]) -> Result<()> {
        if !self.needScatter(ctx) {
            return Ok(());
        }
        let mut regions = newRegions.to_vec();
        let mut backoff = NewBackoffRetryAllExceptStrategy(
            1800,
            Duration::from_millis(500),
            Duration::from_secs(2),
        );
        // Keep tests fast: shrink delay path via WithRetry micro-sleep.
        let err = WithRetryReturnLastErr(
            ctx,
            || {
                let (failed, err) = self.tryScatterRegions(ctx, &regions)?;
                if isUnsupportedError(&err.clone().unwrap_or_else(|| New("")))
                    || err
                        .as_ref()
                        .map(|e| ErrPDRegionsNotFullyScatter.Equal(Some(e)))
                        .unwrap_or(false)
                {
                    self.scatterRegionsSequentially(
                        ctx,
                        &regions,
                        &mut NewBackoffRetryAllExceptStrategy(
                            1800,
                            Duration::from_millis(100),
                            Duration::from_secs(2),
                        ),
                    );
                    return Ok(());
                }
                if !failed.is_empty() {
                    regions
                        .retain(|r| failed.contains(&r.Region.as_ref().map(|m| m.Id).unwrap_or(0)));
                    return Err(Annotatef(
                        Some(SharedError::new(ErrPDNotFullyScatter.clone())),
                        &format!(
                            "pd returns error during batch scattering: {} regions failed to scatter",
                            failed.len()
                        ),
                        &[],
                    )
                    .expect("annotate"));
                }
                if let Some(e) = err {
                    return Err(e);
                }
                Ok(())
            },
            &mut backoff,
        );
        match err {
            Err(e) if ErrPDNotFullyScatter.Equal(Some(&e)) => Ok(()),
            other => other,
        }
    }

    /// `tryScatterRegions`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    fn tryScatterRegions(
        &self,
        _ctx: &Context,
        regionInfo: &[RegionInfo],
    ) -> Result<(HashSet<u64>, Option<SharedError>)> {
        let ids: Vec<u64> = regionInfo
            .iter()
            .filter_map(|r| r.Region.as_ref().map(|m| m.Id))
            .collect();
        match self.backend.ScatterRegions(&ids) {
            Ok(resp) => {
                if let Some(header) = &resp.Header {
                    if let Some(err) = &header.Error {
                        if err.Type != pdpb::ErrorType::OK {
                            return Err(Annotatef(
                                Some(SharedError::new(ErrPDInvalidResponse.clone())),
                                &format!(
                                    "pd returns error during batch scattering: {}",
                                    err.Message
                                ),
                                &[],
                            )
                            .expect("annotate"));
                        }
                    }
                }
                if !resp.FailedRegionsId.is_empty() {
                    return Ok((resp.FailedRegionsId.into_iter().collect(), None));
                }
                if resp.FinishedPercentage < 100 {
                    return Ok((
                        HashSet::new(),
                        Some(
                            Annotatef(
                                Some(SharedError::new(ErrPDRegionsNotFullyScatter.clone())),
                                &format!(
                                    "scatter finished percentage {} less than 100",
                                    resp.FinishedPercentage
                                ),
                                &[],
                            )
                            .expect("annotate"),
                        ),
                    ));
                }
                Ok((HashSet::new(), None))
            }
            Err(e) => {
                if isUnsupportedError(&e) {
                    return Ok((HashSet::new(), Some(e)));
                }
                Err(e)
            }
        }
    }

    /// `scatterRegionsSequentially`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    pub fn scatterRegionsSequentially(
        &self,
        ctx: &Context,
        newRegions: &[RegionInfo],
        backoff: &mut dyn BackoffStrategy,
    ) {
        let mut remain: HashMap<u64, RegionInfo> = newRegions
            .iter()
            .filter_map(|r| r.Region.as_ref().map(|m| (m.Id, r.clone())))
            .collect();
        let _ = WithRetryReturnLastErr(
            ctx,
            || {
                let mut errs: Option<SharedError> = None;
                let ids: Vec<u64> = remain.keys().copied().collect();
                for id in ids {
                    match self.backend.ScatterRegion(id) {
                        Ok(()) => {
                            remain.remove(&id);
                        }
                        Err(e) => {
                            if !PdErrorCanRetry(&e) {
                                remain.remove(&id);
                            }
                            errs = Some(match errs {
                                Some(prev) => New(format!("{prev}; {e}")),
                                None => e,
                            });
                        }
                    }
                }
                match errs {
                    Some(e) if !remain.is_empty() => Err(e),
                    Some(e) => Err(e),
                    None => Ok(()),
                }
            },
            backoff,
        );
    }
}

/// `WaitRegionOnlineAttemptTimes_local`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
/// 留意空集合、取消上下文与默认值是否保持一致。
fn WaitRegionOnlineAttemptTimes_local() -> i32 {
    crate::split::WaitRegionOnlineAttemptTimes.load(Ordering::SeqCst)
}

/// `isUnsupportedError`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
/// 留意空集合、取消上下文与默认值是否保持一致。
pub(crate) fn isUnsupportedError(err: &SharedError) -> bool {
    let s = err.to_string().to_lowercase();
    s.contains("unimplemented") || s.contains("region 0 not found")
}

/// `isNonRetryErrForSplit`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
/// 留意空集合、取消上下文与默认值是否保持一致。
fn isNonRetryErrForSplit(err: &SharedError) -> bool {
    ErrInvalidRange.Equal(Some(err))
}

/// `BackoffRetryAllExcept`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
struct BackoffRetryAllExcept {
    state: stubs::RetryState,
    non_retry: fn(&SharedError) -> bool,
}

/// `NewBackoffRetryAllExceptStrategy`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
/// 留意空集合、取消上下文与默认值是否保持一致。
fn NewBackoffRetryAllExceptStrategy(
    times: i32,
    initial: Duration,
    max: Duration,
) -> BackoffRetryAllExcept {
    BackoffRetryAllExcept {
        state: InitialRetryState(times, initial, max),
        non_retry: isNonRetryErrForSplit,
    }
}

impl BackoffStrategy for BackoffRetryAllExcept {
    /// `NextBackoff`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    fn NextBackoff(&mut self, err: &SharedError) -> Duration {
        if (self.non_retry)(err) {
            self.state.GiveUp();
            return Duration::ZERO;
        }
        self.state.ExponentialBackoff()
    }
    /// `RemainingAttempts`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    fn RemainingAttempts(&self) -> i32 {
        self.state.RemainingAttempts()
    }
}

impl SplitClient for PdClient {
    /// `GetStore`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    fn GetStore(
        &self,
        _ctx: &Context,
        storeID: u64,
        _opts: &[GetStoreOption],
    ) -> Result<metapb::Store> {
        if let Some(s) = self.storeCache.lock().unwrap().get(&storeID) {
            return Ok(s.clone());
        }
        let store = self.backend.GetStore(storeID)?;
        self.storeCache
            .lock()
            .unwrap()
            .insert(storeID, store.clone());
        Ok(store)
    }

    /// `GetRegion`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    fn GetRegion(&self, _ctx: &Context, key: &[u8]) -> Result<RegionInfo> {
        self.backend.GetRegion(key)
    }

    /// `GetRegionByID`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    fn GetRegionByID(&self, _ctx: &Context, regionID: u64) -> Result<RegionInfo> {
        self.backend.GetRegionByID(regionID)
    }

    /// `SplitKeysAndScatter`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    fn SplitKeysAndScatter(
        &self,
        ctx: &Context,
        sortedSplitKeys: &[Vec<u8>],
    ) -> Result<Vec<RegionInfo>> {
        self.splitKeys(ctx, sortedSplitKeys, true)
    }

    /// `SplitKeys`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    fn SplitKeys(&self, ctx: &Context, sortedSplitKeys: &[Vec<u8>]) -> Result<Vec<RegionInfo>> {
        self.splitKeys(ctx, sortedSplitKeys, false)
    }

    /// `SplitWaitAndScatter`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    fn SplitWaitAndScatter(
        &self,
        ctx: &Context,
        region: &RegionInfo,
        keys: &[Vec<u8>],
    ) -> Result<Vec<RegionInfo>> {
        self.splitWaitAndMaybeScatter(ctx, region, keys, true)
    }

    /// `GetOperator`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    fn GetOperator(&self, _ctx: &Context, regionID: u64) -> Result<pdpb::GetOperatorResponse> {
        self.backend.GetOperator(regionID)
    }

    /// `ScanRegions`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    fn ScanRegions(
        &self,
        _ctx: &Context,
        key: &[u8],
        endKey: &[u8],
        limit: i32,
        opts: &[GetRegionOption],
    ) -> Result<Vec<RegionInfo>> {
        let allow = opts.iter().any(|o| o.allow_follower);
        self.backend.ScanRegions(key, endKey, limit, allow)
    }

    /// `GetPlacementRule`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    fn GetPlacementRule(
        &self,
        _ctx: &Context,
        groupID: &str,
        ruleID: &str,
    ) -> Result<pdhttp::Rule> {
        self.http
            .as_ref()
            .ok_or_else(|| New("http missing"))?
            .GetPlacementRule(groupID, ruleID)
    }

    /// `SetPlacementRule`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    fn SetPlacementRule(&self, _ctx: &Context, rule: &pdhttp::Rule) -> Result<()> {
        self.http
            .as_ref()
            .ok_or_else(|| New("http missing"))?
            .SetPlacementRule(rule)
    }

    /// `DeletePlacementRule`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    fn DeletePlacementRule(&self, _ctx: &Context, groupID: &str, ruleID: &str) -> Result<()> {
        self.http
            .as_ref()
            .ok_or_else(|| New("http missing"))?
            .DeletePlacementRule(groupID, ruleID)
    }

    /// `SetStoresLabel`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    fn SetStoresLabel(
        &self,
        _ctx: &Context,
        stores: &[u64],
        labelKey: &str,
        labelValue: &str,
    ) -> Result<()> {
        let http = self.http.as_ref().ok_or_else(|| New("http missing"))?;
        for id in stores {
            http.SetStoreLabels(*id, labelKey, labelValue)?;
        }
        Ok(())
    }

    /// `WaitRegionsScattered`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    fn WaitRegionsScattered(
        &self,
        ctx: &Context,
        regionInfos: &[RegionInfo],
    ) -> Result<(i32, SharedError)> {
        let mut regions = regionInfos.to_vec();
        let mut backoffer = NewBackoffMayNotCountBackoffer();
        let mut needRecheck: Vec<RegionInfo> = Vec::new();
        let result = WithRetryReturnLastErr(
            ctx,
            || {
                needRecheck.clear();
                let mut needRescatter = Vec::new();
                for (i, region) in regions.iter().enumerate() {
                    let id = region.Region.as_ref().map(|m| m.Id).unwrap_or(0);
                    match self.checkScatterOperator(id) {
                        Ok((true, _)) => {}
                        Ok((false, rescatter)) => {
                            needRecheck.push(region.clone());
                            if rescatter {
                                needRescatter.push(region.clone());
                            }
                        }
                        Err(e) => {
                            // non-retryable operator errors stop immediately
                            if e.to_string().contains("DATA_COMPACTED")
                                || e.to_string().contains("get operator error")
                            {
                                needRecheck.extend(regions[i..].iter().cloned());
                                return Err(e);
                            }
                            needRecheck.push(region.clone());
                        }
                    }
                }
                if needRecheck.is_empty() {
                    return Ok(());
                }
                let backoffErr = if needRecheck.len() < regions.len() {
                    ErrBackoffAndDontCount()
                } else {
                    ErrBackoff()
                };
                regions = needRecheck.clone();
                if !needRescatter.is_empty() {
                    if let Err(scatterErr) = self.scatterRegions(ctx, &needRescatter) {
                        return Err(
                            Annotate(Some(backoffErr), scatterErr.to_string()).expect("annotate")
                        );
                    }
                }
                Err(Annotatef(
                    Some(backoffErr),
                    &format!(
                        "scatter region not finished, needRecheck: {}, needRescatter: {}, the first unfinished region: id:{}",
                        needRecheck.len(),
                        needRescatter.len(),
                        needRecheck[0].Region.as_ref().map(|r| r.Id).unwrap_or(0)
                    ),
                    &[],
                )
                .expect("annotate"))
            },
            &mut backoffer,
        );
        match result {
            Ok(()) => Ok((0, New(""))),
            Err(e) => Ok((needRecheck.len() as i32, e)),
        }
    }

    /// `GetCodecPDClient`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    fn GetCodecPDClient(&self) -> Option<CodecPDClient> {
        if !self.isCodecPDClient {
            return None;
        }
        self.codecClient.clone()
    }
}

impl PdClient {
    /// `checkScatterOperator`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    fn checkScatterOperator(&self, regionID: u64) -> Result<(bool, bool)> {
        let resp = self.backend.GetOperator(regionID)?;
        isScatterRegionFinished(&resp)
    }
}

/// Returns (scatterDone, needRescatter).
/// `isScatterRegionFinished`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
/// 留意空集合、取消上下文与默认值是否保持一致。
pub fn isScatterRegionFinished(resp: &pdpb::GetOperatorResponse) -> Result<(bool, bool)> {
    if let Some(header) = &resp.Header {
        if let Some(err) = &header.Error {
            if err.Type == pdpb::ErrorType::REGION_NOT_FOUND {
                return Ok((true, false));
            }
            return Err(Annotatef(
                Some(SharedError::new(ErrPDInvalidResponse.clone())),
                &format!(
                    "get operator error: {}, error message: {}",
                    err.Type.as_str(),
                    err.Message
                ),
                &[],
            )
            .expect("annotate"));
        }
    }
    if String::from_utf8_lossy(&resp.Desc) != "scatter-region" {
        return Ok((true, false));
    }
    match resp.Status {
        pdpb::OperatorStatus::SUCCESS => Ok((true, false)),
        pdpb::OperatorStatus::RUNNING => Ok((false, false)),
        _ => Ok((false, true)),
    }
}

/// `ExponentialBackoffer`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
pub struct ExponentialBackoffer {
    pub attempt: i32,
    pub max: i32,
    pub base: Duration,
    pub cap: Duration,
    pub next: Duration,
}

impl ExponentialBackoffer {
    /// `exponentialBackoff`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    fn exponentialBackoff(&mut self) -> Duration {
        self.attempt += 1;
        let d = self.next;
        self.next = self.next.saturating_mul(2);
        if self.next > self.cap {
            self.next = self.cap;
        }
        d
    }

    /// `NextBackoff`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    pub fn NextBackoff(&mut self, _err: SharedError) -> Duration {
        self.exponentialBackoff()
    }

    /// `Attempt`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    pub fn Attempt(&self) -> i32 {
        self.max - self.attempt
    }
}

/// `PdErrorCanRetry`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
/// 留意空集合、取消上下文与默认值是否保持一致。
pub fn PdErrorCanRetry(err: &SharedError) -> bool {
    // Mirrors Go PdErrorCanRetry: gRPC-ish scatter rejection messages.
    let s = err.to_string();
    s.contains("is not fully replicated")
        || s.contains("has no leader")
        || s.contains("cannot add an operator to the execute queue")
        || s.contains("failed to create scatter region operator")
}

/// `br_err`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
/// 留意空集合、取消上下文与默认值是否保持一致。
fn br_err(err: &'static astersql_errors::Error) -> SharedError {
    SharedError::new(err.clone())
}

#[allow(dead_code)]
/// `annotate_invalid_range`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
/// 留意空集合、取消上下文与默认值是否保持一致。
fn annotate_invalid_range(msg: &str) -> SharedError {
    Annotatef(Some(br_err(&ErrInvalidRange)), msg, &[]).expect("annotate")
}

/// CheckRegionEpoch moved to split.rs; re-export name used by client.go callers.
pub use crate::split::CheckRegionEpoch;
