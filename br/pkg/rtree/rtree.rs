// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc. Licensed under Apache-2.0.

//! Range tree algorithms matching `br/pkg/rtree/rtree.go`.
//!
//! 本模块对齐 Go `br/pkg/rtree/rtree.go`，实现备份/恢复过程中的**区间树**核心算法：
//! - `KeyRange` / `Range`：半开区间 `[StartKey, EndKey)` 与备份文件集合；
//! - `RangeStatsTree`：按体积/键数阈值合并相邻可合并区间（`NeedsMerge`）；
//! - `RangeTree`：维护互不重叠的已完成备份区间，计算未覆盖空洞（`GetIncompleteRange`）；
//! - `ProgressRangeTree`：按原始请求区间跟踪进度，完成后写 meta 并累加 checksum。
//!
//! 数据流：备份响应 → `RangeTree::Put`/`Update` → `GetIncompleteRange` 驱动重试；
//! 恢复合并 → `RangeStatsTree::MergedRanges`；进度树 → `GetIncompleteRanges` 清理完成项。
//! Rust 用 `BTreeMap<StartKey, _>` 模拟 Go `google/btree`；空 `EndKey` 表示正无穷上界。

use std::collections::BTreeMap;
use std::ops::{Deref, DerefMut};

use crate::stubs::{self, AppendDataFile, ChecksumStats, File, FreeListG, MetaWriter, RpcKeyRange};

/// KeyRange represents an origin key range.
///
/// 原始键区间，语义为半开区间 `[StartKey, EndKey)`；`EndKey` 为空表示无上界（正无穷）。
/// 与 Go `KeyRange` 字段一一对应，是区间树与进度树的公共区间表示。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct KeyRange {
    /// 区间下界（含）；用作 BTree 排序键。
    pub StartKey: Vec<u8>,
    /// 区间上界（不含）；空切片表示最大端点。
    pub EndKey: Vec<u8>,
}

impl KeyRange {
    /// Contains check if the range contains the given key, [start, end).
    ///
    /// 判断单键是否落在本区间；`end` 为空时只校验 `key >= start`。
    pub fn Contains(&self, key: &[u8]) -> bool {
        let start = &self.StartKey;
        let end = &self.EndKey;
        key >= start.as_slice() && (end.is_empty() || key < end.as_slice())
    }

    /// ContainsRange check if the range contains the region's key range.
    ///
    /// 判断目标区间是否被本区间完全包含；上界比较用 `<=`，对齐 Go `ContainsRange`。
    pub fn ContainsRange(&self, startKey: &[u8], endKey: &[u8]) -> bool {
        let start = &self.StartKey;
        let end = &self.EndKey;
        startKey >= start.as_slice() && (end.is_empty() || endKey <= end.as_slice())
    }

    /// Intersect returns intersect range in the tree.
    ///
    /// 计算与 `[start, end)` 的交集；无交集时第三元为 `false` 并返回空键。
    /// 空 `end`/`EndKey` 均按“最大端点”处理，与 Go 注释 `empty mean the max end key` 一致。
    pub fn Intersect(&self, start: &[u8], end: &[u8]) -> (Vec<u8>, Vec<u8>, bool) {
        // empty mean the max end key
        // 本区间已在查询起点左侧：无交集
        if !self.EndKey.is_empty() && start >= self.EndKey.as_slice() {
            return (Vec::new(), Vec::new(), false);
        }
        // 查询区间已在本区间左侧：无交集
        if !end.is_empty() && end <= self.StartKey.as_slice() {
            return (Vec::new(), Vec::new(), false);
        }
        // 交集下界取较大者
        let subStart = if start >= self.StartKey.as_slice() {
            start.to_vec()
        } else {
            self.StartKey.clone()
        };
        // 交集上界：空端点继承对侧；否则取较小者
        let subEnd = if end.is_empty() {
            self.EndKey.clone()
        } else if self.EndKey.is_empty() || end < self.EndKey.as_slice() {
            end.to_vec()
        } else {
            self.EndKey.clone()
        };
        (subStart, subEnd, true)
    }
}

/// Range represents a backup response.
///
/// 一次备份响应区间：内嵌键范围 + 产出的 SST/`File` 列表，对齐 Go 嵌入式 `KeyRange`。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Range {
    /// 本响应覆盖的键区间。
    pub KeyRange: KeyRange,
    /// 该区间对应的备份文件元数据集合。
    pub Files: Vec<File>,
}

impl Deref for Range {
    type Target = KeyRange;
    fn deref(&self) -> &Self::Target {
        &self.KeyRange
    }
}

impl DerefMut for Range {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.KeyRange
    }
}

impl Range {
    /// BytesAndKeys returns total bytes and keys in a range.
    ///
    /// 汇总 `Files` 的 `TotalBytes`/`TotalKvs`，供合并阈值与进度统计使用。
    pub fn BytesAndKeys(&self) -> (u64, u64) {
        let mut bytes: u64 = 0;
        let mut keys: u64 = 0;
        for f in &self.Files {
            bytes = bytes.wrapping_add(f.TotalBytes);
            keys = keys.wrapping_add(f.TotalKvs);
        }
        (bytes, keys)
    }

    /// Less impls btree.Item.
    ///
    /// 按 `StartKey` 字典序比较，模拟 Go `btree.Item.Less`。
    pub fn Less(&self, ta: &Range) -> bool {
        self.KeyRange.StartKey < ta.KeyRange.StartKey
    }
}

/// RangeStats represents a restore merge result.
///
/// 恢复侧合并单元：在 `Range` 上附加体积 `Size` 与键数 `Count`，驱动 `NeedsMerge`。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RangeStats {
    /// 被合并的区间与文件集合。
    pub Range: Range,
    /// 区间数据体积（字节），用于拆分阈值。
    pub Size: u64,
    /// 区间键数量，用于拆分阈值。
    pub Count: u64,
}

impl Deref for RangeStats {
    type Target = Range;
    fn deref(&self) -> &Self::Target {
        &self.Range
    }
}

impl RangeStats {
    /// 按 StartKey 排序，对齐 Go `btree.Item` 比较语义。
    pub fn Less(&self, ta: &RangeStats) -> bool {
        self.Range.KeyRange.StartKey < ta.Range.KeyRange.StartKey
    }

    /// 委托到内嵌 `Range::BytesAndKeys`，保持与 Go 方法同名入口。
    pub fn BytesAndKeys(&self) -> (u64, u64) {
        self.Range.BytesAndKeys()
    }
}

/// 带统计信息的区间树：键为 StartKey，值为 `RangeStats`。
/// Go 使用 `btree.BTreeG`；此处以 `BTreeMap` 保持有序与替换语义。
pub struct RangeStatsTree {
    /// StartKey → 合并统计区间。
    pub BTreeG: BTreeMap<Vec<u8>, RangeStats>,
}

/// 构造空的 `RangeStatsTree`，对齐 Go `NewRangeStatsTree`。
pub fn NewRangeStatsTree() -> RangeStatsTree {
    RangeStatsTree {
        BTreeG: BTreeMap::new(),
    }
}

impl RangeStatsTree {
    /// 当前树中区间个数。
    pub fn Len(&self) -> usize {
        self.BTreeG.len()
    }

    /// InsertRange inserts ranges into the range tree.
    /// It returns a non-nil range if there are some overlapped ranges.
    ///
    /// 以 `StartKey` 插入/替换；若同键已存在则返回被替换的旧值（对齐 `BTree.ReplaceOrInsert`）。
    pub fn InsertRange(
        &mut self,
        rg: Range,
        rangeSize: u64,
        rangeCount: u64,
    ) -> Option<RangeStats> {
        let key = rg.KeyRange.StartKey.clone();
        self.BTreeG.insert(
            key,
            RangeStats {
                Range: rg,
                Size: rangeSize,
                Count: rangeCount,
            },
        )
    }

    /// MergedRanges output the sortedRanges having merged according to given thresholds.
    ///
    /// 按 StartKey 顺序扫描，将满足 `NeedsMerge` 的相邻区间并入同一目标：
    /// 扩展 EndKey、累加 Size/Count，并拼接 Files；否则开启新目标段。
    pub fn MergedRanges(&self, splitSizeBytes: u64, splitKeyCount: u64) -> Vec<RangeStats> {
        let mut mergeTargetIndex: isize = -1;
        let mut sortedRanges: Vec<RangeStats> = Vec::with_capacity(self.BTreeG.len());
        for rg in self.BTreeG.values() {
            // 尚无目标，或当前段与目标不可合并 → 新建目标段
            if mergeTargetIndex < 0
                || !NeedsMerge(
                    &sortedRanges[mergeTargetIndex as usize],
                    rg,
                    splitSizeBytes,
                    splitKeyCount,
                )
            {
                mergeTargetIndex += 1;
                sortedRanges.push(rg.clone());
            } else {
                // 可合并：右端并入左目标，文件列表追加
                let target = &mut sortedRanges[mergeTargetIndex as usize];
                target.Range.KeyRange.EndKey = rg.Range.KeyRange.EndKey.clone();
                target.Size = target.Size.wrapping_add(rg.Size);
                target.Count = target.Count.wrapping_add(rg.Count);
                target.Range.Files.extend(rg.Range.Files.clone());
            }
        }
        sortedRanges
    }
}

/// Strip API V2 keyspace prefix (`'x' || uint24(id)`) when present, matching
/// `tikv.DecodeKey(..., APIVersion_V2)` success path used by Go `NeedsMerge`.
///
/// 剥除 API V2 keyspace 前缀后再 `DecodeKeyHead`；失败则回退对原始键解码。
fn parse_inner_key(key: &[u8]) -> Result<(i64, i64, bool), String> {
    // V2 前缀：首字节 'x' + 3 字节 keyspace id
    if key.len() >= 4 && key[0] == b'x' {
        if let Ok(parsed) = stubs::DecodeKeyHead(&key[4..]) {
            return Ok(parsed);
        }
    }
    stubs::DecodeKeyHead(key)
}

/// NeedsMerge checks whether two ranges needs to be merged.
///
/// 判断左右区间是否应合并：右端体积为 0 则合并；超体积/键数阈值则拒绝；
/// 再按表/索引编码头约束——同行记录只比 tableID，同索引比 tableID+indexID，
/// 记录与索引混并一律拒绝。解码失败时跳过合并（与 Go 一致）。
pub fn NeedsMerge(
    left: &RangeStats,
    right: &RangeStats,
    splitSizeBytes: u64,
    splitKeyCount: u64,
) -> bool {
    let (leftBytes, leftKeys) = left.BytesAndKeys();
    let (rightBytes, rightKeys) = right.BytesAndKeys();
    // 右端无数据：总是并入左侧，避免留下空段
    if rightBytes == 0 {
        return true;
    }
    // 合并后超过体积阈值：不可合并
    if leftBytes.wrapping_add(rightBytes) > splitSizeBytes {
        return false;
    }
    // 合并后超过键数阈值：不可合并
    if leftKeys.wrapping_add(rightKeys) > splitKeyCount {
        return false;
    }

    // Go: tikv.DecodeKey(V2) then DecodeKeyHead(inner); on V2 failure DecodeKeyHead(key).
    let parsed1 = parse_inner_key(&left.Range.KeyRange.StartKey);
    let parsed2 = parse_inner_key(&right.Range.KeyRange.StartKey);

    if parsed1.is_err() || parsed2.is_err() {
        // Failed to decode the file key head — skip merge (same as Go).
        // 解码失败：保守拒绝合并，避免跨表/跨索引误并
        return false;
    }

    let (tableID1, indexID1, isRecord1) = parsed1.unwrap();
    let (tableID2, indexID2, isRecord2) = parsed2.unwrap();
    // 两边都是行记录：同表可合并
    if isRecord1 && isRecord2 {
        return tableID1 == tableID2;
    }
    // 两边都是索引：同表且同索引可合并
    if !isRecord1 && !isRecord2 {
        return tableID1 == tableID2 && indexID1 == indexID2;
    }
    // 记录与索引类型不同：不可合并
    false
}

/// RangeTree is sorted tree for Ranges. All stored ranges do not overlap.
///
/// 已完成备份区间树：按 StartKey 有序且**互不重叠**；`PhysicalID` 标识物理表/分区。
pub struct RangeTree {
    /// StartKey → 已备份区间。
    pub BTreeG: BTreeMap<Vec<u8>, Range>,
    /// 物理表/分区 ID，checksum 聚合时按此分组。
    pub PhysicalID: i64,
}

/// 构造空区间树（PhysicalID=0），对齐 Go `NewRangeTree`。
pub fn NewRangeTree() -> RangeTree {
    RangeTree {
        BTreeG: BTreeMap::new(),
        PhysicalID: 0,
    }
}

/// 带 PhysicalID 的构造；`FreeListG` 在 Go btree 中复用节点，Rust 侧忽略以保持签名对齐。
pub fn NewRangeTreeWithFreeListG(physicalID: i64, _f: FreeListG<Range>) -> RangeTree {
    RangeTree {
        BTreeG: BTreeMap::new(),
        PhysicalID: physicalID,
    }
}

impl RangeTree {
    /// 树中区间个数。
    pub fn Len(&self) -> usize {
        self.BTreeG.len()
    }

    /// 按 StartKey 精确查找；不存在返回 `None`。
    pub fn Get(&self, rg: &Range) -> Option<Range> {
        self.BTreeG.get(&rg.KeyRange.StartKey).cloned()
    }

    /// 按 StartKey 删除并返回旧值。
    pub fn Delete(&mut self, rg: &Range) -> Option<Range> {
        self.BTreeG.remove(&rg.KeyRange.StartKey)
    }

    /// 插入或替换同 StartKey 的区间，返回被替换旧值。
    pub fn ReplaceOrInsert(&mut self, rg: Range) -> Option<Range> {
        self.BTreeG.insert(rg.KeyRange.StartKey.clone(), rg)
    }

    /// 升序遍历；回调返回 `false` 时提前停止（对齐 Go `Ascend`）。
    pub fn Ascend<F: FnMut(&Range) -> bool>(&self, mut f: F) {
        for v in self.BTreeG.values() {
            if !f(v) {
                break;
            }
        }
    }

    /// Find is a helper function to find an item that contains the range start key.
    ///
    /// 取 `<= StartKey` 的最大项，再校验其 `Contains(StartKey)`；用于重叠定位与空洞扫描起点。
    pub fn Find(&self, rg: &Range) -> Option<Range> {
        let ret = self
            .BTreeG
            .range(..=rg.KeyRange.StartKey.clone())
            .next_back()
            .map(|(_k, v)| v.clone());

        // 候选不包含目标 StartKey 时视为未命中
        if ret
            .as_ref()
            .map(|r| !r.KeyRange.Contains(&rg.KeyRange.StartKey))
            .unwrap_or(true)
        {
            return None;
        }
        ret
    }

    /// 收集与 `rg` 重叠的全部区间：从 Find 起点向右扫描，直到超过 `rg.EndKey`。
    fn getOverlaps(&self, rg: &Range) -> Vec<Range> {
        // Find 失败时以自身为扫描起点（对齐 Go）
        let found = self.Find(rg).unwrap_or_else(|| rg.clone());

        let mut overlaps: Vec<Range> = Vec::new();
        for over in self
            .BTreeG
            .range(found.KeyRange.StartKey.clone()..)
            .map(|(_k, v)| v)
        {
            // 已越过查询上界：后续不可能再重叠
            if !rg.KeyRange.EndKey.is_empty()
                && rg.KeyRange.EndKey.as_slice() <= over.KeyRange.StartKey.as_slice()
            {
                break;
            }
            overlaps.push(over.clone());
        }
        overlaps
    }

    /// 强制更新：删除所有重叠项后插入 `rg`（`force=true`）。
    pub fn Update(&mut self, rg: Range) -> bool {
        self.updateForce(rg, true)
    }

    /// `force=false` 且存在重叠时拒绝并返回 false；否则清重叠后插入。
    fn updateForce(&mut self, rg: Range, force: bool) -> bool {
        let overlaps = self.getOverlaps(&rg);
        if !force && !overlaps.is_empty() {
            return false;
        }
        for item in overlaps {
            self.BTreeG.remove(&item.KeyRange.StartKey);
        }
        self.BTreeG.insert(rg.KeyRange.StartKey.clone(), rg);
        true
    }

    /// 以 force 模式写入区间及文件列表，覆盖任何重叠。
    pub fn Put(&mut self, startKey: Vec<u8>, endKey: Vec<u8>, files: Vec<File>) {
        let rg = Range {
            KeyRange: KeyRange {
                StartKey: startKey,
                EndKey: endKey,
            },
            Files: files,
        };
        self.updateForce(rg, true);
    }

    /// 可选强制写入；`force=false` 时重叠则失败返回 false。
    pub fn PutForce(
        &mut self,
        startKey: Vec<u8>,
        endKey: Vec<u8>,
        files: Vec<File>,
        force: bool,
    ) -> bool {
        let rg = Range {
            KeyRange: KeyRange {
                StartKey: startKey,
                EndKey: endKey,
            },
            Files: files,
        };
        self.updateForce(rg, force)
    }

    /// 直接按 StartKey 插入/替换，不做重叠清理（调用方需保证不重叠）。
    pub fn InsertRange(&mut self, rg: Range) -> Option<Range> {
        self.BTreeG.insert(rg.KeyRange.StartKey.clone(), rg)
    }

    /// GetIncompleteRange returns missing range covered by startKey and endKey.
    ///
    /// 在请求区间内找出尚未被树覆盖的空洞，供备份重试补齐。
    /// 退化点：`startKey==endKey`（且非空）视为空请求直接返回。
    pub fn GetIncompleteRange(&self, startKey: Vec<u8>, endKey: Vec<u8>) -> Vec<RpcKeyRange> {
        // 空请求（点区间）：无需补齐
        if !startKey.is_empty() && startKey == endKey {
            return Vec::new();
        }
        let mut incomplete: Vec<RpcKeyRange> = Vec::with_capacity(1);
        let requestRange = KeyRange {
            StartKey: startKey.clone(),
            EndKey: endKey.clone(),
        };
        let mut lastEndKey = startKey.clone();
        // 拼写 pviot 保持与 Go 变量名一致（pivot 的历史拼写）
        let mut pviot = Range {
            KeyRange: KeyRange {
                StartKey: startKey.clone(),
                EndKey: Vec::new(),
            },
            Files: Vec::new(),
        };
        // 若起点落在已有区间内，从该区间 StartKey 开始扫描
        if let Some(first) = self.Find(&pviot) {
            pviot.KeyRange.StartKey = first.KeyRange.StartKey;
        }
        let mut pviotNotFound = true;
        for rg in self
            .BTreeG
            .range(pviot.KeyRange.StartKey.clone()..)
            .map(|(_k, v)| v)
        {
            pviotNotFound = false;
            // lastEndKey 与当前区间起点之间存在空洞
            if lastEndKey.as_slice() < rg.KeyRange.StartKey.as_slice() {
                let (start, end, isIntersect) =
                    requestRange.Intersect(&lastEndKey, &rg.KeyRange.StartKey);
                if isIntersect {
                    incomplete.push(RpcKeyRange {
                        StartKey: start,
                        EndKey: end,
                    });
                }
            }
            lastEndKey = rg.KeyRange.EndKey.clone();
            // Go: return len(endKey) == 0 || bytes.Compare(rg.EndKey, endKey) < 0
            // 已覆盖到请求上界：停止扫描
            if !(endKey.is_empty() || rg.KeyRange.EndKey.as_slice() < endKey.as_slice()) {
                break;
            }
        }

        // 扫描结束后尾部仍有未覆盖空洞（或树为空）
        if pviotNotFound
            || (!lastEndKey.eq(&endKey)
                && !lastEndKey.is_empty()
                && (endKey.is_empty() || lastEndKey.as_slice() < endKey.as_slice()))
        {
            let (start, end, isIntersect) = requestRange.Intersect(&lastEndKey, &endKey);
            if isIntersect {
                incomplete.push(RpcKeyRange {
                    StartKey: start,
                    EndKey: end,
                });
            }
        }
        incomplete
    }
}

/// 进度单元：`Origin` 为原始请求区间，`Res` 为已完成子区间树。
pub struct ProgressRange {
    /// 该原始请求下已备份完成的子区间。
    pub Res: RangeTree,
    /// 调用方登记的原始备份请求区间。
    pub Origin: KeyRange,
}

impl ProgressRange {
    /// 按 Origin.StartKey 比较，供有序树排序。
    pub fn Less(&self, than: &ProgressRange) -> bool {
        self.Origin.StartKey < than.Origin.StartKey
    }
}

/// ProgressRangeTree is a sorted tree for ProgressRanges.
///
/// 进度区间树：登记互不重叠的 `ProgressRange`，完成后写出文件并更新 checksum。
pub struct ProgressRangeTree {
    /// Origin.StartKey → 进度单元。
    pub BTreeG: BTreeMap<Vec<u8>, ProgressRange>,
    /// PhysicalID → 累计校验统计。
    pub checksumMap: BTreeMap<i64, ChecksumStats>,
    /// 为 true 时跳过 checksum 累加（对齐 Go 开关）。
    pub skipChecksum: bool,
    /// 可选 meta 写出器；为 None 时 `collectRangeFiles` 只返回空统计。
    pub metaWriter: Option<Box<dyn MetaWriter>>,
    /// 单个 ProgressRange 完成时回调（进度条/计数）。
    pub completeCallBack: Box<dyn Fn() + Send>,
}

/// 构造进度树；默认空回调。`skipChecksum`/`metaWriter` 与 Go 构造参数对齐。
pub fn NewProgressRangeTree(
    metaWriter: Option<Box<dyn MetaWriter>>,
    skipChecksum: bool,
) -> ProgressRangeTree {
    ProgressRangeTree {
        BTreeG: BTreeMap::new(),
        checksumMap: BTreeMap::new(),
        skipChecksum,
        metaWriter,
        completeCallBack: Box::new(|| {}),
    }
}

impl ProgressRangeTree {
    /// 登记的进度区间个数。
    pub fn Len(&self) -> usize {
        self.BTreeG.len()
    }

    /// 设置完成回调，替换默认空闭包。
    pub fn SetCallBack(&mut self, callback: Box<dyn Fn() + Send>) {
        self.completeCallBack = callback;
    }

    /// 只读访问按 PhysicalID 聚合的 checksum 映射。
    pub fn GetChecksumMap(&self) -> &BTreeMap<i64, ChecksumStats> {
        &self.checksumMap
    }

    /// 查找包含 `pr.Origin.StartKey` 的已登记进度区间（floor + Contains）。
    fn find(&self, pr: &ProgressRange) -> Option<&ProgressRange> {
        let ret = self
            .BTreeG
            .range(..=pr.Origin.StartKey.clone())
            .next_back()
            .map(|(_k, v)| v);

        match ret {
            Some(item) if item.Origin.Contains(&pr.Origin.StartKey) => Some(item),
            _ => None,
        }
    }

    /// 插入进度区间；与已有 Origin 重叠则报错（不允许覆盖）。
    pub fn Insert(&mut self, pr: ProgressRange) -> Result<(), String> {
        if let Some(overlap) = self.find(&pr) {
            return Err(format!(
                "failed to insert the progress range into range tree, because there is a overlapping range. The insert item start key: {}; The overlapped item start key: {}, end key: {}.",
                stubs::redact_key(&pr.Origin.StartKey),
                stubs::redact_key(&overlap.Origin.StartKey),
                stubs::redact_key(&overlap.Origin.EndKey),
            ));
        }
        self.BTreeG.insert(pr.Origin.StartKey.clone(), pr);
        Ok(())
    }

    /// 查找完全包含 `[startKey, endKey]` 的进度区间；
    /// 命中但未完全包含时返回错误（region 越界），未命中返回 Ok(None)。
    pub fn FindContained(
        &self,
        startKey: Vec<u8>,
        endKey: Vec<u8>,
    ) -> Result<Option<&ProgressRange>, String> {
        let startPr = ProgressRange {
            Origin: KeyRange {
                StartKey: startKey.clone(),
                EndKey: endKey.clone(),
            },
            Res: NewRangeTree(),
        };
        let ret = self.find(&startPr);

        let Some(item) = ret else {
            return Ok(None);
        };

        // 找到的 Origin 必须完全包住请求 region
        if !item.Origin.ContainsRange(&startKey, &endKey) {
            return Err(format!(
                "The given region is not contained in the found progress range. The region start key is {:?}; The progress range start key is {}, end key is {}.",
                startKey,
                stubs::redact_key(&item.Origin.StartKey),
                stubs::redact_key(&item.Origin.EndKey),
            ));
        }
        Ok(Some(item))
    }

    /// 汇总所有 Origin 的未完成子区间；已完成的项写出文件、回调进度并移出树。
    /// 先收集待删键再删除，避免遍历中修改 BTree（Rust 所有权约束）。
    pub fn GetIncompleteRanges(&mut self) -> Result<Vec<RpcKeyRange>, String> {
        let mut incompleteRanges: Vec<RpcKeyRange> = Vec::with_capacity(self.BTreeG.len());
        let mut deletedRanges: Vec<DeletedRange> = Vec::new();
        let mut rangeAscendErr: Option<String> = None;

        // 先克隆键列表，避免边遍历边修改
        let keys: Vec<Vec<u8>> = self.BTreeG.keys().cloned().collect();
        for key in keys {
            let item = self.BTreeG.get(&key).expect("key collected from tree");
            let incomplete = item
                .Res
                .GetIncompleteRange(item.Origin.StartKey.clone(), item.Origin.EndKey.clone());
            if incomplete.is_empty() {
                // Origin 已完全覆盖：收集文件并标记删除
                match self.collectRangeFiles(item) {
                    Ok(checksum) => {
                        deletedRanges.push(DeletedRange {
                            key: key.clone(),
                            checksum,
                        });
                        (self.completeCallBack)();
                    }
                    Err(err) => {
                        rangeAscendErr = Some(err);
                        break;
                    }
                }
            } else {
                incompleteRanges.extend(incomplete);
            }
        }
        if let Some(err) = rangeAscendErr {
            return Err(err);
        }
        // 第二阶段：删除已完成项并（可选）更新 checksum
        for deletedRange in deletedRanges {
            if let Some(rg) = self.BTreeG.remove(&deletedRange.key) {
                if !self.skipChecksum {
                    self.UpdateChecksum(
                        rg.Res.PhysicalID,
                        deletedRange.checksum.Crc64Xor,
                        deletedRange.checksum.TotalKvs,
                        deletedRange.checksum.TotalBytes,
                    );
                }
            }
        }
        Ok(incompleteRanges)
    }

    /// 将完成区间的 Files 发给 MetaWriter，并异或汇总 crc/kvs/bytes。
    fn collectRangeFiles(&self, item: &ProgressRange) -> Result<ChecksumStats, String> {
        let mut checksum = ChecksumStats::default();
        // 无写出器时跳过 Send，仅返回默认空统计
        if self.metaWriter.is_none() {
            return Ok(checksum);
        }
        let writer = self.metaWriter.as_ref().unwrap();
        for r in item.Res.BTreeG.values() {
            let (crc, kvs, bytes) = stubs::SummaryFiles(&r.Files);
            writer.Send(&r.Files, AppendDataFile)?;
            checksum.Crc64Xor ^= crc;
            checksum.TotalKvs = checksum.TotalKvs.wrapping_add(kvs);
            checksum.TotalBytes = checksum.TotalBytes.wrapping_add(bytes);
        }
        Ok(checksum)
    }

    /// 按 PhysicalID 累加 checksum（Crc64Xor 异或，kvs/bytes 相加）。
    pub fn UpdateChecksum(&mut self, physicalID: i64, crc: u64, kvs: u64, bytes: u64) {
        let ckm = self
            .checksumMap
            .entry(physicalID)
            .or_insert_with(ChecksumStats::default);
        ckm.Crc64Xor ^= crc;
        ckm.TotalKvs = ckm.TotalKvs.wrapping_add(kvs);
        ckm.TotalBytes = ckm.TotalBytes.wrapping_add(bytes);
    }
}

/// 已完成待删除的进度项：缓存键与预计算 checksum，供第二阶段删除使用。
pub struct DeletedRange {
    /// BTree 中的 Origin.StartKey。
    pub key: Vec<u8>,
    /// `collectRangeFiles` 预计算的校验统计。
    pub checksum: ChecksumStats,
}
