// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc. Licensed under Apache-2.0.
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

//! 对应 Go `merge_test.go`：验证 MergeAndRewriteFileRanges 的合并/切分语义。
//! 夹具为纯算法构造，不依赖 TiKV / PD / 网络；断言对齐 Go 用例的
//! TotalRegions、MergedRegions 与各合并区间内文件数。
//! Go-equivalent tests for `br/pkg/restore/utils/merge_test.go`.
//! Pure algorithm fixtures — no TiKV / PD / network.
//! merged 向量描述每个输出 Range 内 Files 长度，而非全局文件总数。

use astersql_br_pkg_errors::ErrRestoreInvalidBackup;

use crate::stubs::{backuppb, codec, tablecodec};
use crate::{MergeAndRewriteFileRanges, MergeRangesStat};

// 默认合并阈值与生产 conn 常量保持数值一致，便于对照 Go 基准用例。

/// 镜像 `conn.DefaultMergeRegionSizeBytes`（96 MiB），避免依赖 conn 包。
/// Mirrors `conn.DefaultMergeRegionSizeBytes` (96 MiB) without depending on conn.
const DEFAULT_MERGE_REGION_SIZE_BYTES: u64 = 96 * 1024 * 1024;
/// 镜像 `conn.DefaultMergeRegionKeyCount`，与默认键数合并阈值一致。
/// Mirrors `conn.DefaultMergeRegionKeyCount`.
const DEFAULT_MERGE_REGION_KEY_COUNT: u64 = 960_000;

/// 构造连续 key 区间的 SST 文件；切换 table_id 时重置 startKeyOffset。
/// 保留 Go 侧 fileBulder 拼写习惯，便于对照源用例。
/// fileBulder — Go spelling preserved; tracks startKeyOffset across table switches.
struct FileBuilder {
    /// 当前正在生成键的表 ID。
    table_id: i64,
    /// 同行/同索引内的 handle 偏移，步进 10。
    start_key_offset: i64,
}

impl FileBuilder {
    /// 初始 table_id/offset 为 0，首次 build 即视为换表重置。
    fn new() -> Self {
        Self {
            table_id: 0,
            start_key_offset: 0,
        }
    }

    /// 生成文件：tableID、indexID(0=行键)、num(1=仅 write / 2=write+default)、bytes、kv。
    /// build — tableID, indexID, num(1|2), bytes, kv.
    fn build(
        &mut self,
        table_id: i32,
        index_id: i32,
        num: i32,
        bytes: i32,
        kv: i32,
    ) -> Vec<backuppb::File> {
        // num 仅允许 1 或 2，对应 Go 侧「单 CF / 双 CF」两种备份形态。
        assert!(num == 1 || num == 2, "num must be 1 or 2");

        // 换表时 offset 归零，保证同表内区间单调递增、跨表键空间独立。
        if self.table_id != table_id as i64 {
            self.table_id = table_id as i64;
            self.start_key_offset = 0;
        }

        // 每段占用 10 个编码整数跨度，保证相邻文件键区间不重叠。
        let low = codec::EncodeInt(None, self.start_key_offset);
        self.start_key_offset += 10;
        let high = codec::EncodeInt(None, self.start_key_offset);

        let (start_key, end_key) = if index_id != 0 {
            // Go: codec.EncodeKey of IntDatum ≈ EncodeInt（省略类型标）；
            // DecodeKeyHead 只读表/索引前缀，相对序仍与 Go 一致。
            // Go: codec.EncodeKey of IntDatum ≈ EncodeInt (type flag omitted; DecodeKeyHead
            // only reads table/index prefix, and relative ordering is preserved).
            let low_value = codec::EncodeInt(None, self.start_key_offset - 10);
            let high_value = codec::EncodeInt(None, self.start_key_offset);
            (
                encode_index_seek_key(table_id as i64, index_id as i64, &low_value),
                encode_index_seek_key(table_id as i64, index_id as i64, &high_value),
            )
        } else {
            // index_id==0：使用行记录前缀，模拟普通表数据 SST。
            (
                encode_row_key(self.table_id, &low),
                encode_row_key(self.table_id, &high),
            )
        };

        let mut files = vec![backuppb::File {
            Name: format!("{}_write.sst", rand_name()),
            StartKey: start_key,
            EndKey: end_key,
            TotalKvs: kv as u64,
            TotalBytes: bytes as u64,
            Cf: "write".to_string(),
            ..Default::default()
        }];
        // 仅 write CF：统计直接写在 write 文件上（非 TiKV 双 CF 形态）。
        if num == 1 {
            return files;
        }

        // 对齐 TiKV：write CF 统计置零，kv/bytes 记在 default CF。
        // Match TiKV: write CF carries zero stats; default CF holds kv/bytes.
        files[0].TotalKvs = 0;
        files[0].TotalBytes = 0;
        // default CF 复用同一行键区间，与 write 成对出现。
        files.push(backuppb::File {
            Name: format!("{}_default.sst", rand_name()),
            StartKey: encode_row_key(self.table_id, &low),
            EndKey: encode_row_key(self.table_id, &high),
            TotalKvs: kv as u64,
            TotalBytes: bytes as u64,
            Cf: "default".to_string(),
            ..Default::default()
        });
        files
    }
}

/// 行记录键：表前缀 + 已编码 handle。
fn encode_row_key(table_id: i64, encoded_handle: &[u8]) -> Vec<u8> {
    let mut key = tablecodec::GenTableRecordPrefix(table_id);
    key.extend_from_slice(encoded_handle);
    key
}

/// 索引 seek 键：表+索引前缀 + 已编码索引列值。
fn encode_index_seek_key(table_id: i64, index_id: i64, encoded_values: &[u8]) -> Vec<u8> {
    let mut key = tablecodec::EncodeTableIndexPrefix(table_id, index_id);
    key.extend_from_slice(encoded_values);
    key
}

/// 确定性递增文件名后缀，避免测试间碰撞且结果可复现。
fn rand_name() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(1);
    N.fetch_add(1, Ordering::Relaxed)
}

/// 单条合并用例：files 为 build 参数五元组，merged 为各结果区间文件数。
struct MergeCase {
    /// build 参数：[tableID, indexID, num, bytes, kv]。
    files: Vec<[i32; 5]>,
    /// 每个合并后区间应包含的文件个数列表。
    merged: Vec<usize>,
    /// 仅断言 TotalRegions / MergedRegions；其余字段用 Default。
    stat: MergeRangesStat,
}

/// 覆盖空备份、超大区间切分、跨表/跨索引不可合并、同表小区间合并等场景。
/// TestMergeRanges — empty backup, big ranges, cross-table, cross-index, same-table merge.
#[test]
fn test_merge_ranges() {
    // 转为 i32 仅方便写在 files 五元组里；调用时仍传 u64 常量。
    let split_size_bytes = DEFAULT_MERGE_REGION_SIZE_BYTES as i32;
    let split_key_count = DEFAULT_MERGE_REGION_KEY_COUNT as i32;
    // cases 顺序与 Go TestMergeRanges 表驱动用例一一对应。
    let cases = vec![
        // —— 边界：空输入 ——
        // 空输入：无 Region、无合并结果。
        MergeCase {
            files: vec![],
            merged: vec![],
            stat: MergeRangesStat {
                TotalRegions: 0,
                MergedRegions: 0,
                ..Default::default()
            },
        },
        // —— 字节阈值切分 ——
        // 首文件已达字节阈值：与后续小文件不可合并。
        MergeCase {
            files: vec![[1, 0, 1, split_size_bytes, 1], [1, 0, 1, 1, 1]],
            merged: vec![1, 1],
            stat: MergeRangesStat {
                TotalRegions: 2,
                MergedRegions: 2,
                ..Default::default()
            },
        },
        // 次文件达字节阈值：同样保持两个独立区间。
        MergeCase {
            files: vec![[1, 0, 1, 1, 1], [1, 0, 1, split_size_bytes, 1]],
            merged: vec![1, 1],
            stat: MergeRangesStat {
                TotalRegions: 2,
                MergedRegions: 2,
                ..Default::default()
            },
        },
        // —— 键数阈值切分 ——
        // 首文件达键数阈值：阻止与下一文件合并。
        MergeCase {
            files: vec![[1, 0, 1, 1, split_key_count], [1, 0, 1, 1, 1]],
            merged: vec![1, 1],
            stat: MergeRangesStat {
                TotalRegions: 2,
                MergedRegions: 2,
                ..Default::default()
            },
        },
        // 次文件达键数阈值。
        MergeCase {
            files: vec![[1, 0, 1, 1, 1], [1, 0, 1, 1, split_key_count]],
            merged: vec![1, 1],
            stat: MergeRangesStat {
                TotalRegions: 2,
                MergedRegions: 2,
                ..Default::default()
            },
        },
        // 三个极小同表文件：应合并为一个区间。
        MergeCase {
            files: vec![[1, 0, 1, 1, 1], [1, 0, 1, 1, 1], [1, 0, 1, 1, 1]],
            merged: vec![3],
            stat: MergeRangesStat {
                TotalRegions: 3,
                MergedRegions: 1,
                ..Default::default()
            },
        },
        // 前两个 1/3 可合并，第三个 1/2 再并会超阈值，故 [2,1]。
        MergeCase {
            files: vec![
                [1, 0, 1, split_size_bytes / 3, 1],
                [1, 0, 1, split_size_bytes / 3, 1],
                [1, 0, 1, split_size_bytes / 2, 1],
            ],
            merged: vec![2, 1],
            stat: MergeRangesStat {
                TotalRegions: 3,
                MergedRegions: 2,
                ..Default::default()
            },
        },
        // 四个文件：前两合并、后两合并 → [2,2]。
        MergeCase {
            files: vec![
                [1, 0, 1, split_size_bytes / 3, 1],
                [1, 0, 1, split_size_bytes / 3, 1],
                [1, 0, 1, split_size_bytes / 2, 1],
                [1, 0, 1, 1, 1],
            ],
            merged: vec![2, 2],
            stat: MergeRangesStat {
                TotalRegions: 4,
                MergedRegions: 2,
                ..Default::default()
            },
        },
        // 中间满阈值文件切开：期望 [2,1,2]。
        MergeCase {
            files: vec![
                [1, 0, 1, split_size_bytes / 3, 1],
                [1, 0, 1, split_size_bytes / 3, 1],
                [1, 0, 1, split_size_bytes, 1],
                [1, 0, 1, split_size_bytes / 2, 1],
                [1, 0, 1, 1, 1],
            ],
            merged: vec![2, 1, 2],
            stat: MergeRangesStat {
                TotalRegions: 5,
                MergedRegions: 3,
                ..Default::default()
            },
        },
        // —— 跨表 / 跨索引 ——
        // 跨表：不同 table_id 的区间不得合并。
        MergeCase {
            files: vec![[1, 0, 1, 1, 1], [2, 0, 1, 1, 1]],
            merged: vec![1, 1],
            stat: MergeRangesStat {
                TotalRegions: 2,
                MergedRegions: 2,
                ..Default::default()
            },
        },
        // 表1 单独；表2 两个小区间可合并。
        MergeCase {
            files: vec![
                [1, 0, 1, split_size_bytes / 3, 1],
                [2, 0, 1, split_size_bytes / 3, 1],
                [2, 0, 1, split_size_bytes / 2, 1],
            ],
            merged: vec![1, 2],
            stat: MergeRangesStat {
                TotalRegions: 3,
                MergedRegions: 2,
                ..Default::default()
            },
        },
        // 同表不同索引：索引前缀不同，不可合并。
        MergeCase {
            files: vec![[1, 1, 1, 1, 1], [1, 2, 1, 1, 1]],
            merged: vec![1, 1],
            stat: MergeRangesStat {
                TotalRegions: 2,
                MergedRegions: 2,
                ..Default::default()
            },
        },
        // 索引顺序对调：仍因不同 index_id 保持分离。
        MergeCase {
            files: vec![[1, 2, 1, 1, 1], [1, 1, 1, 1, 1]],
            merged: vec![1, 1],
            stat: MergeRangesStat {
                TotalRegions: 2,
                MergedRegions: 2,
                ..Default::default()
            },
        },
        // 行键 + 两索引：三者键空间互异，三区间。
        MergeCase {
            files: vec![[1, 0, 1, 1, 1], [2, 1, 1, 1, 1], [2, 2, 1, 1, 1]],
            merged: vec![1, 1, 1],
            stat: MergeRangesStat {
                TotalRegions: 3,
                MergedRegions: 3,
                ..Default::default()
            },
        },
        // 表2 两个行键小文件可合并；索引仍独立 → [1,1,2]。
        MergeCase {
            files: vec![
                [1, 0, 1, 1, 1],
                [2, 1, 1, 1, 1],
                [2, 0, 1, 1, 1],
                [2, 0, 1, 1, 1],
            ],
            merged: vec![1, 1, 2],
            stat: MergeRangesStat {
                TotalRegions: 4,
                MergedRegions: 3,
                ..Default::default()
            },
        },
        // 表2 两个同索引文件可合并 → [1,2,1]。
        MergeCase {
            files: vec![
                [1, 0, 1, 1, 1],
                [2, 1, 1, 1, 1],
                [2, 1, 1, 1, 1],
                [2, 0, 1, 1, 1],
            ],
            merged: vec![1, 2, 1],
            stat: MergeRangesStat {
                TotalRegions: 4,
                MergedRegions: 3,
                ..Default::default()
            },
        },
    ];

    // 逐 case 执行：失败时带 case 下标与原始五元组，便于定位。
    for (i, cs) in cases.iter().enumerate() {
        let mut files = Vec::new();
        let mut fb = FileBuilder::new();
        // 按五元组展开为真实 backuppb::File 列表。
        for f in &cs.files {
            files.extend(fb.build(f[0], f[1], f[2], f[3], f[4]));
        }
        // rewriteRules=None：仅测合并，不测键重写。
        let (rngs, stat) = MergeAndRewriteFileRanges(
            files,
            None,
            DEFAULT_MERGE_REGION_SIZE_BYTES,
            DEFAULT_MERGE_REGION_KEY_COUNT,
        )
        .unwrap_or_else(|e| panic!("case {i}: {e:?} files={:?}", cs.files));
        // 对齐 Go：先比统计再比各区间文件个数与键覆盖。
        assert_eq!(
            cs.stat.TotalRegions, stat.TotalRegions,
            "case {i} TotalRegions files={:?}",
            cs.files
        );
        assert_eq!(
            cs.stat.MergedRegions, stat.MergedRegions,
            "case {i} MergedRegions files={:?}",
            cs.files
        );
        assert_eq!(rngs.len(), cs.merged.len(), "case {i} range count");
        for (range_index, rg) in rngs.iter().enumerate() {
            assert_eq!(
                rg.Files.len(),
                cs.merged[range_index],
                "case {i} range {range_index} files={:?}",
                cs.files
            );
            // 合并后区间必须覆盖其内全部文件的起止键。
            // 键覆盖用字节序比较，与 rtree 区间半开语义一致。
            for file in &rg.Files {
                assert!(
                    rg.StartKey.as_slice() <= file.StartKey.as_slice(),
                    "case {i}: StartKey not covering file"
                );
                assert!(
                    rg.EndKey.as_slice() >= file.EndKey.as_slice(),
                    "case {i}: EndKey not covering file"
                );
            }
        }
    }
}

/// RawKV 仅有 default CF：去掉 write 后仍应成功合并为一个 Region。
/// TestMergeRawKVRanges — RawKV has no write CF; only default CF participates.
#[test]
fn test_merge_raw_kv_ranges() {
    // RawKV 路径：TotalRegions 取 defaultCF 计数，write 为 0 仍合法。
    let mut fb = FileBuilder::new();
    // num=2 先生成 write+default，再丢弃 write，模拟纯 RawKV 备份。
    let mut files = fb.build(1, 0, 2, 1, 1);
    files = files[1..].to_vec();
    let (_, stat) = MergeAndRewriteFileRanges(
        files,
        None,
        DEFAULT_MERGE_REGION_SIZE_BYTES,
        DEFAULT_MERGE_REGION_KEY_COUNT,
    )
    .expect("merge raw kv");
    // 单个 default CF 文件：合并前后都是 1 个 Region。
    assert_eq!(stat.TotalRegions, 1);
    assert_eq!(stat.MergedRegions, 1);
}

/// 非法 CF 名：应返回 ErrRestoreInvalidBackup，与 Go 错误类型对齐。
/// TestInvalidRanges — illegal CF name yields ErrRestoreInvalidBackup.
#[test]
fn test_invalid_ranges() {
    // 与 Go TestInvalidRanges 对齐：未知 CF 必须失败而非静默跳过。
    let mut fb = FileBuilder::new();
    let mut files = fb.build(1, 0, 1, 1, 1);
    // 同时改掉 Name/Cf，避免文件名回退匹配 write/default。
    files[0].Name = "invalid.sst".to_string();
    files[0].Cf = "invalid".to_string();
    let err = MergeAndRewriteFileRanges(
        files,
        None,
        DEFAULT_MERGE_REGION_SIZE_BYTES,
        DEFAULT_MERGE_REGION_KEY_COUNT,
    )
    .expect_err("invalid cf");
    // Equal 走错误类型身份比较，不依赖文案。
    assert!(
        ErrRestoreInvalidBackup.Equal(Some(&err)),
        "cause should be ErrRestoreInvalidBackup, got {err}"
    );
}

/// 压力路径公共体：构造 files_count 个小文件并执行一次合并。
fn run_benchmark_merge_ranges(files_count: usize) {
    // 同表连续小文件：合并算法热路径，规模由调用方指定。
    let mut files = Vec::new();
    let mut fb = FileBuilder::new();
    // 每个 build 产出 1 个 write 文件，总计 files_count 个区间候选。
    for _ in 0..files_count {
        files.extend(fb.build(1, 0, 1, 1, 1));
    }
    // 不关心返回值，只验证大规模输入下算法可完成。
    MergeAndRewriteFileRanges(
        files,
        None,
        DEFAULT_MERGE_REGION_SIZE_BYTES,
        DEFAULT_MERGE_REGION_KEY_COUNT,
    )
    .expect("benchmark merge");
}

/// Go BenchmarkMergeRanges100 的单次迭代单元测试形态。
/// BenchmarkMergeRanges100 — Go benchmark body as one-iteration unit test.
#[test]
fn benchmark_merge_ranges_100() {
    // 100 文件：冒烟级规模，确认合并路径不 panic。
    run_benchmark_merge_ranges(100);
}

/// Go BenchmarkMergeRanges1k 的单次迭代单元测试形态。
/// BenchmarkMergeRanges1k — Go benchmark body as one-iteration unit test.
#[test]
fn benchmark_merge_ranges_1k() {
    // 1k 文件：中等负载，覆盖区间树插入与合并循环。
    run_benchmark_merge_ranges(1000);
}

/// Go BenchmarkMergeRanges10k 的单次迭代单元测试形态。
/// BenchmarkMergeRanges10k — Go benchmark body as one-iteration unit test.
#[test]
fn benchmark_merge_ranges_10k() {
    // 10k 文件：对齐 Go 大基准规模的单次正确性检查。
    run_benchmark_merge_ranges(10_000);
}
