// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc. Licensed under Apache-2.0.

//! Compacted SST split strategy, matching `compacted_file_strategy.go`.
//!
//! 本文件实现日志还原中“已压缩 SST”的切分策略，与 Go 同名文件语义对齐。
//! 职责是把一批 RewrittenSSTs 累计到按表划分的 SplitHelper，并决定何时切分、何时跳过。
//! 与普通日志文件切分不同：压缩产物的 KV 规模往往更大，因此用 impactFactor 稀释计数。
//! 检查点集合用于跳过已完成文件，并回调进度，避免重复导入。
//! 本策略只服务压缩产物路径，普通日志文件走其它 SplitStrategy。
//! ShouldSkip 与 Accumulate 解耦：先过滤检查点，再累计规模。
//! hasRule 用改写目标表 ID 查规则，避免改写后误判“无规则”。

use std::collections::{HashMap, HashSet};

use astersql_br_pkg_restore_split::splitter::{BaseSplitStrategy, NewBaseSplitStrategy};
use astersql_br_pkg_restore_split::sum_sorted::{NewSplitHelper, Span, SplitHelper, Value, Valued};
use astersql_br_pkg_restore_utils::{GetRewriteRawKeys, GetRewriteRuleOfTable, RewriteRules};

use crate::ssts::{RewrittenSSTs, SSTs};
use crate::stubs::log;

// 压缩 SST 对 split 阈值的稀释系数，与 Go impactFactor 一致。
// 累计 KV/尺寸时除以该值，避免单批过大导致过早切分。
const impactFactor: i64 = 16;

/// 压缩文件切分策略：在 BaseSplitStrategy 上叠加检查点跳过与 impactFactor 稀释。
pub struct CompactedFileSplitStrategy {
    pub base: BaseSplitStrategy,
    // 已由检查点确认完成的 SST 文件名集合。
    pub checkpointSets: HashSet<String>,
    // 跳过已完成文件时回写 KV/尺寸进度，保持检查点统计连续。
    pub checkpointFileProgressFn: Box<dyn FnMut(u64, u64) + Send>,
}

/// 构造策略：注入改写规则、检查点集合与进度回调。
pub fn NewCompactedFileSplitStrategy(
    rules: HashMap<i64, RewriteRules>,
    checkpointsSet: HashSet<String>,
    updateStatsFn: Box<dyn FnMut(u64, u64) + Send>,
) -> CompactedFileSplitStrategy {
    CompactedFileSplitStrategy {
        base: NewBaseSplitStrategy(rules),
        checkpointSets: checkpointsSet,
        checkpointFileProgressFn: updateStatsFn,
    }
}

// 解析 SST 在改写后的有效表 ID，以及用于键改写的边界规则。
struct SstIdentity {
    EffectiveID: i64,
    RewriteBoundary: Option<RewriteRules>,
}

impl CompactedFileSplitStrategy {
    // 若 SST 被改写到另一张表，EffectiveID 取目标表，并构造单表改写边界。
    // 否则沿用原 TableID，不附加 RewriteBoundary。
    fn inspect(&self, ssts: &dyn SSTs) -> SstIdentity {
        if let Some(r) = ssts.as_rewritten() {
            if r.RewrittenTo() != ssts.TableID() {
                let rule =
                    GetRewriteRuleOfTable(ssts.TableID(), r.RewrittenTo(), HashMap::new(), false);
                return SstIdentity {
                    EffectiveID: r.RewrittenTo(),
                    RewriteBoundary: Some(rule),
                };
            }
        }
        SstIdentity {
            EffectiveID: ssts.TableID(),
            RewriteBoundary: None,
        }
    }

    /// 将一组 SST 的键范围与稀释后的规模合并进对应表的 SplitHelper。
    pub fn Accumulate(&mut self, ssts: &dyn SSTs) {
        let identity = self.inspect(ssts);
        // 首次见到该有效表 ID 时懒创建 splitter，与 Go 侧行为一致。
        if !self.base.TableSplitter.contains_key(&identity.EffectiveID) {
            log::Info("Initialized splitter for table.");
            self.base
                .TableSplitter
                .insert(identity.EffectiveID, NewSplitHelper());
        }
        let splitHelper = self
            .base
            .TableSplitter
            .get_mut(&identity.EffectiveID)
            .expect("just inserted");

        for f in ssts.GetSSTs() {
            // 键范围必须能按改写规则解析；失败视为不可达，直接 Panic。
            let (startKey, endKey) = match GetRewriteRawKeys(&f, identity.RewriteBoundary.as_ref())
            {
                Ok((s, e)) => (s.unwrap_or_default(), e.unwrap_or_default()),
                Err(err) => {
                    log::Panic(&format!(
                        "[unreachable] the rewrite rule doesn't match the SST file: {err}"
                    ));
                }
            };
            self.base.AccumulateCount += 1;
            // 空文件不参与 merge，但仍已计入 AccumulateCount。
            if f.TotalKvs == 0 || f.Size_ == 0 {
                log::Warn("No key-value pairs in sst files");
                continue;
            }
            // 稀释后至少为 1，保证极小 subcompaction 仍占位。
            let mut calculateCount = (f.TotalKvs as i64) / impactFactor;
            if calculateCount == 0 {
                log::Warn("less than impactFactor key-value pairs in subcompaction");
                calculateCount = 1;
            }
            let mut calculateSize = f.Size_ / (impactFactor as u64);
            if calculateSize == 0 {
                log::Warn("less than impactFactor key-value size in subcompaction");
                calculateSize = 1;
            }
            splitHelper.Merge(Valued {
                Key: Span {
                    StartKey: startKey,
                    EndKey: endKey,
                },
                Value: Value {
                    Size: calculateSize,
                    Number: calculateCount,
                },
            });
        }
    }

    /// 累计文件数超过 4096/impactFactor 时触发切分，与 Go 阈值一致。
    /// 阈值按稀释后计数理解：约等于 256 个“等效文件”后切分。
    pub fn ShouldSplit(&self) -> bool {
        self.base.AccumulateCount > (4096 / impactFactor) as i32
    }

    /// 无改写规则则整组跳过；命中检查点的文件剔除并回报进度。
    /// 全部跳过返回 true；部分跳过则原地改写 SST 列表后返回 false。
    pub fn ShouldSkip(&mut self, ssts: &mut dyn SSTs) -> bool {
        if !hasRule(ssts, &self.base.Rules) {
            log::Warn("skip for no rule files");
            return true;
        }
        let mut sstOutputs = Vec::with_capacity(ssts.GetSSTs().len());
        let origin_len = ssts.GetSSTs().len();
        for sst in ssts.GetSSTs() {
            if !self.checkpointSets.contains(&sst.Name) {
                sstOutputs.push(sst);
            } else {
                // 已完成文件仍推进统计，避免检查点进度倒退。
                (self.checkpointFileProgressFn)(sst.TotalKvs, sst.Size_);
            }
        }
        if sstOutputs.is_empty() {
            log::Info("all files in SST set skipped");
            return true;
        }
        if sstOutputs.len() != origin_len {
            log::Info("partial files in SST set skipped due to checkpoint");
            ssts.SetSSTs(sstOutputs);
            return false;
        }
        false
    }

    /// 暴露按有效表 ID 索引的 SplitHelper，供上层生成 split 键。
    pub fn TableSplitter(&self) -> &HashMap<i64, SplitHelper> {
        &self.base.TableSplitter
    }
}

// 改写 SST 查目标表规则，否则查原表；无规则则 ShouldSkip 会整组丢弃。
fn hasRule<T>(ssts: &dyn SSTs, rules: &HashMap<i64, T>) -> bool {
    if let Some(r) = ssts.as_rewritten() {
        return rules.contains_key(&r.RewrittenTo());
    }
    rules.contains_key(&ssts.TableID())
}
