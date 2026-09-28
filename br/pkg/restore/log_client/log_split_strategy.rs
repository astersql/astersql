// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc. Licensed under Apache-2.0.

//! Log file split strategy, matching `log_split_strategy.go`.
//! 日志恢复的 Region 分裂策略：按大文件累积 key 范围，并结合检查点 SkipMap 跳过已完成文件。
//! 小文件（≤阈值）不参与累积，避免碎片化分裂；阈值默认 1MB，与 Go 常量一致。
//! 检查点加载只收录下游表 ID 命中的偏移，防止跨表误跳过。
//! ShouldSplit 阈值 4096 与 Go 硬编码一致，修改需双边同步。

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use astersql_br_pkg_restore_split::splitter::{BaseSplitStrategy, NewBaseSplitStrategy};
use astersql_br_pkg_restore_split::sum_sorted::{NewSplitHelper, Span, Value, Valued};
use astersql_br_pkg_restore_utils::RewriteRules;

use crate::log_file_manager::LogDataFileInfo;
use crate::log_file_map::{LogFilesSkipMap, NewLogFilesSkipMap};
use crate::stubs::checkpoint::{LogMetaManagerT, LogRestoreKeyType, LogRestoreValueMarshaled};
use crate::stubs::metrics;
use crate::stubs::summary;
use crate::stubs::{Context, Error, Result, log};

/// Minimum file size considered for per-batch split accumulation.
/// 低于此长度的文件不进入 Accumulate，减少无意义的 SplitHelper 合并。
pub const SplitFileThresholdDefault: u64 = 1024 * 1024; // 1 MB

/// 封装 BaseSplitStrategy + 检查点跳过表 + 进度回调。
pub struct LogSplitStrategy {
    pub base: BaseSplitStrategy,
    pub checkpointSkipMap: LogFilesSkipMap,
    // 跳过已完成文件时仍上报条目数/字节，保证进度条连续。
    pub checkpointFileProgressFn: Box<dyn FnMut(u64, u64) + Send>,
    pub splitFileThreshold: u64,
    // 内存指标节流：距上次更新不足 30s 则跳过 Traverse。
    lastMemUsageUpdate: Instant,
}

/// 构造策略：可选加载检查点，仅把下游表 ID 命中的偏移写入 SkipMap。
pub fn NewLogSplitStrategy(
    ctx: &Context,
    useCheckpoint: bool,
    logCheckpointMetaManager: Option<&LogMetaManagerT>,
    rules: HashMap<i64, RewriteRules>,
    updateStatsFn: Box<dyn FnMut(u64, u64) + Send>,
    splitFileThreshold: u64,
) -> Result<LogSplitStrategy> {
    // 下游（rewrite 后）表 ID 集合，用于过滤无关检查点条目。
    let mut downstreamIdset: HashSet<i64> = HashSet::new();
    for rule in rules.values() {
        downstreamIdset.insert(rule.NewTableID);
    }
    let mut skipMap = NewLogFilesSkipMap();
    if useCheckpoint {
        // 启用检查点时 manager 必填，否则与 Go 一样视为配置错误。
        let manager = logCheckpointMetaManager
            .ok_or_else(|| Error::new("checkpoint enabled but manager is None"))?;
        let t = manager.LoadCheckpointData(ctx, &mut |groupKey: LogRestoreKeyType,
                                                       off: LogRestoreValueMarshaled|
         -> Result<()> {
            // Foffs: tableID → 文件偏移列表；Goff 为 group 偏移。
            for (tableID, foffs) in off.Foffs {
                if downstreamIdset.contains(&tableID) {
                    for foff in foffs {
                        skipMap.Insert(&groupKey, off.Goff, foff);
                    }
                }
            }
            Ok(())
        })?;
        // 将汇总开始时间回调到更早检查点，避免 ETA 被重启拉长。
        summary::AdjustStartTimeToEarlierTime(t);
    }
    Ok(LogSplitStrategy {
        base: NewBaseSplitStrategy(rules),
        checkpointSkipMap: skipMap,
        checkpointFileProgressFn: updateStatsFn,
        splitFileThreshold,
        // 初值回拨 60s，使首次 Accumulate 立即刷新一次内存指标。
        lastMemUsageUpdate: Instant::now() - Duration::from_secs(60),
    })
}

impl LogSplitStrategy {
    /// 将大文件的 key 范围合并进对应表的 SplitHelper。
    pub fn Accumulate(&mut self, file: &LogDataFileInfo) {
        // 小文件直接忽略，避免为噪声范围建树。
        if file.Length <= self.splitFileThreshold {
            return;
        }
        self.base.AccumulateCount += 1;
        if !self.base.TableSplitter.contains_key(&file.TableId) {
            self.base
                .TableSplitter
                .insert(file.TableId, NewSplitHelper());
        }
        let splitHelper = self
            .base
            .TableSplitter
            .get_mut(&file.TableId)
            .expect("just inserted");
        // Size/Number 参与后续分裂点权衡，与 Go Valued 合并语义一致。
        splitHelper.Merge(Valued {
            Key: Span {
                StartKey: file.StartKey.clone(),
                EndKey: file.EndKey.clone(),
            },
            Value: Value {
                Size: file.Length,
                Number: file.NumberOfEntries,
            },
        });
        self.maybeUpdateMemUsage();
    }

    /// 累积文件数超过 4096 时建议触发一次分裂，与 Go 阈值对齐。
    /// 返回 true 仅表示“建议分裂”，真正执行由上层调度。
    pub fn ShouldSplit(&self) -> bool {
        self.base.AccumulateCount > 4096
    }

    /// 元数据文件、无 rewrite 规则、或检查点已完成 → 跳过。
    pub fn ShouldSkip(&mut self, file: &LogDataFileInfo) -> bool {
        // DDL/meta 文件不走 DML 分裂路径。
        if file.IsMeta {
            return true;
        }
        // 无规则意味着该表不在本次恢复范围。
        if !self.base.Rules.contains_key(&file.TableId) {
            log::Info("skip for no rule files");
            return true;
        }
        if self.checkpointSkipMap.NeedSkip(
            &file.MetaDataGroupName,
            file.OffsetInMetaGroup,
            file.OffsetInMergedGroup,
        ) {
            // 跳过仍计入进度，避免 UI/统计把已完成工作算作停滞。
            (self.checkpointFileProgressFn)(file.NumberOfEntries as u64, file.Length);
            return true;
        }
        false
    }

    // 周期性汇总 SplitHelper 内存占用，写入 KV_SPLIT_HELPER_MEM_USAGE 指标。
    fn maybeUpdateMemUsage(&mut self) {
        if self.lastMemUsageUpdate.elapsed() < Duration::from_secs(30) {
            return;
        }
        self.lastMemUsageUpdate = Instant::now();
        let mut memUsed = 0usize;
        // Traverse 回调返回 true 表示继续遍历全部节点。
        for hlp in self.base.TableSplitter.values() {
            hlp.Traverse(|v| {
                memUsed += v.MemSize();
                true
            });
        }
        metrics::KV_SPLIT_HELPER_MEM_USAGE.Set(memUsed as f64);
    }
}
