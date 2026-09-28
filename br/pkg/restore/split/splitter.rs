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

//! 拆分策略与 SplitPoint，对齐 Go `splitter.go`。
//! 在 RegionSplitter 之上定义按表累积、按阈值切分的流水线接口。
//! RewriteRules 将备份侧 tableID 映射到恢复侧，再决定 split 键。
//! Splitter strategies and SplitPoint, matching `splitter.go`.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use astersql_br_pkg_restore_utils::stubs::tablecodec;
use astersql_br_pkg_restore_utils::{GetRewriteEncodedKeys, GetRewriteTableID, RewriteRules};
use astersql_errors::{New, SharedError};

use crate::client::SplitClient;
use crate::region::RegionInfo;
use crate::split::{NewRegionSplitter, RegionSplitter, ScanRegionsWithRetry};
use crate::stubs::{Context, Result, codec, log, logutil};
use crate::sum_sorted::{NewValued, Span, SplitHelper, Value, Valued};

/// 基础拆分能力：单 region / 全量有序键拆分，以及等待 scatter 超时。
/// Splitter defines the interface for basic splitting strategies.
pub trait Splitter: Send + Sync {
    /// 在指定 region 上对有序键执行拆分。
    fn ExecuteSortedKeysOnRegion(
        &self,
        ctx: &Context,
        region: &RegionInfo,
        keys: &[Vec<u8>],
    ) -> Result<Vec<RegionInfo>>;

    /// 对全局有序键执行拆分（可跨 region）。
    fn ExecuteSortedKeys(&self, ctx: &Context, keys: &[Vec<u8>]) -> Result<()>;

    /// 等待 scatter，超时返回未完成数。
    fn WaitForScatterRegionsTimeout(
        &self,
        ctx: &Context,
        regionInfos: &[RegionInfo],
        timeout: Duration,
    ) -> i32;
}

// RegionSplitter 直接实现 Splitter，供策略层统一调用。
impl Splitter for RegionSplitter {
    fn ExecuteSortedKeysOnRegion(
        &self,
        ctx: &Context,
        region: &RegionInfo,
        keys: &[Vec<u8>],
    ) -> Result<Vec<RegionInfo>> {
        RegionSplitter::ExecuteSortedKeysOnRegion(self, ctx, region, keys)
    }

    fn ExecuteSortedKeys(&self, ctx: &Context, keys: &[Vec<u8>]) -> Result<()> {
        RegionSplitter::ExecuteSortedKeys(self, ctx, keys)
    }

    fn WaitForScatterRegionsTimeout(
        &self,
        ctx: &Context,
        regionInfos: &[RegionInfo],
        timeout: Duration,
    ) -> i32 {
        RegionSplitter::WaitForScatterRegionsTimeout(self, ctx, regionInfos, timeout)
    }
}

/// 累积策略：何时累加、何时跳过、何时触发拆分，以及导出/重置累积结果。
/// SplitStrategy defines how values should be accumulated and when to trigger a split.
pub trait SplitStrategy<T> {
    /// 将一项纳入累积。
    fn Accumulate(&mut self, v: T);
    /// 是否已达拆分阈值。
    fn ShouldSplit(&self) -> bool;
    /// 是否跳过该项（如无关表）。
    fn ShouldSkip(&self, v: &T) -> bool;
    /// 导出当前累积迭代器。
    fn GetAccumulations(&self) -> SplitHelperIterator;
    /// 重置累积状态。
    fn ResetAccumulations(&mut self);
}

/// 按表维护 SplitHelper，并持有 RewriteRules；计数驱动阈值判断。
pub struct BaseSplitStrategy {
    /// 已累积条目计数，供 ShouldSplit 类逻辑使用。
    pub AccumulateCount: i32,
    /// 旧 tableID → 该表的有值区间树。
    pub TableSplitter: HashMap<i64, SplitHelper>,
    /// 旧 tableID → 重写规则。
    pub Rules: HashMap<i64, RewriteRules>,
}

/// 以规则表初始化；TableSplitter 懒填充。
pub fn NewBaseSplitStrategy(rules: HashMap<i64, RewriteRules>) -> BaseSplitStrategy {
    BaseSplitStrategy {
        AccumulateCount: 0,
        TableSplitter: HashMap::new(),
        Rules: rules,
    }
}

// BaseSplitStrategy：导出/重置累积，供具体策略复用。
impl BaseSplitStrategy {
    /// 将各表 SplitHelper 克隆为 RewriteSplitter，按 RewriteKey 排序后导出。
    /// 无匹配规则时 Fatal；rewrite tableID 为 0 则 Warn 并跳过。
    pub fn GetAccumulations(&self) -> SplitHelperIterator {
        let mut tableSplitters: Vec<RewriteSplitter> = Vec::with_capacity(self.TableSplitter.len());
        for (tableID, splitter) in &self.TableSplitter {
            let rewriteRule = match self.Rules.get(tableID) {
                Some(r) => r,
                None => {
                    log::Fatal("[unreachable] no table id matched");
                    // 规则表与累积表不一致属编程错误，对齐 Go Fatal。
                }
            };
            let newTableID = GetRewriteTableID(*tableID, rewriteRule);
            if newTableID == 0 {
                log::Warn("failed to get the rewrite table id");
                // rewrite 后 tableID 无效则跳过该表，避免编码错误前缀。
                continue;
            }
            tableSplitters.push(NewRewriteSpliter(
                codec::EncodeBytes(Vec::new(), &tablecodec::EncodeTablePrefix(newTableID)),
                newTableID,
                rewriteRule.clone(),
                splitter.clone(),
            ));
        }
        tableSplitters.sort_by(|a, b| a.RewriteKey.cmp(&b.RewriteKey));
        // 按 RewriteKey 排序，保证跨表遍历顺序稳定。
        NewSplitHelperIterator(tableSplitters)
    }

    /// 清空按表累积并重置计数，供下一批流水线使用。
    pub fn ResetAccumulations(&mut self) {
        log::Info("reset accumulations");
        self.TableSplitter.clear();
        self.AccumulateCount = 0;
    }
}

/// 单表重写后的拆分状态：RewriteKey、新 tableID、规则与 SplitHelper。
pub struct RewriteSplitter {
    /// 重写后表前缀（encoded），用作排序键。
    pub RewriteKey: Vec<u8>,
    /// 重写后的新 tableID。
    pub tableID: i64,
    /// 该表适用的重写规则。
    pub rule: RewriteRules,
    /// 该表累积的有值区间。
    pub splitter: SplitHelper,
}

/// 构造 RewriteSplitter（函数名保留 Go 拼写 Spliter）。
pub fn NewRewriteSpliter(
    rewriteKey: Vec<u8>,
    tableID: i64,
    rule: RewriteRules,
    splitter: SplitHelper,
) -> RewriteSplitter {
    RewriteSplitter {
        RewriteKey: rewriteKey,
        tableID,
        rule,
        splitter,
    }
}

/// 多表 RewriteSplitter 的有序迭代器，供 SplitPoint 遍历。
pub struct SplitHelperIterator {
    /// 已按 RewriteKey 排序的多表状态。
    pub tableSplitters: Vec<RewriteSplitter>,
}

/// 包装已排序的 tableSplitters。
pub fn NewSplitHelperIterator(tableSplitters: Vec<RewriteSplitter>) -> SplitHelperIterator {
    SplitHelperIterator { tableSplitters }
}

// 迭代器遍历：按表 prefix 上界回调。
impl SplitHelperIterator {
    /// 逐表遍历；endKey 取下一 table prefix，回调可中断。
    pub fn Traverse<F>(&self, mut fn_: F)
    where
        F: FnMut(Valued, Vec<u8>, &RewriteRules) -> bool,
    {
        for entry in &self.tableSplitters {
            let endKey = codec::EncodeBytes(
                // 表上界：下一 tableID 的 encoded prefix。
                Vec::new(),
                &tablecodec::EncodeTablePrefix(entry.tableID + 1),
            );
            let rule = &entry.rule;
            let mut cont = true;
            entry.splitter.Traverse(|v| {
                cont = fn_(v, endKey.clone(), rule);
                cont
            });
            if !cont {
                break;
            }
        }
    }
}

/// 流水线拆分：在 Splitter 上增加按 SplitHelperIterator 执行。
pub trait PipelineRegionsSplitter: Splitter {
    /// 清空缓冲，按累积区间拆分，再最多等待 60s scatter。
    fn ExecuteRegions(&self, ctx: &Context, splitHelper: &SplitHelperIterator) -> Result<()>;
}

/// 阈值（Size/Keys）驱动的流水线实现；regions_buf 缓存待 scatter 区域。
pub struct PipelineRegionsSplitterImpl {
    /// 底层 region 拆分执行器。
    pub region_splitter: RegionSplitter,
    /// 触发拆分的 Size 阈值（字节）。
    pub splitThresholdSize: u64,
    /// 触发拆分的键条数阈值。
    pub splitThresholdKeys: i64,
    /// 本轮拆出的新区，供 scatter 等待。
    regions_buf: Mutex<Vec<RegionInfo>>,
}

/// 注入 SplitClient 与拆分阈值。
pub fn NewPipelineRegionsSplitter(
    client: Box<dyn SplitClient>,
    splitSize: u64,
    splitKeys: i64,
) -> PipelineRegionsSplitterImpl {
    PipelineRegionsSplitterImpl {
        region_splitter: NewRegionSplitter(client),
        splitThresholdSize: splitSize,
        splitThresholdKeys: splitKeys,
        regions_buf: Mutex::new(Vec::new()),
    }
}

// 委托给内部 RegionSplitter，保持 Splitter 契约。
impl Splitter for PipelineRegionsSplitterImpl {
    fn ExecuteSortedKeysOnRegion(
        &self,
        ctx: &Context,
        region: &RegionInfo,
        keys: &[Vec<u8>],
    ) -> Result<Vec<RegionInfo>> {
        self.region_splitter
            .ExecuteSortedKeysOnRegion(ctx, region, keys)
    }

    fn ExecuteSortedKeys(&self, ctx: &Context, keys: &[Vec<u8>]) -> Result<()> {
        self.region_splitter.ExecuteSortedKeys(ctx, keys)
    }

    fn WaitForScatterRegionsTimeout(
        &self,
        ctx: &Context,
        regionInfos: &[RegionInfo],
        timeout: Duration,
    ) -> i32 {
        self.region_splitter
            .WaitForScatterRegionsTimeout(ctx, regionInfos, timeout)
    }
}

// ExecuteRegions：SplitPoint 收集点 → 拆分 → 等待 scatter。
impl PipelineRegionsSplitter for PipelineRegionsSplitterImpl {
    fn ExecuteRegions(&self, ctx: &Context, splitHelper: &SplitHelperIterator) -> Result<()> {
        self.regions_buf.lock().unwrap().clear();
        // 每轮执行前清空待 scatter 缓冲。
        let this = self as *const PipelineRegionsSplitterImpl;
        // splitRegionByPoints needs &self
        // 以闭包回调拆分，避免与 regions_buf 借用冲突。
        SplitPoint(
            ctx,
            splitHelper,
            self.region_splitter.client.as_ref(),
            |ctx, init_len, init_num, region, valueds| {
                // SAFETY: self lives for the call
                // 回调期间 self 有效；用裸指针避开借用冲突。
                let s = unsafe { &*this };
                s.splitRegionByPoints(ctx, init_len, init_num, region, valueds)
            },
        )?;
        let scatter = self.regions_buf.lock().unwrap().clone();
        let _ = self.WaitForScatterRegionsTimeout(ctx, &scatter, Duration::from_secs(60));
        // scatter 等待失败不阻断主流程（与 Go 忽略错误一致）。
        Ok(())
    }
}

// splitF 回调签名：初始 Size/Number、当前 region、重叠 Valued 切片。
type SplitFunc<'a> = dyn FnMut(&Context, u64, i64, &RegionInfo, &[Valued]) -> Result<()> + 'a;

/// 按 region 与有值区间求交，调用 splitF 执行具体拆分。
/// SplitPoint selects ranges overlapped with each region and calls `splitF`.
/// 核心调度：扫描 region，将有值区间切到 region 边界并回调。
pub fn SplitPoint<F>(
    ctx: &Context,
    iter: &SplitHelperIterator,
    client: &dyn SplitClient,
    mut splitF: F,
) -> Result<()>
where
    F: FnMut(&Context, u64, i64, &RegionInfo, &[Valued]) -> Result<()>,
{
    // 本地 region 滑动窗口缓冲。
    let mut regions: Vec<RegionInfo> = Vec::new();
    // 当前消费到的 region 下标。
    let mut regionIndex: usize = 0;
    // 归属当前 region 的有值片段。
    let mut regionValueds: Vec<Valued> = Vec::new();
    // 上一批待回调的 region。
    let mut regionInfo: Option<RegionInfo> = None;
    // 跨 region 均摊后的初始 Size。
    let mut initialLength: u64 = 0;
    // 跨 region 均摊后的初始 Number。
    let mut initialNumber: i64 = 0;
    // Traverse 闭包内错误外提。
    let mut err: Option<SharedError> = None;

    iter.Traverse(|v, endKey, rule| {
        if v.Value.Number == 0 || v.Value.Size == 0 {
            // 零权重区间不产生 split 点。
            return true;
        }
        let (vStartKey, vEndKey) = match GetRewriteEncodedKeys(&v, Some(rule)) {
            Ok((Some(s), Some(e))) => (s, e),
            Ok(_) => {
                err = Some(New("rewrite keys missing"));
                // 重写键缺失视为致命，中断 Traverse。
                return false;
            }
            Err(e) => {
                err = Some(e);
                return false;
            }
        };

        while regionIndex < regions.len() {
            // 跳过终点已不覆盖 vStart 的旧 region。
            let end = regions[regionIndex]
                .Region
                .as_ref()
                .map(|r| r.EndKey.clone())
                .unwrap_or_default();
            if vStartKey.as_slice() < end.as_slice() {
                break;
            }
            regionIndex += 1;
        }
        if regionIndex == regions.len() {
            // 缓冲内无可用 region，清空后强制重新扫描。
            regions.clear();
        }
        let mut regionOverCount: u64 = 0;
        loop {
            if regionIndex >= regions.len() {
                let startKey = if !regions.is_empty() {
                    regions[regions.len() - 1]
                        .Region
                        .as_ref()
                        .map(|r| r.EndKey.clone())
                        .unwrap_or_default()
                } else {
                    vStartKey.clone()
                };
                match ScanRegionsWithRetry(ctx, client, &startKey, &endKey, 64) {
                    // 本地 region 缓冲耗尽时向 PD 再扫一批。
                    Ok(r) => {
                        regions = r;
                        regionIndex = 0;
                    }
                    Err(e) => {
                        err = Some(e);
                        return false;
                    }
                }
            }

            let region = regions[regionIndex].clone();
            regionOverCount += 1;
            // 统计同一 Valued 跨越的 region 数，用于均摊权重。
            let region_end = region
                .Region
                .as_ref()
                .map(|r| r.EndKey.clone())
                .unwrap_or_default();

            if vEndKey.as_slice() < region_end.as_slice() {
                // 当前 Valued 完全落在该 region 内：准备回调或累积。
                let endLength = v.Value.Size / regionOverCount;
                let endNumber = v.Value.Number / regionOverCount as i64;
                let same = match (
                    regionInfo.as_ref().and_then(|r| r.Region.as_ref()),
                    region.Region.as_ref(),
                ) {
                    (Some(a), Some(b)) => a.StartKey == b.StartKey && a.EndKey == b.EndKey,
                    _ => false,
                };
                if !regionValueds.is_empty() && !same {
                    // region 切换且已有累积：先对上一 region 回调。
                    let ri = regionInfo.as_ref().unwrap();
                    let ri_end = ri
                        .Region
                        .as_ref()
                        .map(|r| r.EndKey.clone())
                        .unwrap_or_default();
                    if vStartKey.as_slice() < ri_end.as_slice() {
                        regionValueds.push(NewValued(
                            vStartKey.clone(),
                            ri_end,
                            Value {
                                Size: endLength,
                                Number: endNumber,
                            },
                        ));
                    }
                    if let Err(e) = splitF(ctx, initialLength, initialNumber, ri, &regionValueds) {
                        err = Some(e);
                        return false;
                    }
                    regionValueds.clear();
                }
                if regionOverCount == 1 {
                    // 单 region 覆盖：整段 Valued 记入；跨 region 则记初值。
                    regionValueds.push(Valued {
                        Key: Span {
                            StartKey: vStartKey.clone(),
                            EndKey: vEndKey.clone(),
                        },
                        Value: v.Value,
                    });
                } else {
                    initialLength = endLength;
                    // 跨多 region 时把均摊值作为下一 region 初值。
                    initialNumber = endNumber;
                }
                regionInfo = Some(region);
                // 记录当前 region，供后续切换时回调。
                return true;
            }
            regionIndex += 1;
        }
    });

    // Traverse 中断后统一返回错误。
    if let Some(e) = err {
        return Err(e);
    }
    // 扫尾：最后一 region 上残留的 Valued 也要回调。
    if !regionValueds.is_empty() {
        let ri = regionInfo.as_ref().unwrap();
        splitF(ctx, initialLength, initialNumber, ri, &regionValueds)?;
    }
    Ok(())
}

// 内部按点拆分逻辑。
impl PipelineRegionsSplitterImpl {
    /// 按阈值从 Valued 序列挑选 split 点，失败时回退全量 ExecuteSortedKeys。
    pub(crate) fn splitRegionByPoints(
        &self,
        ctx: &Context,
        initialLength: u64,
        initialNumber: i64,
        region: &RegionInfo,
        valueds: &[Valued],
    ) -> Result<()> {
        // 本 region 内选出的 raw/encoded 拆分点。
        let mut splitPoints: Vec<Vec<u8>> = Vec::new();
        // 上一段起点，用于判断键是否前进。
        let mut lastKey = region
            .Region
            .as_ref()
            .map(|r| r.StartKey.clone())
            .unwrap_or_default();
        // 从跨 region 初值继续累加 Size。
        let mut length = initialLength;
        // 从跨 region 初值继续累加 Number。
        let mut number = initialNumber;
        for v in valueds {
            // 调试：正在为拆分累积一项。
            log::Debug("[split-point] accumulating a item for splitting.");
            let start = v.GetStartKey();
            if start.as_slice() != lastKey.as_slice()
            // 键前进且超过 Size/Keys 阈值时记录 split 点。
                && (v.Value.Size + length > self.splitThresholdSize
                    || v.Value.Number + number > self.splitThresholdKeys)
            {
                match codec::DecodeBytes(&start, None) {
                    // 优先解码为 raw key；失败则用 encoded 原值。
                    Ok((_, rawKey)) => {
                        splitPoints.push(rawKey);
                        log::Info("[split-point] added split key for region.");
                    }
                    // Go intentionally ignores DecodeBytes' error and forwards
                    // the nil rawKey returned on failure.
                    Err(_) => splitPoints.push(Vec::new()),
                }
                length = 0;
                // 命中阈值后重置累积，开启下一段。
                number = 0;
            }
            // 推进 lastKey，供下一轮阈值判断。
            lastKey = start;
            // 累加本段 Size/Number。
            length += v.Value.Size;
            number += v.Value.Number;
        }

        if splitPoints.is_empty() {
            // 无点可拆：该 region 已满足阈值或无前进键。
            return Ok(());
        }

        match self
            .region_splitter
            .ExecuteSortedKeysOnRegion(ctx, region, &splitPoints)
        {
            Ok(newRegions) => {
                if !ctx.Done() {
                    // 仅在未取消时把新区写入 scatter 缓冲。
                    self.regions_buf.lock().unwrap().extend(newRegions);
                }
                log::Info("split the region");
                Ok(())
            }
            Err(errSplit) => {
                // 单 region 失败则排序后走全局 ExecuteSortedKeys 回退。
                log::Warn(&format!("failed to split the scaned region: {errSplit}"));
                splitPoints.sort();
                self.region_splitter.ExecuteSortedKeys(ctx, &splitPoints)
            }
        }
    }
}

// 保留 Arc 引用以免未使用告警；无运行时语义。
// silence unused import warning for Arc if any
#[allow(dead_code)]
fn _unused() {
    let _: Option<Arc<()>> = None;
}
