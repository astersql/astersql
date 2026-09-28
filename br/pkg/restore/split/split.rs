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

//! Region 拆分执行器与扫描辅助，对齐 Go `split.go`。
//! 负责按规则拆分 region、等待 online/scatter，以及分页扫描 region。
//! 重试计数用线程局部存储，避免并行测试互相污染（相对 Go 包级变量）。
//! PD/TiKV 调用经 SplitClient 注入，便于单测替身。
//! Region split executor and scan helpers, matching `split.go`.

use std::cell::Cell;
use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::time::Duration;

use astersql_br_pkg_errors::{ErrInvalidRange, ErrPDBatchScanRegion, Is as BrIs};
use astersql_errors::{Annotatef, Cause, New, SharedError};

use crate::client::SplitClient;
use crate::region::RegionInfo;
use crate::stubs::{
    self, BackoffStrategy, Context, HINT_SCAN_REGION_BACKOFF, InitialRetryState, Result,
    RetryState, WithAllowFollowerHandle, WithRetry, codec, log, logutil, redact,
};

// Default matching lightning config.DefaultRegionCheckBackoffLimit.
thread_local! {
    /// `WAIT_REGION_ONLINE_ATTEMPT_TIMES`：与 Go 同名常量/静态对齐。
    static WAIT_REGION_ONLINE_ATTEMPT_TIMES: Cell<i32> = const { Cell::new(1800) };
    /// `SPLIT_RETRY_TIMES`：与 Go 同名常量/静态对齐。
    static SPLIT_RETRY_TIMES: Cell<i32> = const { Cell::new(150) };
}

#[derive(Clone, Copy)]
// 重试计数类别：等待 online 或拆分。
enum RetryCounterKind {
    WaitRegionOnline,
    Split,
}

/// Thread-local test override with the same load/store surface as AtomicI32.
/// （中文）Thread-local test override with the same load/store surface as AtomicI32.
///
/// The Go package variables are overridden by individual tests. Rust executes
/// （中文）The Go package variables are overridden by individual tests. Rust executes
/// those tests in parallel, so process-global atomics let one test change
/// （中文）those tests in parallel, so process-global atomics let one test change
/// another test's retry contract.
/// （中文）another test's retry contract.
pub struct RetryCounter(RetryCounterKind);

// impl RetryCounter {：方法实现见下。
impl RetryCounter {
    // `fn`：见函数体控制流与错误处理。
    const fn new(kind: RetryCounterKind) -> Self {
        Self(kind)
    }

    /// `load`：见函数体控制流与错误处理。
    pub fn load(&self, _ordering: Ordering) -> i32 {
        match self.0 {
            // 分支匹配：各臂处理不同结果/错误。
            RetryCounterKind::WaitRegionOnline => WAIT_REGION_ONLINE_ATTEMPT_TIMES.get(),
            RetryCounterKind::Split => SPLIT_RETRY_TIMES.get(),
        }
    }

    /// `store`：见函数体控制流与错误处理。
    pub fn store(&self, value: i32, _ordering: Ordering) {
        match self.0 {
            // 分支匹配：各臂处理不同结果/错误。
            RetryCounterKind::WaitRegionOnline => WAIT_REGION_ONLINE_ATTEMPT_TIMES.set(value),
            RetryCounterKind::Split => SPLIT_RETRY_TIMES.set(value),
        }
    }
}

/// 等待 region online 的最大尝试次数（线程局部）。
pub static WaitRegionOnlineAttemptTimes: RetryCounter =
    RetryCounter::new(RetryCounterKind::WaitRegionOnline);
/// 拆分重试最大次数（线程局部）。
pub static SplitRetryTimes: RetryCounter = RetryCounter::new(RetryCounterKind::Split);

/// 拆分重试初始间隔。
pub const SplitRetryInterval: Duration = Duration::from_millis(50);
/// 拆分重试最大间隔。
pub const SplitMaxRetryInterval: Duration = Duration::from_secs(4);
/// 等待 scatter 的上限时长。
pub const ScatterWaitUpperInterval: Duration = Duration::from_secs(30 * 60);
/// 分页扫描 region 的每页上限。
pub const ScanRegionPaginationLimit: i32 = 128;
/// 默认 region 索引步长。
pub const DefaultRegionIndexStep: u32 = 128;

/// 步长为 0 时回落到默认值。
pub fn NormalizeRegionIndexStep(regionIndexStep: u32) -> u32 {
    if regionIndexStep == 0 {
        // 条件分支：见块内处理与 Go 对齐点。
        DefaultRegionIndexStep
    } else {
        regionIndexStep
    }
}

/// RegionSplitter executes region splits by rules.
/// （中文）RegionSplitter executes region splits by rules.
pub struct RegionSplitter {
    pub client: Box<dyn SplitClient>,
    pub regionIndexStep: u32,
    pub coarseScatter: bool,
}

/// 以默认 regionIndexStep 构造 RegionSplitter。
pub fn NewRegionSplitter(client: Box<dyn SplitClient>) -> RegionSplitter {
    NewRegionSplitterWithRegionIndexStep(client, DefaultRegionIndexStep)
}

/// 指定 region 索引步长构造拆分器。
pub fn NewRegionSplitterWithRegionIndexStep(
    client: Box<dyn SplitClient>,
    regionIndexStep: u32,
) -> RegionSplitter {
    RegionSplitter {
        client,
        regionIndexStep: NormalizeRegionIndexStep(regionIndexStep),
        coarseScatter: false,
    }
}

// impl RegionSplitter {：方法实现见下。
impl RegionSplitter {
    /// `SetCoarseScatter`：见函数体控制流与错误处理。
    pub fn SetCoarseScatter(&mut self, coarseScatter: bool) {
        self.coarseScatter = coarseScatter;
    }

    /// 在指定 region 上对有序键拆分。
    pub fn ExecuteSortedKeysOnRegion(
        &self,
        ctx: &Context,
        region: &RegionInfo,
        keys: &[Vec<u8>],
    ) -> Result<Vec<RegionInfo>> {
        self.client.SplitWaitAndScatter(ctx, region, keys)
    }

    /// 对全局有序键执行拆分。
    pub fn ExecuteSortedKeys(&self, ctx: &Context, sortedSplitKeys: &[Vec<u8>]) -> Result<()> {
        if sortedSplitKeys.is_empty() {
            // 条件分支：见块内处理与 Go 对齐点。
            log::Info("skip split regions, no split keys");
            return Ok(());
        }
        log::Info("execute split sorted keys");
        self.executeSplitByRanges(ctx, sortedSplitKeys)
    }

    // `executeSplitByRanges`：见函数体控制流与错误处理。
    fn executeSplitByRanges(&self, ctx: &Context, sortedKeys: &[Vec<u8>]) -> Result<()> {
        let mut roughSortedSplitKeys: Vec<Vec<u8>> = Vec::new();
        if (self.regionIndexStep as usize) < sortedKeys.len() {
            // 条件分支：见块内处理与 Go 对齐点。
            let step = self.regionIndexStep as usize;
            let mut cur = step;
            while cur < sortedKeys.len() {
                roughSortedSplitKeys.push(sortedKeys[cur].clone());
                cur += step;
            }
        }
        if !roughSortedSplitKeys.is_empty() {
            // 条件分支：见块内处理与 Go 对齐点。
            self.executeSplitByKeys(ctx, &roughSortedSplitKeys, true)?;
        }
        self.executeSplitByKeys(ctx, sortedKeys, !self.coarseScatter)?;
        Ok(())
    }

    // `executeSplitByKeys`：见函数体控制流与错误处理。
    fn executeSplitByKeys(
        &self,
        ctx: &Context,
        sortedKeys: &[Vec<u8>],
        scatter: bool,
    ) -> Result<()> {
        let scatterRegions = self.splitKeys(ctx, sortedKeys, scatter)?;
        if !scatterRegions.is_empty() {
            // 条件分支：见块内处理与 Go 对齐点。
            self.waitRegionsScattered(ctx, &scatterRegions, ScatterWaitUpperInterval);
        }
        Ok(())
    }

    // `splitKeys`：见函数体控制流与错误处理。
    fn splitKeys(
        &self,
        ctx: &Context,
        sortedKeys: &[Vec<u8>],
        scatter: bool,
    ) -> Result<Vec<RegionInfo>> {
        if scatter {
            // 条件分支：见块内处理与 Go 对齐点。
            self.client.SplitKeysAndScatter(ctx, sortedKeys)
        } else {
            self.client.SplitKeys(ctx, sortedKeys)
        }
    }

    // `waitRegionsScattered`：见函数体控制流与错误处理。
    fn waitRegionsScattered(
        &self,
        ctx: &Context,
        scatterRegions: &[RegionInfo],
        timeout: Duration,
    ) {
        let _ = self.WaitForScatterRegionsTimeout(ctx, scatterRegions, timeout);
    }

    /// 等待 scatter，超时返回未完成数。
    pub fn WaitForScatterRegionsTimeout(
        &self,
        ctx: &Context,
        regionInfos: &[RegionInfo],
        timeout: Duration,
    ) -> i32 {
        let (ctx2, _cancel) = Context::WithTimeout(ctx, timeout);
        let (left, _) = self
            .client
            .WaitRegionsScattered(&ctx2, regionInfos)
            .unwrap_or((regionInfos.len() as i32, New("scatter wait")));
        left
    }
}

// `br_err`：见函数体控制流与错误处理。
fn br_err(err: &'static astersql_errors::Error) -> SharedError {
    SharedError::new(err.clone())
}

/// checkRegionConsistency checks readiness and continuity of regions.
/// （中文）checkRegionConsistency checks readiness and continuity of regions.
pub fn checkRegionConsistency(
    startKey: &[u8],
    endKey: &[u8],
    regions: &[RegionInfo],
    limitted: bool,
) -> Result<()> {
    if regions.is_empty() {
        // 条件分支：见块内处理与 Go 对齐点。
        let msg = format!(
            "scan region return empty result, startKey: {}, endKey: {}",
            redact::Key(startKey),
            redact::Key(endKey)
        );
        return Err(Annotatef(Some(br_err(&ErrPDBatchScanRegion)), &msg, &[]).expect("annotate"));
        // 错误路径：向上返回，保留上下文。
    }

    let first = regions[0]
        .Region
        .as_ref()
        .ok_or_else(|| New("nil region"))?;
    if first.StartKey.as_slice() > startKey {
        // 条件分支：见块内处理与 Go 对齐点。
        let epoch = first
            .RegionEpoch
            .as_ref()
            .map(|e| e.String())
            .unwrap_or_default();
        let msg = format!(
            "first region {}'s startKey({}) > startKey({}), region epoch: {}",
            first.Id,
            redact::Key(&first.StartKey),
            redact::Key(startKey),
            epoch
        );
        return Err(Annotatef(Some(br_err(&ErrPDBatchScanRegion)), &msg, &[]).expect("annotate"));
        // 错误路径：向上返回，保留上下文。
    } else if !limitted
        && !regions[regions.len() - 1]
            .Region
            .as_ref()
            .map(|r| r.EndKey.is_empty())
            .unwrap_or(true)
        && regions[regions.len() - 1]
            .Region
            .as_ref()
            .map(|r| r.EndKey.as_slice() < endKey)
            .unwrap_or(false)
    {
        let last = regions[regions.len() - 1].Region.as_ref().unwrap();
        let epoch = last
            .RegionEpoch
            .as_ref()
            .map(|e| e.String())
            .unwrap_or_default();
        let msg = format!(
            "last region {}'s endKey({}) < endKey({}), region epoch: {}",
            last.Id,
            redact::Key(&last.EndKey),
            redact::Key(endKey),
            epoch
        );
        return Err(Annotatef(Some(br_err(&ErrPDBatchScanRegion)), &msg, &[]).expect("annotate"));
        // 错误路径：向上返回，保留上下文。
    }

    let mut cur = &regions[0];
    if cur.Leader.is_none() {
        // 条件分支：见块内处理与 Go 对齐点。
        let msg = format!(
            "region {}'s leader is nil",
            cur.Region.as_ref().map(|r| r.Id).unwrap_or(0)
        );
        return Err(Annotatef(Some(br_err(&ErrPDBatchScanRegion)), &msg, &[]).expect("annotate"));
        // 错误路径：向上返回，保留上下文。
    }
    if cur.Leader.as_ref().map(|l| l.StoreId).unwrap_or(0) == 0 {
        // 条件分支：见块内处理与 Go 对齐点。
        let msg = format!(
            "region {}'s leader's store id is 0",
            cur.Region.as_ref().map(|r| r.Id).unwrap_or(0)
        );
        return Err(Annotatef(Some(br_err(&ErrPDBatchScanRegion)), &msg, &[]).expect("annotate"));
        // 错误路径：向上返回，保留上下文。
    }
    for r in &regions[1..] {
        if r.Leader.is_none() {
            // 条件分支：见块内处理与 Go 对齐点。
            let msg = format!(
                "region {}'s leader is nil",
                r.Region.as_ref().map(|x| x.Id).unwrap_or(0)
            );
            // 错误路径：向上返回，保留上下文。
            return Err(
                Annotatef(Some(br_err(&ErrPDBatchScanRegion)), &msg, &[]).expect("annotate")
            );
        }
        if r.Leader.as_ref().map(|l| l.StoreId).unwrap_or(0) == 0 {
            // 条件分支：见块内处理与 Go 对齐点。
            let msg = format!(
                "region {}'s leader's store id is 0",
                r.Region.as_ref().map(|x| x.Id).unwrap_or(0)
            );
            // 错误路径：向上返回，保留上下文。
            return Err(
                Annotatef(Some(br_err(&ErrPDBatchScanRegion)), &msg, &[]).expect("annotate")
            );
        }
        let cur_end = cur
            .Region
            .as_ref()
            .map(|x| x.EndKey.as_slice())
            .unwrap_or(&[]);
        let next_start = r
            .Region
            .as_ref()
            .map(|x| x.StartKey.as_slice())
            .unwrap_or(&[]);
        if cur_end != next_start {
            // 条件分支：见块内处理与 Go 对齐点。
            let msg = format!(
                "region {}'s endKey not equal to next region {}'s startKey, endKey: {}, startKey: {}, region epoch: {} {}",
                cur.Region.as_ref().map(|x| x.Id).unwrap_or(0),
                r.Region.as_ref().map(|x| x.Id).unwrap_or(0),
                redact::Key(cur_end),
                redact::Key(next_start),
                cur.Region
                    .as_ref()
                    .and_then(|x| x.RegionEpoch.as_ref())
                    .map(|e| e.String())
                    .unwrap_or_default(),
                r.Region
                    .as_ref()
                    .and_then(|x| x.RegionEpoch.as_ref())
                    .map(|e| e.String())
                    .unwrap_or_default(),
            );
            // 错误路径：向上返回，保留上下文。
            return Err(
                Annotatef(Some(br_err(&ErrPDBatchScanRegion)), &msg, &[]).expect("annotate")
            );
        }
        cur = r;
    }
    Ok(())
}

/// 限定数量的带重试扫描。
pub fn scanRegionsLimitWithRetry(
    ctx: &Context,
    client: &dyn SplitClient,
    startKey: &[u8],
    endKey: &[u8],
    limit: i32,
    mut mustLeader: bool,
) -> Result<(Vec<RegionInfo>, bool)> {
    let mut batch = Vec::new();
    let mut err: Option<SharedError> = None;
    let mut backoffer = NewWaitRegionOnlineBackoffer();
    let _ = WithRetry(
        ctx,
        || {
            mustLeader = mustLeader || err.is_some();
            let opts = if mustLeader {
                vec![]
            } else {
                vec![WithAllowFollowerHandle()]
            };
            match client.ScanRegions(ctx, startKey, endKey, limit, &opts) {
                // 分支匹配：各臂处理不同结果/错误。
                Ok(b) => {
                    batch = b;
                    if let Err(e) = checkRegionConsistency(startKey, endKey, &batch, true) {
                        // 条件分支：见块内处理与 Go 对齐点。
                        log::Warn("failed to scan region, retrying");
                        err = Some(e.clone());
                        return Err(e);
                        // 错误路径：向上返回，保留上下文。
                    }
                    err = None;
                    Ok(())
                }
                Err(e) => {
                    // 错误路径：向上返回，保留上下文。
                    err = Some(e.clone());
                    Err(e)
                    // 错误路径：向上返回，保留上下文。
                }
            }
        },
        &mut backoffer,
    );
    match err {
        // 分支匹配：各臂处理不同结果/错误。
        Some(e) => Err(e),
        None => Ok((batch, mustLeader)),
    }
}

/// 分页扫描，键经 codec 编解码。
pub fn PaginateScanRegionWithCodecAware(
    ctx: &Context,
    client: &dyn SplitClient,
    mut startKey: Vec<u8>,
    mut endKey: Vec<u8>,
    limit: i32,
) -> Result<Vec<RegionInfo>> {
    let mut encodeRegionRange: Option<Box<dyn Fn(&[u8], &[u8]) -> (Vec<u8>, Vec<u8>)>> = None;
    if let Some(codecCli) = client.GetCodecPDClient() {
        // 条件分支：见块内处理与 Go 对齐点。
        let cd = codecCli.GetCodec();
        let (s, e) = cd.DecodeRange(&startKey, &endKey)?;
        startKey = s;
        endKey = e;
        encodeRegionRange = Some(Box::new(move |a, b| cd.EncodeRegionRange(a, b)));
    } else {
        startKey = codec::EncodeBytes(Vec::new(), &startKey);
        endKey = codec::EncodeBytes(Vec::new(), &endKey);
    }
    let mut regions = PaginateScanRegion(ctx, client, &startKey, &endKey, limit)?;
    if let Some(enc) = encodeRegionRange {
        // 条件分支：见块内处理与 Go 对齐点。
        encodeRegionKeys(&mut regions, &enc);
    }
    Ok(regions)
}

/// `encodeRegionKeys`：见函数体控制流与错误处理。
pub fn encodeRegionKeys(
    regions: &mut [RegionInfo],
    encodeRegionRange: &dyn Fn(&[u8], &[u8]) -> (Vec<u8>, Vec<u8>),
) {
    for region in regions.iter_mut() {
        if let Some(meta) = region.Region.as_mut() {
            // 条件分支：见块内处理与 Go 对齐点。
            let (s, e) = encodeRegionRange(&meta.StartKey, &meta.EndKey);
            meta.StartKey = s;
            meta.EndKey = e;
        }
    }
}

/// 分页扫描 region（非 codec 感知）。
pub fn PaginateScanRegion(
    ctx: &Context,
    client: &dyn SplitClient,
    startKey: &[u8],
    endKey: &[u8],
    limit: i32,
) -> Result<Vec<RegionInfo>> {
    if !endKey.is_empty() && startKey > endKey {
        // 条件分支：见块内处理与 Go 对齐点。
        let msg = format!(
            "startKey > endKey, startKey: {}, endkey: {}",
            hex::encode(startKey),
            hex::encode(endKey)
        );
        return Err(Annotatef(Some(br_err(&ErrInvalidRange)), &msg, &[]).expect("annotate"));
        // 错误路径：向上返回，保留上下文。
    }

    let mut lastRegions: Vec<RegionInfo> = Vec::new();
    let mut err: Option<SharedError> = None;
    let mut mustLeader = false;
    let mut backoffer = NewWaitRegionOnlineBackoffer();

    while backoffer.RemainingAttempts() > 0 {
        // Go: defer mustLeader = true after each outer WithRetry attempt.
        let mut regions: Vec<RegionInfo> = Vec::with_capacity(16);
        let mut scanStartKey = startKey.to_vec();
        let mut scan_err: Option<SharedError> = None;
        let mut ml = mustLeader;
        loop {
            match scanRegionsLimitWithRetry(ctx, client, &scanStartKey, endKey, limit, ml) {
                // 分支匹配：各臂处理不同结果/错误。
                Ok((batch, next_ml)) => {
                    ml = next_ml;
                    mustLeader = next_ml;
                    let batch_len = batch.len();
                    regions.extend(batch);
                    if (batch_len as i32) < limit {
                        // 条件分支：见块内处理与 Go 对齐点。
                        break;
                        // 结束当前循环。
                    }
                    scanStartKey = regions
                        .last()
                        .and_then(|r| r.Region.as_ref())
                        .map(|r| r.EndKey.clone())
                        .unwrap_or_default();
                    if scanStartKey.is_empty()
                    // 条件分支：见块内处理与 Go 对齐点。
                        || (!endKey.is_empty() && scanStartKey.as_slice() >= endKey)
                    {
                        break;
                        // 结束当前循环。
                    }
                }
                Err(e) => {
                    // 错误路径：向上返回，保留上下文。
                    let msg = format!(
                        "scan regions from start-key:{}, err: {}",
                        redact::Key(&scanStartKey),
                        e
                    );
                    let wrapped = Annotatef(
                        Some(
                            ErrPDBatchScanRegion
                                .Wrap(Some(e))
                                .map(SharedError::new)
                                .unwrap_or_else(|| br_err(&ErrPDBatchScanRegion)),
                        ),
                        &msg,
                        &[],
                    )
                    .expect("annotate");
                    scan_err = Some(wrapped);
                    break;
                    // 结束当前循环。
                }
            }
        }
        // Match Go defer: force leader-only on subsequent outer attempts.
        mustLeader = true;
        if let Some(e) = scan_err {
            // 条件分支：见块内处理与 Go 对齐点。
            err = Some(e.clone());
            if ctx.Done() {
                // 条件分支：见块内处理与 Go 对齐点。
                return Err(e);
                // 错误路径：向上返回，保留上下文。
            }
            let delay = backoffer.NextBackoff(&e);
            if delay.is_zero() && backoffer.RemainingAttempts() <= 0 {
                // 条件分支：见块内处理与 Go 对齐点。
                break;
                // 结束当前循环。
            }
            continue;
            // 继续下一轮重试或迭代。
        }
        if regions.len() != lastRegions.len() {
            // 条件分支：见块内处理与 Go 对齐点。
            backoffer.Stat.ReduceRetry();
        }
        lastRegions = regions;
        match checkRegionConsistency(startKey, endKey, &lastRegions, false) {
            // 分支匹配：各臂处理不同结果/错误。
            Ok(()) => {
                err = None;
                break;
                // 结束当前循环。
            }
            Err(e) => {
                // 错误路径：向上返回，保留上下文。
                log::Warn("failed to scan region, retrying");
                err = Some(e.clone());
                if ctx.Done() {
                    // 条件分支：见块内处理与 Go 对齐点。
                    return Err(e);
                    // 错误路径：向上返回，保留上下文。
                }
                let _ = backoffer.NextBackoff(&e);
            }
        }
    }
    match err {
        // 分支匹配：各臂处理不同结果/错误。
        Some(e) => Err(e),
        None => Ok(lastRegions),
    }
}

/// 部分一致性检查（允许缺口策略）。
pub fn checkPartRegionConsistency(
    startKey: &[u8],
    endKey: &[u8],
    regions: &[RegionInfo],
) -> Result<()> {
    let _ = endKey;
    if regions.is_empty() {
        // 条件分支：见块内处理与 Go 对齐点。
        let msg = format!(
            "scan region return empty result, startKey: {}, endKey: {}",
            redact::Key(startKey),
            redact::Key(endKey)
        );
        return Err(Annotatef(Some(br_err(&ErrPDBatchScanRegion)), &msg, &[]).expect("annotate"));
        // 错误路径：向上返回，保留上下文。
    }
    let first = regions[0].Region.as_ref().unwrap();
    if first.StartKey.as_slice() > startKey {
        // 条件分支：见块内处理与 Go 对齐点。
        let msg = format!(
            "first region's startKey > startKey, startKey: {}, regionStartKey: {}",
            redact::Key(startKey),
            redact::Key(&first.StartKey)
        );
        return Err(Annotatef(Some(br_err(&ErrPDBatchScanRegion)), &msg, &[]).expect("annotate"));
        // 错误路径：向上返回，保留上下文。
    }
    let mut cur = &regions[0];
    for r in &regions[1..] {
        let cur_end = cur
            .Region
            .as_ref()
            .map(|x| x.EndKey.as_slice())
            .unwrap_or(&[]);
        let next_start = r
            .Region
            .as_ref()
            .map(|x| x.StartKey.as_slice())
            .unwrap_or(&[]);
        if cur_end != next_start {
            // 条件分支：见块内处理与 Go 对齐点。
            let msg = format!(
                "region endKey not equal to next region startKey, endKey: {}, startKey: {}",
                redact::Key(cur_end),
                redact::Key(next_start)
            );
            // 错误路径：向上返回，保留上下文。
            return Err(
                Annotatef(Some(br_err(&ErrPDBatchScanRegion)), &msg, &[]).expect("annotate")
            );
        }
        cur = r;
    }
    Ok(())
}

/// 带重试的 region 扫描。
pub fn ScanRegionsWithRetry(
    ctx: &Context,
    client: &dyn SplitClient,
    startKey: &[u8],
    endKey: &[u8],
    limit: i32,
) -> Result<Vec<RegionInfo>> {
    if !endKey.is_empty() && startKey > endKey {
        // 条件分支：见块内处理与 Go 对齐点。
        let msg = format!(
            "startKey > endKey, startKey: {}, endkey: {}",
            hex::encode(startKey),
            hex::encode(endKey)
        );
        return Err(Annotatef(Some(br_err(&ErrInvalidRange)), &msg, &[]).expect("annotate"));
        // 错误路径：向上返回，保留上下文。
    }

    let mut regions = Vec::new();
    let mut err: Option<SharedError> = None;
    let mut backoffer = NewWaitRegionOnlineBackoffer();
    let _ = WithRetry(
        ctx,
        || {
            let opts = if err.is_some() {
                vec![]
            } else {
                vec![WithAllowFollowerHandle()]
            };
            match client.ScanRegions(ctx, startKey, endKey, limit, &opts) {
                // 分支匹配：各臂处理不同结果/错误。
                Ok(r) => {
                    regions = r;
                    if let Err(e) = checkPartRegionConsistency(startKey, endKey, &regions) {
                        // 条件分支：见块内处理与 Go 对齐点。
                        log::Warn("failed to scan region, retrying");
                        err = Some(e.clone());
                        return Err(e);
                        // 错误路径：向上返回，保留上下文。
                    }
                    err = None;
                    Ok(())
                }
                Err(e) => {
                    // 错误路径：向上返回，保留上下文。
                    let msg = format!(
                        "scan regions from start-key:{}, err: {}",
                        redact::Key(startKey),
                        e
                    );
                    let wrapped = Annotatef(Some(br_err(&ErrPDBatchScanRegion)), &msg, &[])
                        .expect("annotate");
                    err = Some(wrapped.clone());
                    Err(wrapped)
                    // 错误路径：向上返回，保留上下文。
                }
            }
        },
        &mut backoffer,
    );
    match err {
        // 分支匹配：各臂处理不同结果/错误。
        Some(e) => Err(e),
        None => Ok(regions),
    }
}

/// `WaitRegionOnlineBackoffer`：结构体定义，字段语义见成员注释。
pub struct WaitRegionOnlineBackoffer {
    pub Stat: RetryState,
}

/// 构造等待 online 的 backoffer。
pub fn NewWaitRegionOnlineBackoffer() -> WaitRegionOnlineBackoffer {
    WaitRegionOnlineBackoffer {
        Stat: InitialRetryState(
            WaitRegionOnlineAttemptTimes.load(Ordering::SeqCst),
            Duration::from_millis(10),
            Duration::from_secs(2),
        ),
    }
}

// impl BackoffStrategy for WaitRegionOnlineBackoffer {：方法实现见下。
impl BackoffStrategy for WaitRegionOnlineBackoffer {
    // `NextBackoff`：见函数体控制流与错误处理。
    fn NextBackoff(&mut self, err: &SharedError) -> Duration {
        // Walk annotated/wrapped chain (Go errors.As on ErrPDBatchScanRegion).
        if BrIs(Some(err), &ErrPDBatchScanRegion) {
            // 条件分支：见块内处理与 Go 对齐点。
            let mut delay = self.Stat.ExponentialBackoff();
            if HINT_SCAN_REGION_BACKOFF.load(Ordering::SeqCst) {
                // 条件分支：见块内处理与 Go 对齐点。
                delay = Duration::from_micros(1);
            }
            return delay;
        }
        self.Stat.GiveUp();
        Duration::ZERO
    }

    // `RemainingAttempts`：见函数体控制流与错误处理。
    fn RemainingAttempts(&self) -> i32 {
        self.Stat.RemainingAttempts()
    }
}

/// `BackoffMayNotCountBackoffer`：结构体定义，字段语义见成员注释。
pub struct BackoffMayNotCountBackoffer {
    state: RetryState,
}

/// Sentinel errors matching Go `ErrBackoff` / `ErrBackoffAndDontCount`.
/// （中文）Sentinel errors matching Go `ErrBackoff` / `ErrBackoffAndDontCount`.
pub fn ErrBackoff() -> SharedError {
    New("found backoff error")
}

/// 退避但不消耗重试次数的错误。
pub fn ErrBackoffAndDontCount() -> SharedError {
    New("found backoff error but don't count")
}

/// 构造“失败未必计数”的 backoffer。
pub fn NewBackoffMayNotCountBackoffer() -> BackoffMayNotCountBackoffer {
    BackoffMayNotCountBackoffer {
        state: InitialRetryState(
            WaitRegionOnlineAttemptTimes.load(Ordering::SeqCst),
            Duration::from_millis(10),
            Duration::from_secs(2),
        ),
    }
}

// impl BackoffStrategy for BackoffMayNotCountBackoffer {：方法实现见下。
impl BackoffStrategy for BackoffMayNotCountBackoffer {
    // `NextBackoff`：见函数体控制流与错误处理。
    fn NextBackoff(&mut self, err: &SharedError) -> Duration {
        // Go errors.ErrorEqual compares the unwrapped causes, not arbitrary
        // wrapper text containing the sentinel message.
        let cause = Cause(Some(err)).unwrap_or_else(|| err.clone());
        let msg = cause.to_string();
        if msg == "found backoff error but don't count" {
            // 条件分支：见块内处理与 Go 对齐点。
            let delay = self.state.ExponentialBackoff();
            self.state.ReduceRetry();
            return delay;
        }
        if msg == "found backoff error" {
            // 条件分支：见块内处理与 Go 对齐点。
            return self.state.ExponentialBackoff();
        }
        self.state.GiveUp();
        Duration::ZERO
    }

    // `RemainingAttempts`：见函数体控制流与错误处理。
    fn RemainingAttempts(&self) -> i32 {
        self.state.RemainingAttempts()
    }
}

/// getSplitKeysOfRegions maps each region to the split keys that fall inside it.
/// （中文）getSplitKeysOfRegions maps each region to the split keys that fall inside it.
pub fn getSplitKeysOfRegions(
    sortedKeys: &[Vec<u8>],
    sortedRegions: &[RegionInfo],
    isRawKV: bool,
) -> HashMap<u64, Vec<Vec<u8>>> {
    let mut splitKeyMap: HashMap<u64, Vec<Vec<u8>>> = HashMap::new();
    if sortedKeys.is_empty() || sortedRegions.is_empty() {
        // 条件分支：见块内处理与 Go 对齐点。
        return splitKeyMap;
    }
    let mut curKeyIndex = 0usize;
    let mut splitKey = codec::EncodeBytesExt(Vec::new(), &sortedKeys[curKeyIndex], isRawKV);

    for region in sortedRegions {
        loop {
            if sortedKeys[curKeyIndex].is_empty() {
                // 条件分支：见块内处理与 Go 对齐点。
                curKeyIndex += 1;
                if curKeyIndex >= sortedKeys.len() {
                    // 条件分支：见块内处理与 Go 对齐点。
                    return splitKeyMap;
                }
                splitKey = codec::EncodeBytesExt(Vec::new(), &sortedKeys[curKeyIndex], isRawKV);
                continue;
                // 继续下一轮重试或迭代。
            }
            let meta = match region.Region.as_ref() {
                Some(m) => m,
                None => break,
            };
            if splitKey.as_slice() == meta.GetStartKey() {
                // 条件分支：见块内处理与 Go 对齐点。
                curKeyIndex += 1;
                if curKeyIndex >= sortedKeys.len() {
                    // 条件分支：见块内处理与 Go 对齐点。
                    return splitKeyMap;
                }
                splitKey = codec::EncodeBytesExt(Vec::new(), &sortedKeys[curKeyIndex], isRawKV);
                continue;
                // 继续下一轮重试或迭代。
            }
            if !region.ContainsInterior(&splitKey) {
                // 条件分支：见块内处理与 Go 对齐点。
                break;
                // 结束当前循环。
            }
            splitKeyMap
                .entry(meta.Id)
                .or_default()
                .push(sortedKeys[curKeyIndex].clone());
            curKeyIndex += 1;
            if curKeyIndex >= sortedKeys.len() {
                // 条件分支：见块内处理与 Go 对齐点。
                return splitKeyMap;
            }
            splitKey = codec::EncodeBytesExt(Vec::new(), &sortedKeys[curKeyIndex], isRawKV);
        }
    }
    if !sortedKeys.is_empty() && !sortedRegions.is_empty() {
        // 条件分支：见块内处理与 Go 对齐点。
        let lastKey = &sortedKeys[sortedKeys.len() - 1];
        let endOfLast = sortedRegions[sortedRegions.len() - 1]
            .Region
            .as_ref()
            .map(|r| r.GetEndKey())
            .unwrap_or(&[]);
        if lastKey.as_slice() != endOfLast {
            // 条件分支：见块内处理与 Go 对齐点。
            log::Error("in getSplitKeysOfRegions, regions don't cover all keys");
        }
    }
    splitKeyMap
}

/// 校验 region epoch 是否匹配。
pub fn CheckRegionEpoch(new: &RegionInfo, old: &RegionInfo) -> bool {
    match (&new.Region, &old.Region) {
        // 分支匹配：各臂处理不同结果/错误。
        (Some(n), Some(o)) => n.RegionEpoch == o.RegionEpoch,
        _ => false,
    }
}
