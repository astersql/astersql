// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc. Licensed under Apache-2.0.

//! Region 分页迭代与一致性校验；对齐 Go `regioniter.go`。
//!
//! `RegionIter` 通过 `TiKVClusterMeta::RegionScan` 按页拉取带 leader 的 region，
//! 在推进游标前用 `CheckRegionConsistency` 保证覆盖连续无空洞。
//!
//! 空 `endKey` 表示扫到键空间尽头；末 region `EndKey` 为空时置 `infScanFinished`。
//! 瞬时扫描失败经 `with_retry` 最多尝试 8 次，固定退避 500ms，吸收 PD 抖动。

use std::cmp::Ordering;
use std::thread;
use std::time::Duration;

use crate::stubs::{Peer, Region, key_next};

/// 默认每页 region 数，对齐 Go `defaultPageSize`。
pub const defaultPageSize: i32 = 2048;

/// Region 元数据加上其 leader Peer，供扫描与收集检查点使用。
/// Leader 用于按 store 聚合 flush TS 请求。
#[derive(Clone, Debug, Default)]
pub struct RegionWithLeader {
    pub Region: Region,
    pub Leader: Peer,
}

/// TiKV store 简要信息：ID 与启动时间。
/// BootAt 可用于判断 store 重启后是否需重建订阅。
#[derive(Clone, Debug, Default)]
pub struct Store {
    pub ID: u64,
    pub BootAt: u64,
}

/// 集群元数据抽象：扫描 region、列举 store、控制 GC 与取当前 TS。
pub trait TiKVClusterMeta: Send + Sync {
    /// 扫描覆盖 `[key, endKey)` 的 region，最多 `limit` 个。
    fn RegionScan(
        &self,
        key: &[u8],
        endKey: &[u8],
        limit: i32,
    ) -> Result<Vec<RegionWithLeader>, String>;
    fn Stores(&self) -> Result<Vec<Store>, String>;
    /// 阻塞 GC 到指定 TS，返回实际生效值。
    fn BlockGCUntil(&self, at: u64) -> Result<u64, String>;
    fn UnblockGC(&self) -> Result<(), String>;
    fn FetchCurrentTS(&self) -> Result<u64, String>;
}

/// 半开区间上的 region 分页迭代器；`endKey` 为空表示扫到 +∞。
/// 状态字段公开以便测试与日志打印当前游标。
pub struct RegionIter<'a> {
    pub cli: &'a dyn TiKVClusterMeta,
    /// 原始查询起点（不变）。
    pub startKey: Vec<u8>,
    /// 原始查询终点；空表示 +∞。
    pub endKey: Vec<u8>,
    /// 下一页扫描起点，随 `Next` 推进到上一页末 region 的 EndKey。
    pub currentStartKey: Vec<u8>,
    /// 是否已见到 EndKey 为空的末 region（无限扫描完成标记）。
    pub infScanFinished: bool,
    /// 每页上限；默认 `defaultPageSize`，测试可调小以强制翻页。
    pub PageSize: i32,
}

impl std::fmt::Display for RegionIter<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "RegionIter:[{:?},{:?});{};from={:?}",
            self.currentStartKey, self.endKey, self.infScanFinished, self.startKey
        )
    }
}

/// 构造从 `[startKey, endKey)` 开始的迭代器，页大小取默认值。
pub fn IterateRegion<'a>(
    cli: &'a dyn TiKVClusterMeta,
    startKey: &[u8],
    endKey: &[u8],
) -> RegionIter<'a> {
    RegionIter {
        cli,
        startKey: startKey.to_vec(),
        endKey: endKey.to_vec(),
        currentStartKey: startKey.to_vec(),
        infScanFinished: false,
        PageSize: defaultPageSize,
    }
}

/// 定位包含 `key` 的单个 region（扫描 `[key, key_next)` limit=1）。
pub fn locateKeyOfRegion(
    cli: &dyn TiKVClusterMeta,
    key: &[u8],
) -> Result<RegionWithLeader, String> {
    let regions = cli.RegionScan(key, &key_next(key), 1)?;
    if regions.is_empty() {
        return Err(format!("scanning the key {:?} returns empty region", key));
    }
    Ok(regions[0].clone())
}

/// 校验扫描结果：非空、首尾覆盖请求区间、相邻 region EndKey==下一 StartKey。
/// 空 EndKey 表示 +∞，不参与「末尾过短」判断。
pub fn CheckRegionConsistency(
    startKey: &[u8],
    endKey: &[u8],
    regions: &[RegionWithLeader],
) -> Result<(), String> {
    if regions.is_empty() {
        return Err(format!(
            "scan region return empty result, startKey: {:?}, endKey: {:?}",
            startKey, endKey
        ));
    }
    // 首 region 起点不得晚于请求 startKey，否则左侧有空洞。
    if regions[0].Region.StartKey.as_slice().cmp(startKey) == Ordering::Greater {
        return Err(format!(
            "first region's startKey > startKey, startKey: {:?}, regionStartKey: {:?}",
            startKey, regions[0].Region.StartKey
        ));
    } else if !regions[regions.len() - 1].Region.EndKey.is_empty()
        && regions[regions.len() - 1]
            .Region
            .EndKey
            .as_slice()
            .cmp(endKey)
            == Ordering::Less
    {
        // 末 region 有限 EndKey 且短于请求 endKey → 右侧未覆盖。
        return Err(format!(
            "last region's endKey < endKey, endKey: {:?}, regionEndKey: {:?}",
            endKey,
            regions[regions.len() - 1].Region.EndKey
        ));
    }

    // 相邻 region 必须首尾相接，禁止空洞或重叠错位。
    let mut cur = &regions[0];
    for r in &regions[1..] {
        if cur.Region.EndKey != r.Region.StartKey {
            return Err(format!(
                "region endKey not equal to next region startKey, endKey: {:?}, startKey: {:?}",
                cur.Region.EndKey, r.Region.StartKey
            ));
        }
        cur = r;
    }
    Ok(())
}

/// 最多尝试 8 次（固定间隔 500ms），吸收瞬时 PD 扫描抖动，对齐 Go 重试语义。
fn with_retry<F, T>(mut f: F) -> Result<T, String>
where
    F: FnMut() -> Result<T, String>,
{
    let mut last = String::new();
    for _ in 0..8 {
        match f() {
            Ok(v) => return Ok(v),
            Err(e) => {
                last = e;
                // Go's BackoffStrategy advances and sleeps after every failed
                // attempt, including the final one, before reporting failure.
                thread::sleep(Duration::from_millis(500));
            }
        }
    }
    Err(last)
}

impl RegionIter<'_> {
    /// 拉取下一页：扫描后做一致性检查；空结果也走一致性错误路径。
    /// 若末 region EndKey 为空则标记无限扫描完成，并推进 `currentStartKey`。
    pub fn Next(&mut self) -> Result<Vec<RegionWithLeader>, String> {
        let page = self.PageSize;
        let cur = self.currentStartKey.clone();
        let end = self.endKey.clone();
        let cli = self.cli;
        let rs = with_retry(|| {
            let regions = cli.RegionScan(&cur, &end, page)?;
            if !regions.is_empty() {
                // 有结果时，一致性上界取本页最后 region 的 EndKey。
                let end_key = regions[regions.len() - 1].Region.GetEndKey().to_vec();
                CheckRegionConsistency(&cur, &end_key, &regions)?;
                return Ok(regions);
            }
            // 空结果：用请求 end 触发一致性错误（与 Go 相同）。
            Err(CheckRegionConsistency(&cur, &end, &regions).unwrap_err())
        })?;
        let end_key = rs[rs.len() - 1].Region.EndKey.clone();
        if end_key.is_empty() {
            self.infScanFinished = true;
        }
        self.currentStartKey = end_key;
        Ok(rs)
    }

    /// 结束条件：无限扫描看 `infScanFinished`；否则游标已达/越过 `endKey`。
    pub fn Done(&self) -> bool {
        if self.endKey.is_empty() {
            return self.infScanFinished;
        }
        self.infScanFinished
            || self.currentStartKey.as_slice().cmp(self.endKey.as_slice()) != Ordering::Less
    }
}
