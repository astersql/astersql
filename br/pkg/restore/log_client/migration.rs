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

//! Migration skip maps and WithMigrations builder, matching `migration.go`.
//!
//! 本模块对齐 Go `br/pkg/restore/log_client/migration.go`，负责日志备份恢复时的
//! **migration 跳过表**与 **WithMigrations 过滤器构建**：
//! - 三级跳过映射：meta → physical → logical（offset），对应 `MetaEdit` 的删除语义；
//! - `WithMigrationsBuilder` 按 `[shiftStartTS, restoredTS]` 粗过滤 migration，
//!   汇总 skipmap / compaction 目录 / ingested SST 路径；
//! - `retain-latest-mvcc` 场景下解析 compact-log-backup 注释并校验分片覆盖；
//! - `WithMigrations::{Metas,Physicals,Logicals,Compactions,IngestedSSTs}` 将跳过表
//!   应用到迭代器，过滤已删除或越界的日志文件。
//!
//! 数据流：`Migration[]` → `Build` → `WithMigrations` → 上层 `log_file_manager`
//! 迭代；粗过滤先丢弃时间窗外的 compaction，细过滤在迭代时按 skipmap 剔除。
//! Go 的 `NeedSkip` 存在 `if exists { return false }` 反转缺陷且基本未用；
//! Rust 按 `LogFilesSkipMapExt` 语义实现为「不存在则不跳过」。

use std::collections::{HashMap, HashSet};

use serde::Deserialize;

// 迭代组合子对齐 Go `br/pkg/utils/iter`，用于 Metas/Physicals/Compactions 管道。
use astersql_br_pkg_utils_iter::{
    ConcatAll, FilterOut, FlatMap, FromSlice, Indexed, Map, MapFilter, TryNextor,
};

use crate::log_file_manager::{
    FileIndex, FileIndexIter, GroupIndex, GroupIndexIter, MetaName, MetaNameIter, Subcompactions,
};
use crate::stubs::backuppb::{
    IngestedSSTs, LogFileCompaction, LogFileSubcompaction, MetaEdit, Migration,
};
use crate::stubs::berrors;
use crate::stubs::storeapi::Storage;
use crate::stubs::stream::{self, IngestedSSTsGroupExt};
use crate::stubs::{Context, Error, Result};

/// 逻辑文件（按 RangeOffset）跳过集合；键存在即表示该 offset 应跳过。
pub type LogicalSkipMap = HashMap<u64, ()>;

/// 单个物理文件的跳过状态：整文件跳过（`skip`）或按 offset 细粒度跳过。
#[derive(Clone, Debug, Default)]
pub struct LogicalFileSkipMap {
    /// offset → 占位；仅在 `skip=false` 时有效。
    pub skipmap: LogicalSkipMap,
    /// 为 true 时整物理文件跳过，忽略 `skipmap`。
    pub skip: bool,
}

/// 物理路径 → 逻辑跳过表；用于同一 meta 下多个物理文件。
pub type PhysicalSkipMap = HashMap<String, LogicalFileSkipMap>;

/// 单个 meta 的跳过状态：整 meta 跳过或按物理路径细分。
#[derive(Clone, Debug, Default)]
pub struct PhysicalFileSkipMap {
    /// 物理路径 → 逻辑跳过；仅在 `skip=false` 时有效。
    pub skipmap: PhysicalSkipMap,
    /// 为 true 时整份 meta 跳过（对应 `MetaEdit.DestructSelf`）。
    pub skip: bool,
}

/// meta 路径 → 物理跳过表；WithMigrations 的核心索引。
pub type MetaSkipMap = HashMap<String, PhysicalFileSkipMap>;

/// 标记整份 meta 删除：`skip=true` 且清空子映射，对齐 Go `skipMeta`。
pub fn skipMeta(skipmap: &mut MetaSkipMap, metaPath: &str) {
    skipmap.insert(
        metaPath.to_string(),
        PhysicalFileSkipMap {
            skipmap: HashMap::new(),
            skip: true,
        },
    );
}

/// 标记物理文件删除；若父 meta 已整份跳过则短路，避免无谓细化。
pub fn skipPhysical(skipmap: &mut MetaSkipMap, metaPath: &str, physicalPath: &str) {
    let metaMap = skipmap
        .entry(metaPath.to_string())
        .or_insert_with(|| PhysicalFileSkipMap {
            skipmap: HashMap::new(),
            skip: false,
        });
    // 父级已 DestructSelf，子级删除不再写入。
    if metaMap.skip {
        return;
    }
    metaMap.skipmap.insert(
        physicalPath.to_string(),
        LogicalFileSkipMap {
            skipmap: HashMap::new(),
            skip: true,
        },
    );
}

/// 标记逻辑 span（按 offset）删除；父级已跳过则短路。
pub fn skipLogical(skipmap: &mut MetaSkipMap, metaPath: &str, physicalPath: &str, offset: u64) {
    let metaMap = skipmap
        .entry(metaPath.to_string())
        .or_insert_with(|| PhysicalFileSkipMap {
            skipmap: HashMap::new(),
            skip: false,
        });
    if metaMap.skip {
        return;
    }
    let fileMap = metaMap
        .skipmap
        .entry(physicalPath.to_string())
        .or_insert_with(|| LogicalFileSkipMap {
            skipmap: HashMap::new(),
            skip: false,
        });
    // 物理文件已整文件跳过时，不再记录单个 offset。
    if fileMap.skip {
        return;
    }
    fileMap.skipmap.insert(offset, ());
}

/// Intended semantics match LogFilesSkipMapExt (`!exists` => not skip).
/// Go source currently has inverted `if exists` which panics / is unused.
///
/// 查询三级跳过表：meta / 物理 / offset 任一命中即跳过。
/// 路径不存在视为「未删除」→ 返回 false（与 Go 反转缺陷刻意不同）。
pub fn NeedSkip(skipmap: &MetaSkipMap, metaPath: &str, physicalPath: &str, offset: u64) -> bool {
    // meta 未登记：保持恢复，不跳过。
    let Some(metaMap) = skipmap.get(metaPath) else {
        return false;
    };
    if metaMap.skip {
        return true;
    }
    // 物理路径未登记：同样视为未删除。
    let Some(fileMap) = metaMap.skipmap.get(physicalPath) else {
        return false;
    };
    if fileMap.skip {
        return true;
    }
    // 最终落在 offset 集合；contains 即跳过该逻辑 span。
    fileMap.skipmap.contains_key(&offset)
}

/// 按恢复时间窗构建 `WithMigrations`；字段语义对齐 Go 同名结构。
pub struct WithMigrationsBuilder {
    /// 压缩产物过滤下界（通常为 shift 后的 startTS）。
    pub shiftStartTS: u64,
    /// 恢复区间起点，用于 ingested SST 与 retain-latest-mvcc 覆盖校验。
    pub startTS: u64,
    /// 恢复区间终点（含）。
    pub restoredTS: u64,
}

impl WithMigrationsBuilder {
    /// 默认 `shiftStartTS == startTS`；调用方可再 `SetShiftStartTS` 调整。
    pub fn new(startTS: u64, restoredTS: u64) -> Self {
        Self {
            shiftStartTS: startTS,
            startTS,
            restoredTS,
        }
    }

    /// 覆盖粗过滤下界；对齐 log restore 的 shift-start-ts 语义。
    pub fn SetShiftStartTS(&mut self, ts: u64) {
        self.shiftStartTS = ts;
    }

    /// 将一批 `MetaEdit` 合并进跳过表：DestructSelf / 删物理 / 删逻辑 span。
    pub fn updateSkipMap(&self, skipmap: &mut MetaSkipMap, metas: &[MetaEdit]) {
        for meta in metas {
            // DestructSelf 优先：整 meta 删除后无需再处理子删除列表。
            if meta.DestructSelf {
                skipMeta(skipmap, &meta.Path);
                continue;
            }
            // 物理文件删除列表 → skipPhysical。
            for path in &meta.DeletePhysicalFiles {
                skipPhysical(skipmap, &meta.Path, path);
            }
            // 逻辑 span 按 Offset 写入；Path 为物理文件路径。
            for filesInPhysical in &meta.DeleteLogicalFiles {
                for span in &filesInPhysical.Spans {
                    skipLogical(skipmap, &meta.Path, &filesInPhysical.Path, span.Offset);
                }
            }
        }
    }

    /// 粗过滤：若 migration 内所有有效 compaction 的输入 TS 均落在恢复窗外则丢弃整条。
    /// 无有效 InputMin/Max 的 compaction 不参与判定（避免误杀）。
    pub fn coarseGrainedFilter(&self, mig: &Migration) -> bool {
        for compaction in &mig.Compactions {
            let rangeValid = compaction.InputMinTs != 0 && compaction.InputMaxTs != 0;
            let outOfRange = compaction.InputMaxTs < self.shiftStartTS
                || compaction.InputMinTs > self.restoredTS;
            // 仅当区间有效且完全越界时过滤；部分重叠仍保留。
            if rangeValid && outOfRange {
                return true;
            }
        }
        false
    }

    /// 校验 retain-latest-mvcc：compact-log-backup compaction 须完整覆盖
    /// `[startTS, restoredTS]` 且分片齐全；否则返回带解释的 InvalidArgument。
    pub fn ValidateRetainLatestMVCCCompactionCoverage(&self, migs: &[Migration]) -> Result<()> {
        let mut intervals = Vec::with_capacity(8);
        for mig in migs {
            for compaction in &mig.Compactions {
                let (interval, ok) =
                    compactLogBackupCompactionIntervalForRetainLatestMVCC(compaction)?;
                if ok {
                    intervals.push(interval);
                }
            }
        }
        if retainLatestMVCCCompactionsCover(&intervals, self.startTS, self.restoredTS) {
            return Ok(());
        }
        Err(Error::Annotatef(
            berrors::ErrInvalidArgument("retain-latest-mvcc coverage incomplete"),
            format!(
                "retain-latest-mvcc-version requires compact-log-backup compactions with cal-shift-ts enabled, minimal-compaction-size=0, complete TS coverage over [{}, {}], and complete shards",
                self.startTS, self.restoredTS
            ),
        ))
    }

    /// 汇总跳过表、compaction 产物目录与 ingested SST 路径；已粗过滤的 migration 跳过。
    pub fn Build(&self, migs: &[Migration]) -> WithMigrations {
        let mut skipmap: MetaSkipMap = HashMap::new();
        let mut compactionDirs = Vec::with_capacity(8);
        let mut fullBackups = Vec::with_capacity(8);

        for mig in migs {
            // 时间窗外的 migration 整条跳过，不污染 skipmap / 目录列表。
            if self.coarseGrainedFilter(mig) {
                continue;
            }
            self.updateSkipMap(&mut skipmap, &mig.EditMeta);
            for c in &mig.Compactions {
                // Artifacts 目录供后续 Subcompactions 遍历。
                compactionDirs.push(c.Artifacts.clone());
            }
            fullBackups.extend(mig.IngestedSstPaths.iter().cloned());
        }
        WithMigrations {
            skipmap,
            compactionDirs,
            fullBackups,
            restoredTS: self.restoredTS,
            startTS: self.startTS,
            shiftStartTS: self.shiftStartTS,
        }
    }
}

/// compact-log-backup 注释中的分片信息：`index` 从 1 起，须 ≤ `total`。
#[derive(Debug, Deserialize)]
struct CompactLogBackupCommentShard {
    index: u64,
    total: u64,
}

/// 注释内 config 段；字段名与 Go JSON tag 对齐（kebab-case）。
#[derive(Debug, Deserialize)]
struct CompactLogBackupCommentConfig {
    #[serde(rename = "from-ts")]
    from_ts: Option<u64>,
    #[serde(rename = "until-ts")]
    until_ts: Option<u64>,
    #[serde(rename = "cal-shift-ts")]
    cal_shift_ts: Option<bool>,
    #[serde(rename = "minimal-compaction-size")]
    minimal_compaction_size: Option<u64>,
    shard: Option<CompactLogBackupCommentShard>,
}

/// compact-log-backup 写入 compaction.Comments 的 JSON 外壳。
#[derive(Debug, Deserialize)]
struct CompactLogBackupComment {
    config: Option<CompactLogBackupCommentConfig>,
}

/// retain-latest-mvcc 覆盖判定用的时间区间 + 分片坐标。
#[derive(Clone, Debug)]
pub struct RetainLatestMVCCCompactionInterval {
    /// 覆盖起点（含），优先取注释 from-ts。
    pub from: u64,
    /// 覆盖终点（含），优先取注释 until-ts。
    pub until: u64,
    /// 分片序号，缺省视为 1/1。
    pub shardIndex: u64,
    /// 总分片数；与 `shardIndex` 一起判定是否凑齐。
    pub shardTotal: u64,
}

/// 从 compaction 注释解析 retain-latest-mvcc 可用区间。
/// 返回 `(interval, ok)`：`ok=false` 表示该 compaction 不参与覆盖（缺注释/
/// 未开 cal-shift-ts / minimal-compaction-size≠0）；`ok=true` 时 interval 有效。
pub fn compactLogBackupCompactionIntervalForRetainLatestMVCC(
    compaction: &LogFileCompaction,
) -> Result<(RetainLatestMVCCCompactionInterval, bool)> {
    let comments = compaction.GetComments();
    // 空注释：非 compact-log-backup 产物，不参与覆盖统计。
    if comments.is_empty() {
        return Ok((
            RetainLatestMVCCCompactionInterval {
                from: 0,
                until: 0,
                shardIndex: 0,
                shardTotal: 0,
            },
            false,
        ));
    }
    // JSON 解析失败视为参数错误，阻断 retain-latest-mvcc 校验。
    let comment: CompactLogBackupComment = serde_json::from_str(comments).map_err(|err| {
        Error::Annotatef(
            berrors::ErrInvalidArgument(err.to_string()),
            "failed to parse compact-log-backup compaction comments",
        )
    })?;
    // 缺少 config 段：同样不参与覆盖。
    let Some(config) = comment.config else {
        return Ok((
            RetainLatestMVCCCompactionInterval {
                from: 0,
                until: 0,
                shardIndex: 0,
                shardTotal: 0,
            },
            false,
        ));
    };
    // retain-latest-mvcc 要求显式启用 cal-shift-ts。
    if config.cal_shift_ts != Some(true) {
        return Ok((
            RetainLatestMVCCCompactionInterval {
                from: 0,
                until: 0,
                shardIndex: 0,
                shardTotal: 0,
            },
            false,
        ));
    }
    // 必须 minimal-compaction-size=0，保证无「过小丢弃」造成的空洞。
    if config.minimal_compaction_size != Some(0) {
        return Ok((
            RetainLatestMVCCCompactionInterval {
                from: 0,
                until: 0,
                shardIndex: 0,
                shardTotal: 0,
            },
            false,
        ));
    }
    // 注释缺省时回落到 compaction protobuf 字段，保持与 Go 一致。
    let fromTS = config
        .from_ts
        .unwrap_or_else(|| compaction.GetCompactionFromTs());
    let untilTS = config
        .until_ts
        .unwrap_or_else(|| compaction.GetCompactionUntilTs());
    // from > until 为非法配置，直接报错而非静默忽略。
    if fromTS > untilTS {
        return Err(Error::Annotatef(
            berrors::ErrInvalidArgument("invalid TS range"),
            format!(
                "compact-log-backup compaction comments have invalid TS range [{fromTS}, {untilTS}]"
            ),
        ));
    }
    let Some(shard) = config.shard else {
        // 无分片字段时视为单分片 1/1，直接参与覆盖。
        return Ok((
            RetainLatestMVCCCompactionInterval {
                from: fromTS,
                until: untilTS,
                shardIndex: 1,
                shardTotal: 1,
            },
            true,
        ));
    };
    if shard.index == 0 || shard.total == 0 || shard.index > shard.total {
        return Err(Error::Annotatef(
            berrors::ErrInvalidArgument("invalid shard"),
            format!(
                "compact-log-backup compaction comments have invalid shard {}/{}",
                shard.index, shard.total
            ),
        ));
    }
    Ok((
        RetainLatestMVCCCompactionInterval {
            from: fromTS,
            until: untilTS,
            shardIndex: shard.index,
            shardTotal: shard.total,
        },
        true,
    ))
}

/// 判断在 `[from, until]` 上是否存在某一 `shardTotal`，其全部分片均已出现。
/// 只统计完全覆盖该子区间的 interval（from≤from 且 until≥until）。
pub fn hasCompleteShardCoverage(
    intervals: &[RetainLatestMVCCCompactionInterval],
    from: u64,
    until: u64,
) -> bool {
    let mut shardsByTotal: HashMap<u64, HashSet<u64>> = HashMap::new();
    for interval in intervals {
        if interval.from <= from && interval.until >= until {
            shardsByTotal
                .entry(interval.shardTotal)
                .or_default()
                .insert(interval.shardIndex);
        }
    }
    for (total, shards) in shardsByTotal {
        // 任一 total 的分片集合凑齐即可。
        if shards.len() as u64 == total {
            return true;
        }
    }
    false
}

/// 将 `[startTS, restoredTS]` 按 interval 边界切成子段，逐段检查分片覆盖。
/// `startTS >= restoredTS` 视为空区间，直接通过（对齐 Go）。
pub fn retainLatestMVCCCompactionsCover(
    intervals: &[RetainLatestMVCCCompactionInterval],
    startTS: u64,
    restoredTS: u64,
) -> bool {
    if startTS >= restoredTS {
        return true;
    }
    let mut boundaries = vec![startTS, restoredTS];
    for interval in intervals {
        // 与恢复窗无交集的 interval 不贡献切点。
        if interval.until < startTS || interval.from > restoredTS {
            continue;
        }
        let from = interval.from.max(startTS);
        let until = interval.until.min(restoredTS);
        boundaries.push(from);
        boundaries.push(until);
    }
    // 排序去重后形成连续子区间端点序列。
    boundaries.sort_unstable();
    boundaries.dedup();
    // 仅剩单点时仍按整窗检查一次（防御性，正常不应出现）。
    if boundaries.len() == 1 {
        return hasCompleteShardCoverage(intervals, startTS, restoredTS);
    }
    for i in 0..boundaries.len() - 1 {
        if boundaries[i] == boundaries[i + 1] {
            continue;
        }
        // 任一子段缺少完整分片覆盖则整体失败。
        if !hasCompleteShardCoverage(intervals, boundaries[i], boundaries[i + 1]) {
            return false;
        }
    }
    true
}

/// 物理层包装：携带可选逻辑跳过表，供 `Logicals` 过滤 FileIndex。
pub struct PhysicalWithMigrations {
    /// `None` 表示无 offset 级跳过（可能因整物理已在上层滤掉）。
    pub skipmap: Option<LogicalSkipMap>,
    /// 原始物理分组索引。
    pub physical: GroupIndex,
}

impl PhysicalWithMigrations {
    /// 过滤逻辑文件：RangeOffset 落在 skipmap 中则剔除。
    pub fn Logicals(&self, fileIndexIter: FileIndexIter) -> FileIndexIter {
        let skip = self.skipmap.clone();
        FilterOut(fileIndexIter, move |fileIndex: &FileIndex| {
            if let Some(ref sm) = skip {
                if sm.contains_key(&fileIndex.Item.RangeOffset) {
                    return true;
                }
            }
            false
        })
    }
}

/// meta 层包装：携带可选物理跳过表，供 `Physicals` 过滤 GroupIndex。
pub struct MetaWithMigrations {
    /// `None` 表示无物理级细过滤（整 meta 跳过时也置 None 并 FilterOut）。
    pub skipmap: Option<PhysicalSkipMap>,
    pub meta: crate::stubs::backuppb::Metadata,
    /// meta 路径名，用于与 skipmap 键对齐。
    pub name: String,
}

impl MetaWithMigrations {
    /// 将 GroupIndex 迭代包装为 PhysicalWithMigrations；整物理 skip 则 FilterOut。
    pub fn Physicals(
        &self,
        groupIndexIter: GroupIndexIter,
    ) -> Box<dyn TryNextor<PhysicalWithMigrations>> {
        let skipmap = self.skipmap.clone();
        MapFilter(groupIndexIter, move |groupIndex: GroupIndex| {
            let mut logiSkipmap = None;
            if let Some(ref sm) = skipmap {
                if let Some(entry) = sm.get(&groupIndex.Item.Path) {
                    if entry.skip {
                        // 整物理跳过：返回占位并标记 filter-out。
                        return (
                            PhysicalWithMigrations {
                                skipmap: None,
                                physical: groupIndex,
                            },
                            true,
                        );
                    }
                    logiSkipmap = Some(entry.skipmap.clone());
                }
            }
            (
                PhysicalWithMigrations {
                    skipmap: logiSkipmap,
                    physical: groupIndex,
                },
                false,
            )
        })
    }
}

/// Build 产物：跳过表 + compaction/ingested 路径 + 恢复时间窗。
pub struct WithMigrations {
    /// meta→物理→逻辑三级跳过表。
    pub skipmap: MetaSkipMap,
    /// compaction Artifacts 目录列表。
    pub compactionDirs: Vec<String>,
    /// ingested SST 全量备份路径。
    pub fullBackups: Vec<String>,
    pub shiftStartTS: u64,
    pub startTS: u64,
    pub restoredTS: u64,
}

impl WithMigrations {
    /// meta 级过滤：整 meta skip 则 FilterOut，否则下传物理 skipmap。
    pub fn Metas(&self, metaNameIter: MetaNameIter) -> Box<dyn TryNextor<MetaWithMigrations>> {
        let skipmap = self.skipmap.clone();
        MapFilter(metaNameIter, move |mname: MetaName| {
            let mut phySkipmap = None;
            if let Some(entry) = skipmap.get(&mname.name) {
                if entry.skip {
                    return (
                        MetaWithMigrations {
                            skipmap: None,
                            meta: mname.meta,
                            name: mname.name,
                        },
                        true,
                    );
                }
                phySkipmap = Some(entry.skipmap.clone());
            }
            (
                MetaWithMigrations {
                    skipmap: phySkipmap,
                    meta: mname.meta,
                    name: mname.name,
                },
                false,
            )
        })
    }

    /// 遍历 compactionDirs，按 `[shiftStartTS, restoredTS]` 加载 Subcompaction。
    /// 当前桩路径下 Storage walk 为空；测试可直接注入 `Subcompactions`。
    pub fn Compactions(
        &self,
        ctx: &Context,
        s: &dyn Storage,
    ) -> Box<dyn TryNextor<LogFileSubcompaction>> {
        let dirs = self.compactionDirs.clone();
        let shift = self.shiftStartTS;
        let restored = self.restoredTS;
        let iters = dirs
            .into_iter()
            .map(|name| Subcompactions(ctx, &name, s, shift, restored))
            .collect();
        ConcatAll(iters)
    }

    /// 加载 ingested SST：过滤未完成分组及 GroupTS 落在 `[startTS, restoredTS]` 外的项。
    pub fn IngestedSSTs(&self, ctx: &Context, s: &dyn Storage) -> Box<dyn TryNextor<IngestedSSTs>> {
        let start = self.startTS;
        let restored = self.restoredTS;
        let filtered = FilterOut(
            stream::LoadIngestedSSTs(ctx, s, &self.fullBackups),
            move |ebk| {
                let gts = ebk.GroupTS();
                // 未完成或时间窗外的分组整组丢弃。
                !ebk.GroupFinished() || gts < start || gts > restored
            },
        );
        FlatMap(filtered, |ebk| {
            Map(FromSlice(ebk), |p: stream::PathedIngestedSSTs| {
                p.IngestedSSTs
            })
        })
    }
}
