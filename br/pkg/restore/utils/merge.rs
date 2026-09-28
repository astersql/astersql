// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc. Licensed under Apache-2.0.

//! 合并备份文件区间并套用键重写，对齐 Go `merge.go`。
//! 同 StartKey 的 write/default CF 文件归为一组；经 RewriteRange 改写后
//! 插入 RangeStatsTree，再按 splitSizeBytes/splitKeyCount 合并为较少 Region。
//! 统计字段供日志与上层调度判断合并收益；非法 CF 返回 ErrRestoreInvalidBackup。

//! Merge backup file ranges matching `merge.go`.

// HashMap 按 StartKey 归并同区间的 write/default 文件。
use std::collections::HashMap;

// InvalidRange：重写/重复区间；InvalidBackup：无法识别 CF。
use astersql_br_pkg_errors::{ErrInvalidRange, ErrRestoreInvalidBackup};
// RangeStatsTree 负责插入与按阈值 MergedRanges。
use astersql_br_pkg_rtree::{File as RtreeFile, KeyRange, NewRangeStatsTree, Range, RangeStats};
use astersql_errors::{Annotatef, SharedError};

// CF 名常量与 misc 共用，保证合并与过滤识别一致。
use crate::misc::{DefaultCFName, WriteCFName};
// 合并前先 RewriteRange，使区间键落在目标表空间。
use crate::rewrite_rule::{RewriteRange, RewriteRules};
use crate::stubs::backuppb;

/// 将静态错误包装为 SharedError，供 Annotatef 链式标注。
fn br_err(err: &'static astersql_errors::Error) -> SharedError {
    SharedError::new(err.clone())
}

/// MergeRanges 产出统计：文件/CF 计数、合并前后 Region 规模均值。
/// MergeRangesStat holds statistics for the MergeRanges.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MergeRangesStat {
    pub TotalFiles: i32,
    pub TotalWriteCFFile: i32,
    pub TotalDefaultCFFile: i32,
    /// 合并前 Region 数，取 write/default CF 文件数较大者。
    pub TotalRegions: i32,
    pub RegionKeysAvg: i32,
    pub RegionBytesAvg: i32,
    /// 合并后区间数，对应 MergedRanges 返回长度。
    pub MergedRegions: i32,
    pub MergedRegionKeysAvg: i32,
    pub MergedRegionBytesAvg: i32,
}

/// 将 backuppb::File 转为 rtree 侧 File，仅保留合并/区间树需要的字段。
fn to_rtree_files(files: &[backuppb::File]) -> Vec<RtreeFile> {
    files
        .iter()
        .map(|f| RtreeFile {
            Name: f.Name.clone(),
            Cf: f.Cf.clone(),
            StartKey: f.StartKey.clone(),
            EndKey: f.EndKey.clone(),
            TotalBytes: f.TotalBytes,
            TotalKvs: f.TotalKvs,
            Crc64Xor: f.Crc64Xor,
        })
        .collect()
}

/// 按 StartKey 分组、重写键前缀后合并区间，对齐 Go MergeAndRewriteFileRanges。
/// splitSizeBytes/splitKeyCount 为合并阈值：超过则切开，避免单 Region 过大。
/// MergeAndRewriteFileRanges returns ranges of the files are merged based on
/// splitSizeBytes and splitKeyCount.
pub fn MergeAndRewriteFileRanges(
    files: Vec<backuppb::File>,
    rewriteRules: Option<&RewriteRules>,
    splitSizeBytes: u64,
    splitKeyCount: u64,
) -> Result<(Vec<RangeStats>, MergeRangesStat), SharedError> {
    // 空输入：直接返回空区间与零统计，与 Go 行为一致。
    if files.is_empty() {
        return Ok((Vec::new(), MergeRangesStat::default()));
    }

    // 累计全量字节/键数，用于合并前后均值统计。
    let mut totalBytes: u64 = 0;
    let mut totalKvs: u64 = 0;
    // TotalFiles 统计输入文件数，与 CF 分类计数独立。
    let totalFiles = files.len() as i32;
    let mut writeCFFile: i32 = 0;
    let mut defaultCFFile: i32 = 0;

    // 同 StartKey 归桶；桶内 EndKey 必须一致，否则视为脏数据 panic。
    let mut filesMap: HashMap<Vec<u8>, Vec<backuppb::File>> = HashMap::new();
    for file in files {
        let start_key = file.StartKey.clone();
        let bucket = filesMap.entry(start_key.clone()).or_default();
        let end_key = file.EndKey.clone();
        let name = file.Name.clone();
        let cf = file.Cf.clone();
        // 先取出统计再 push，避免 move 后无法读取。
        let total_bytes = file.TotalBytes;
        let total_kvs = file.TotalKvs;
        bucket.push(file);

        // 同 StartKey 桶内首尾 EndKey 必须相等。
        let first = &bucket[0];
        let last = bucket.last().expect("bucket just pushed");
        if first.EndKey != last.EndKey {
            crate::stubs::log::Panic(
                "there are two files having the same start key, but different end key",
            );
        }

        // CF 字段优先；文件名包含 write/default 作为兼容回退（历史备份）。
        if cf == WriteCFName || name.contains(WriteCFName) {
            writeCFFile += 1;
        } else if cf == DefaultCFName || name.contains(DefaultCFName) {
            defaultCFFile += 1;
        }
        totalBytes += total_bytes;
        totalKvs += total_kvs;
        let _ = (start_key, end_key);
    }

    // 既非 write 也非 default：备份数据不可识别。
    if writeCFFile == 0 && defaultCFFile == 0 {
        return Err(Annotatef(
            Some(br_err(&ErrRestoreInvalidBackup)),
            "unknown backup data from neither Wrtie CF nor Default CF",
            &[],
        )
        .expect("annotate"));
    }

    // Region 数取两侧 CF 较大值（一对 write+default 计为一个逻辑区间）。
    let totalRegions = defaultCFFile.max(writeCFFile);

    // 逐组构造 Range，累加组内 Size/Count 后插入树。
    let mut rangeTree = NewRangeStatsTree();
    for group_files in filesMap.into_values() {
        // 组内所有 CF 文件的字节/键合计作为区间权重。
        let mut rangeSize: u64 = 0;
        let mut rangeCount: u64 = 0;
        for f in &group_files {
            rangeSize += f.TotalBytes;
            rangeCount += f.TotalKvs;
        }

        // 区间键取组内首文件起止（同 StartKey 时 EndKey 已校验一致）。
        let mut rg = Range {
            KeyRange: KeyRange {
                StartKey: group_files[0].GetStartKey(),
                EndKey: group_files[0].GetEndKey(),
            },
            Files: to_rtree_files(&group_files),
        };

        // 先按规则改写区间键，再插入区间树；重复区间视为非法。
        let tmpRng = RewriteRange(&mut rg, rewriteRules).map_err(|_err| {
            let msg = format!("unable to rewrite range files {group_files:?}");
            Annotatef(Some(br_err(&ErrInvalidRange)), &msg, &[]).expect("annotate")
        })?;
        if let Some(out) = rangeTree.InsertRange(tmpRng, rangeSize, rangeCount) {
            let msg = format!("duplicate range {out:?} files {group_files:?}");
            return Err(Annotatef(Some(br_err(&ErrInvalidRange)), &msg, &[]).expect("annotate"));
        }
    }

    // 按字节/键数阈值合并相邻小区间。
    let sortedRanges = rangeTree.MergedRanges(splitSizeBytes, splitKeyCount);
    let merged_regions = sortedRanges.len();
    let regionBytesAvg = totalBytes / totalRegions as u64;
    let regionKeysAvg = totalKvs / totalRegions as u64;
    // 合并结果为空时均值置 0，避免除零。
    let mergedRegionBytesAvg = if merged_regions == 0 {
        0
    } else {
        totalBytes / merged_regions as u64
    };
    let mergedRegionKeysAvg = if merged_regions == 0 {
        0
    } else {
        totalKvs / merged_regions as u64
    };

    Ok((
        sortedRanges,
        MergeRangesStat {
            TotalFiles: totalFiles,
            TotalWriteCFFile: writeCFFile,
            TotalDefaultCFFile: defaultCFFile,
            TotalRegions: totalRegions,
            RegionKeysAvg: regionKeysAvg as i32,
            RegionBytesAvg: regionBytesAvg as i32,
            MergedRegions: merged_regions as i32,
            MergedRegionKeysAvg: mergedRegionKeysAvg as i32,
            MergedRegionBytesAvg: mergedRegionBytesAvg as i32,
        },
    ))
}
