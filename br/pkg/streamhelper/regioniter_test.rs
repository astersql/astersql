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

//! Region 迭代器用例，对齐 Go `regioniter_test.go`。
//!
//! 用静态 `ConstantRegions` 假集群验证 `IterateRegion` 在分页、边界与全键空间
//! 场景下收集到的 region 边界与期望一致。
//!
//! 边界键用空串表示 ±∞；`many_regions` 生成六位数字边界以覆盖万级分页。
//! Go-equivalent tests from `regioniter_test.go`.

use astersql_br_pkg_streamhelper_spans::{Overlaps, Span};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use crate::regioniter::{IterateRegion, RegionWithLeader, Store, TiKVClusterMeta};
use crate::stubs::{KeyRange, Peer, Region, RegionEpoch};

/// 固定有序 region 列表，作为 `TiKVClusterMeta` 的内存替身。
/// 假定内部已按 StartKey 升序，供扫描提前 break。
#[derive(Clone)]
struct ConstantRegions(Vec<RegionWithLeader>);

/// 抽取 region 键范围，便于边界比较。
/// 忽略 Id/Epoch，只关心半开区间几何。
fn region_to_range(region: &RegionWithLeader) -> KeyRange {
    KeyRange {
        StartKey: region.Region.StartKey.clone(),
        EndKey: region.Region.EndKey.clone(),
    }
}

impl ConstantRegions {
    /// 仅比较 StartKey/EndKey 序列是否一致（忽略 Id/Epoch）。
    /// 长度不同或任一段边界不等即判失败。
    fn equals_to(&self, other: &[RegionWithLeader]) -> bool {
        if self.0.len() != other.len() {
            return false;
        }
        for (left, right) in self.0.iter().zip(other.iter()) {
            let r1 = region_to_range(left);
            let r2 = region_to_range(right);
            if r1.StartKey != r2.StartKey || r1.EndKey != r2.EndKey {
                return false;
            }
        }
        true
    }

    /// 调试用展示：`id[start,end);...`。
    fn display(&self) -> String {
        self.0
            .iter()
            .map(|r| {
                let rng = region_to_range(r);
                format!("{}[{:?},{:?})", r.Region.Id, rng.StartKey, rng.EndKey)
            })
            .collect::<Vec<_>>()
            .join(";")
    }
}

impl TiKVClusterMeta for ConstantRegions {
    /// 按重叠与 limit 过滤；列表已按 StartKey 排序，越过查询起点可提前结束。
    fn RegionScan(
        &self,
        key: &[u8],
        endKey: &[u8],
        limit: i32,
    ) -> Result<Vec<RegionWithLeader>, String> {
        let mut result = Vec::new();
        for region in &self.0 {
            let a = Span {
                StartKey: key.to_vec(),
                EndKey: endKey.to_vec(),
            };
            let b = Span {
                StartKey: region.Region.StartKey.clone(),
                EndKey: region.Region.EndKey.clone(),
            };
            if Overlaps(&a, &b) && (result.len() as i32) < limit {
                result.push(region.clone());
            } else if region.Region.StartKey.as_slice() > key {
                // 后续 region 更靠右，不可能再重叠。
                break;
            }
        }
        Ok(result)
    }

    fn Stores(&self) -> Result<Vec<Store>, String> {
        Err("Unsupported operation".into())
    }

    fn BlockGCUntil(&self, _at: u64) -> Result<u64, String> {
        Err("Unsupported operation".into())
    }

    fn UnblockGC(&self) -> Result<(), String> {
        Err("Unsupported operation".into())
    }

    /// 用墙钟毫秒左移 18 位伪造 TS，满足接口；本测试不依赖具体值。
    fn FetchCurrentTS(&self) -> Result<u64, String> {
        let ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        Ok(ms << 18)
    }
}

/// 由相邻边界键构造连续 region：`keys[i]..keys[i+1]`。
/// Id 从 1 递增；Leader 置默认，本测试不依赖 Peer。
fn make_subrange_regions(keys: &[&str]) -> ConstantRegions {
    if keys.is_empty() {
        return ConstantRegions(Vec::new());
    }
    let mut id = 1u64;
    let mut regions = Vec::new();
    let mut start = keys[0];
    for key in &keys[1..] {
        regions.push(RegionWithLeader {
            Region: Region {
                Id: id,
                StartKey: start.as_bytes().to_vec(),
                EndKey: key.as_bytes().to_vec(),
                RegionEpoch: RegionEpoch::default(),
            },
            Leader: Peer::default(),
        });
        id += 1;
        start = key;
    }
    ConstantRegions(regions)
}

/// 在边界两侧补空串，表示从 -∞ 到 +∞ 的完整切分（空键 = 无穷）。
/// 对应 Go 测试里对全键空间的 region 切分方式。
fn use_regions(keys: &[&str]) -> ConstantRegions {
    let mut ks = Vec::with_capacity(keys.len() + 2);
    ks.push("");
    ks.extend_from_slice(keys);
    ks.push("");
    make_subrange_regions(&ks)
}

/// 生成零填充六位数字边界，便于大规模分页用例。
/// 固定宽度保证字典序与数值序一致。
fn many_regions(from: i32, to: i32) -> Vec<String> {
    (from..to).map(|i| format!("{i:06}")).collect()
}

/// 在列表前插入空起点（覆盖从 -∞ 开始）。
/// 用于构造「从键空间开头」的期望边界序列。
fn append_initial(a: &[String]) -> Vec<String> {
    let mut v = vec![String::new()];
    v.extend_from_slice(a);
    v
}

/// 在列表末插入空终点（覆盖到 +∞）。
/// 用于构造「扫到键空间尽头」的期望边界序列。
fn append_final(a: &[String]) -> Vec<String> {
    let mut v = a.to_vec();
    v.push(String::new());
    v
}

/// 单条用例：集群边界、查询区间与期望收集到的边界序列。
/// `required_region_boundary` 传给 `make_subrange_regions` 生成期望列表。
struct Case {
    /// 内部切分点（不含两端无穷）。
    region_boundary: Vec<String>,
    /// 迭代查询起点。
    start_key: String,
    /// 迭代查询终点；空串表示 +∞。
    end_key: String,
    /// 期望收集到的边界键序列（可含空串）。
    required_region_boundary: Vec<String>,
}

#[test]
fn test_region_iterator() {
    // 覆盖：局部窗口、左侧裁剪、大规模分页、到 +∞、从 -∞ 到中点。
    let cases = vec![
        // 落在尾部窗口，应收齐覆盖 [0077,0079) 的 region 边界。
        Case {
            region_boundary: vec!["0001", "0003", "0008", "0078"]
                .into_iter()
                .map(str::to_string)
                .collect(),
            start_key: "0077".into(),
            end_key: "0079".into(),
            required_region_boundary: vec!["0008", "0078", ""]
                .into_iter()
                .map(str::to_string)
                .collect(),
        },
        // 查询起点早于首边界，期望含空起点。
        Case {
            region_boundary: vec!["0001", "0005", "0008", "0097"]
                .into_iter()
                .map(str::to_string)
                .collect(),
            start_key: "0000".into(),
            end_key: "0008".into(),
            required_region_boundary: vec!["", "0001", "0005", "0008"]
                .into_iter()
                .map(str::to_string)
                .collect(),
        },
        // 万级 region：验证分页拼接后边界连续。
        Case {
            region_boundary: many_regions(0, 10000),
            start_key: "000001".into(),
            end_key: "005000".into(),
            required_region_boundary: many_regions(1, 5001),
        },
        // 从 000100 扫到 +∞。
        Case {
            region_boundary: many_regions(0, 10000),
            start_key: "000100".into(),
            end_key: "".into(),
            required_region_boundary: append_final(&many_regions(100, 10000)),
        },
        // 从 -∞ 扫到 003000。
        Case {
            region_boundary: many_regions(0, 10000),
            start_key: "".into(),
            end_key: "003000".into(),
            required_region_boundary: append_initial(&many_regions(0, 3001)),
        },
    ];
    // 逐案跑通：失败时打印期望与实际边界串便于对照 Go。
    for (i, c) in cases.iter().enumerate() {
        let boundary: Vec<&str> = c.region_boundary.iter().map(|s| s.as_str()).collect();
        let required: Vec<&str> = c
            .required_region_boundary
            .iter()
            .map(|s| s.as_str())
            .collect();
        let regions = use_regions(&boundary);
        let required_regions = make_subrange_regions(&required);
        let mut collected = Vec::new();
        // 完整消费迭代器，断言收集边界与期望一致。
        let mut iter = IterateRegion(&regions, c.start_key.as_bytes(), c.end_key.as_bytes());
        while !iter.Done() {
            let page = iter.Next().unwrap_or_else(|e| panic!("case#{i} next: {e}"));
            collected.extend(page);
        }
        assert!(
            required_regions.equals_to(&collected),
            "case#{i}: {} :: {}",
            required_regions.display(),
            ConstantRegions(collected).display()
        );
    }
}

/// 前一次扫描失败、第二次成功，用于验证 Go 固定 500ms 退避契约。
struct FailOnceRegions {
    calls: AtomicUsize,
    region: RegionWithLeader,
}

impl TiKVClusterMeta for FailOnceRegions {
    fn RegionScan(
        &self,
        _key: &[u8],
        _endKey: &[u8],
        _limit: i32,
    ) -> Result<Vec<RegionWithLeader>, String> {
        if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
            Err("transient scan failure".into())
        } else {
            Ok(vec![self.region.clone()])
        }
    }

    fn Stores(&self) -> Result<Vec<Store>, String> {
        Err("Unsupported operation".into())
    }

    fn BlockGCUntil(&self, _at: u64) -> Result<u64, String> {
        Err("Unsupported operation".into())
    }

    fn UnblockGC(&self) -> Result<(), String> {
        Err("Unsupported operation".into())
    }

    fn FetchCurrentTS(&self) -> Result<u64, String> {
        Err("Unsupported operation".into())
    }
}

#[test]
fn next_uses_go_equivalent_retry_backoff() {
    let cli = FailOnceRegions {
        calls: AtomicUsize::new(0),
        region: RegionWithLeader {
            Region: Region {
                Id: 1,
                StartKey: Vec::new(),
                EndKey: Vec::new(),
                RegionEpoch: RegionEpoch::default(),
            },
            Leader: Peer::default(),
        },
    };
    let mut iter = IterateRegion(&cli, b"key", b"");

    let started = Instant::now();
    let regions = iter.Next().expect("the second scan should succeed");

    assert_eq!(cli.calls.load(Ordering::SeqCst), 2);
    assert_eq!(regions.len(), 1);
    assert!(
        started.elapsed() >= Duration::from_millis(450),
        "retry must honor Go's 500ms backoff"
    );
}
