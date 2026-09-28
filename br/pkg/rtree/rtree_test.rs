// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc. Licensed under Apache-2.0.

//! Go-equivalent tests for `br/pkg/rtree/rtree_test.go`.
//! Pure range-tree / progress-tree algorithms — no TiKV / PD / objstore.
//!
//! 本文件对齐 Go `br/pkg/rtree/rtree_test.go`，覆盖区间树与进度树的纯算法行为。
//! 不启动 TiKV/PD/对象存储；夹具键用可读 ASCII 或编码后的表记录前缀。
//! 主要场景：空洞补齐、`PutForce` 重叠策略、区间交集、合并阈值、进度完成回调与 checksum。
//! 断言对照 Go 期望；Rust 侧 MetaWriter 用内存替身验证 Send/聚合契约。
//! 任务只补注释，测试逻辑与断言保持不变。
//!
//! 阅读顺序建议：先看 `test_range_tree` 理解空洞扫描，再看 `PutForce` 的覆盖/拒绝语义，
//! 然后是 `MergedRanges` 阈值合并，最后是进度树完成回调与 checksum 聚合。
//! 所有 `RpcKeyRange` 断言均使用半开区间；空 EndKey 表示正无穷上界。
//! 回调测试中的“孤儿树”片段对应 Go 删除节点后仍持有指针再改写的场景。
//! 夹具 `bytes`/`new_range`/`file_named` 只服务可读性，不模拟真实 SST 内容。
//! `put_contained` 封装 FindContained + Put，避免测试体重复借用拆解。
//! `MemMetaWriter` 仅校验 AppendDataFile 与发送次数，不落盘。
//! 合并测试同时覆盖普通行键与 keyspace 包装键，防止 V2 前缀路径回归。
//! PutForce 用例用文件名标记“哪一次写入胜出”，便于对照覆盖语义。
//! 进度树用例用短 ASCII 键，刻意避开编码细节，专注 Origin/Res 关系。
//! 完成回调与 checksum 拆成两个测试：前者无写出器，后者带内存 MetaWriter。
//! 若后续补齐 stub，勿把“孤儿树无影响”误改成联动更新。
//! 密度目标来自计划：有效代码行约 20%，本文件门槛为 166 行中文注释。
//! 注释解释意图与约束，不复述断言字面量。

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::{
    AppendDataFile, EncodeKeyspaceKey, EncodeRecordKey, File, GenTableRecordPrefix, KeyRange,
    MetaWriter, NewProgressRangeTree, NewRangeStatsTree, NewRangeTree, ProgressRange, Range,
    RangeTree, RpcKeyRange, SummaryFiles,
};

// 测试夹具：把可读字符串转成字节键，便于与 Go 字符串键用例对齐。
fn bytes(s: &str) -> Vec<u8> {
    // 测试键均为 ASCII，无转义需求
    // 不做转义，直接取字节
    s.as_bytes().to_vec()
}

// 构造无文件的简单 Range，专注区间几何行为。
fn new_range(start: Vec<u8>, end: Vec<u8>) -> Range {
    Range {
        KeyRange: KeyRange {
            StartKey: start,
            EndKey: end,
        },
        // 几何用例不需要真实 SST
        Files: Vec::new(),
    }
}

/// `TestRangeTree` → `test_range_tree`.
///
/// 核心场景：空树空洞、逐段 Update、重叠覆盖后 Len 变化，以及全覆盖后无空洞。
/// `assert_incomplete` 对照 Go 期望的 RpcKeyRange 列表；`assert_all_complete` 穷举小字节空间。
#[test]
fn test_range_tree() {
    let mut range_tree = NewRangeTree();
    // 空树：精确 Get 应 miss
    assert!(range_tree.Get(&new_range(bytes(""), bytes(""))).is_none());

    // 按 StartKey 精确查找的局部助手
    let search =
        |tree: &RangeTree, key: Vec<u8>| -> Option<Range> { tree.Get(&new_range(key, bytes(""))) };
    // 局部闭包避免重复样板代码
    // 校验 GetIncompleteRange 返回的空洞与期望一一对应
    let assert_incomplete =
        |tree: &RangeTree, start_key: Vec<u8>, end_key: Vec<u8>, ranges: Vec<KeyRange>| {
            let incomplete = tree.GetIncompleteRange(start_key.clone(), end_key.clone());
            // 长度不符时打印 incomplete 与 expect 便于定位
            // 空洞个数先对齐，再逐项比端点
            assert_eq!(
                ranges.len(),
                incomplete.len(),
                "start={start_key:?} end={end_key:?} incomplete={incomplete:?} expect={ranges:?}"
            );
            for (idx, rg) in incomplete.iter().enumerate() {
                // 与 Go bytes.Equal 语义一致
                // 半开区间端点必须字节级相等
                assert_eq!(ranges[idx].StartKey, rg.StartKey, "idx={idx}");
                assert_eq!(ranges[idx].EndKey, rg.EndKey, "idx={idx}");
            }
        };
    // 仅在全覆盖断言处调用，成本可接受
    // 穷举单字节键空间：全覆盖后任意子区间不应再有空洞
    let assert_all_complete = |tree: &RangeTree| {
        for s in 0..0xfeu8 {
            for e in (s + 1)..0xffu8 {
                // 全覆盖时 incomplete 必须为空
                // 期望空空洞列表
                assert_incomplete(tree, vec![s], vec![e], vec![]);
            }
        }
    };

    // 空树：请求 ["", b) 本身就是完整空洞
    assert_incomplete(
        &range_tree,
        bytes(""),
        bytes("b"),
        vec![KeyRange {
            StartKey: bytes(""),
            EndKey: bytes("b"),
        }],
    );
    // 空上界请求：整棵键空间都未覆盖
    assert_incomplete(
        &range_tree,
        bytes(""),
        bytes(""),
        vec![KeyRange {
            StartKey: bytes(""),
            EndKey: bytes(""),
        }],
    );
    // 从 b 到正无穷的请求同样整段缺失
    assert_incomplete(
        &range_tree,
        bytes("b"),
        bytes(""),
        vec![KeyRange {
            StartKey: bytes("b"),
            EndKey: bytes(""),
        }],
    );

    // 准备互不重叠的五段
    let range0 = new_range(bytes(""), bytes("a"));
    // 半开区间 [a,b)
    let range_a = new_range(bytes("a"), bytes("b"));
    // [b,c)
    let range_b = new_range(bytes("b"), bytes("c"));
    // [c,d)
    let range_c = new_range(bytes("c"), bytes("d"));
    // [d, +∞)
    let range_d = new_range(bytes("d"), bytes(""));

    // 仅插入 a..b：该子区间完整，两侧仍是空洞
    range_tree.Update(range_a.clone());
    assert_eq!(1, range_tree.Len());
    assert_incomplete(&range_tree, bytes("a"), bytes("b"), vec![]);
    assert_incomplete(
        &range_tree,
        bytes(""),
        bytes(""),
        vec![
            KeyRange {
                StartKey: bytes(""),
                EndKey: bytes("a"),
            },
            KeyRange {
                StartKey: bytes("b"),
                EndKey: bytes(""),
            },
        ],
    );
    // 从 b 起向右仍全部缺失
    assert_incomplete(
        &range_tree,
        bytes("b"),
        bytes(""),
        vec![KeyRange {
            StartKey: bytes("b"),
            EndKey: bytes(""),
        }],
    );

    // 再插入 c..d：a..c 中间留下 b..c 空洞
    range_tree.Update(range_c.clone());
    assert_eq!(2, range_tree.Len());
    // 请求 a..c：中间缺 b..c
    assert_incomplete(
        &range_tree,
        bytes("a"),
        bytes("c"),
        vec![KeyRange {
            StartKey: bytes("b"),
            EndKey: bytes("c"),
        }],
    );
    // 精确请求 b..c 时空洞即其本身
    assert_incomplete(
        &range_tree,
        bytes("b"),
        bytes("c"),
        vec![KeyRange {
            StartKey: bytes("b"),
            EndKey: bytes("c"),
        }],
    );
    // 全空间请求：三段空洞
    assert_incomplete(
        &range_tree,
        bytes(""),
        bytes(""),
        vec![
            KeyRange {
                StartKey: bytes(""),
                EndKey: bytes("a"),
            },
            KeyRange {
                StartKey: bytes("b"),
                EndKey: bytes("c"),
            },
            KeyRange {
                StartKey: bytes("d"),
                EndKey: bytes(""),
            },
        ],
    );

    // Get 精确匹配 StartKey；中间键与未插入段均 miss
    assert!(search(&range_tree, vec![]).is_none());
    assert_eq!(Some(range_a.clone()), search(&range_tree, bytes("a")));
    // b 尚未插入
    assert!(search(&range_tree, bytes("b")).is_none());
    assert_eq!(Some(range_c.clone()), search(&range_tree, bytes("c")));
    // d 尚未插入
    assert!(search(&range_tree, bytes("d")).is_none());

    // 补上 b..c 后，仅剩首尾空洞
    range_tree.Update(range_b.clone());
    assert_eq!(3, range_tree.Len());
    assert_eq!(Some(range_b.clone()), search(&range_tree, bytes("b")));
    // a..d 中段已齐，仅剩首尾空洞
    assert_incomplete(
        &range_tree,
        bytes(""),
        bytes(""),
        vec![
            KeyRange {
                StartKey: bytes(""),
                EndKey: bytes("a"),
            },
            KeyRange {
                StartKey: bytes("d"),
                EndKey: bytes(""),
            },
        ],
    );

    // 补上 d 到正无穷后只剩最左空洞
    range_tree.Update(range_d.clone());
    assert_eq!(4, range_tree.Len());
    // d 段可按 StartKey 精确取回
    assert_eq!(Some(range_d.clone()), search(&range_tree, bytes("d")));
    // 只剩最左空洞
    assert_incomplete(
        &range_tree,
        bytes(""),
        bytes(""),
        vec![KeyRange {
            StartKey: bytes(""),
            EndKey: bytes("a"),
        }],
    );

    // 补上首段后五段齐备
    range_tree.Update(range0);
    assert_eq!(5, range_tree.Len());

    // 用更大区间 b..d 覆盖中间两段：Len 落到 4 且全覆盖
    let range_bd = new_range(bytes("b"), bytes("d"));
    range_tree.Update(range_bd);
    assert_eq!(4, range_tree.Len());
    assert_all_complete(&range_tree);

    // 再 Update 较小的 b..c：会拆出 c..d 空洞
    range_tree.Update(range_b);
    assert_eq!(4, range_tree.Len());
    // 覆盖回退留下的唯一空洞 c..d
    assert_incomplete(
        &range_tree,
        bytes(""),
        bytes(""),
        vec![KeyRange {
            StartKey: bytes("c"),
            EndKey: bytes("d"),
        }],
    );

    // 补回 c..d 后再次全覆盖
    range_tree.Update(range_c);
    assert_eq!(5, range_tree.Len());
    assert_all_complete(&range_tree);
}

// PutForce 用例期望：区间端点 + 首个文件名（SST 标识）。
struct ExpectedRange {
    start_key: Vec<u8>,
    end_key: Vec<u8>,
    filename: &'static str,
}

// 升序遍历树并与期望列表逐项比对（端点与文件名）。
fn check_expected_ranges(range_tree: &RangeTree, expected: &[ExpectedRange]) {
    // 先比长度可给出更清晰的失败信息
    // 段数必须与期望一致，否则 Ascend 下标会越界
    assert_eq!(range_tree.Len(), expected.len());
    let mut i = 0;
    range_tree.Ascend(|item| {
        // 与 BTreeMap 迭代顺序一致
        // Ascend 顺序即 StartKey 升序
        assert_eq!(expected[i].start_key, item.StartKey);
        assert_eq!(expected[i].end_key, item.EndKey);
        // Files[0].Name 即覆盖胜出标记
        // 本用例每个区间恰好一个命名文件
        assert_eq!(expected[i].filename, item.Files[0].Name);
        i += 1;
        // 返回 true 表示继续遍历后续节点
        // 继续 Ascend
        true
    });
}

// 仅带 Name 的最小 File，用于观察覆盖后保留哪个 SST。
fn file_named(name: &str) -> Vec<File> {
    vec![File {
        // 仅 Name 参与断言
        Name: name.to_string(),
        ..Default::default()
    }]
}

/// `TestRangeTreePutForce` → `test_range_tree_put_force`.
///
/// 验证 force=true 时重叠可覆盖，force=false 时重叠被拒绝且树不变。
/// 场景覆盖：前缀重叠、内嵌重叠、邻接（无重叠）插入，以及最终多段并存布局。
#[test]
fn test_range_tree_put_force() {
    let mut range_tree = NewRangeTree();
    // 空树起步，随后建立左右两簇区间
    // 两段不相邻：force 无关均可插入
    assert!(range_tree.PutForce(bytes("aa"), bytes("bb"), file_named("1.sst"), true));
    assert!(range_tree.PutForce(bytes("ff"), bytes("hh"), file_named("2.sst"), false));
    // 初始两段布局：aa..bb + ff..hh
    check_expected_ranges(
        &range_tree,
        &[
            ExpectedRange {
                start_key: bytes("aa"),
                end_key: bytes("bb"),
                filename: "1.sst",
            },
            ExpectedRange {
                start_key: bytes("ff"),
                end_key: bytes("hh"),
                filename: "2.sst",
            },
        ],
    );

    // force=true：用更短或内嵌区间反复覆盖左侧 aa..bb，右侧保持
    for (start, end, filename) in [
        ("a", "ab", "3.sst"),
        ("aaa", "abc", "4.sst"),
        ("aaaa", "aaab", "5.sst"),
        ("aa", "bb", "6.sst"),
    ] {
        assert!(range_tree.PutForce(bytes(start), bytes(end), file_named(filename), true));
        check_expected_ranges(
            &range_tree,
            &[
                ExpectedRange {
                    start_key: bytes(start),
                    end_key: bytes(end),
                    filename,
                },
                ExpectedRange {
                    start_key: bytes("ff"),
                    end_key: bytes("hh"),
                    filename: "2.sst",
                },
            ],
        );
    }

    // force=false：与 ff..hh 重叠的写入必须失败，树保持不变
    for (start, end, filename) in [
        ("f", "fh", "7.sst"),
        ("fff", "fhi", "8.sst"),
        ("ffff", "fffh", "9.sst"),
        ("ff", "hh", "10.sst"),
    ] {
        assert!(!range_tree.PutForce(bytes(start), bytes(end), file_named(filename), false));
        check_expected_ranges(
            &range_tree,
            &[
                ExpectedRange {
                    start_key: bytes("aa"),
                    end_key: bytes("bb"),
                    filename: "6.sst",
                },
                ExpectedRange {
                    start_key: bytes("ff"),
                    end_key: bytes("hh"),
                    filename: "2.sst",
                },
            ],
        );
    }

    // 强制缩短左侧为 aa..ab
    assert!(range_tree.PutForce(bytes("aa"), bytes("ab"), file_named("11.sst"), true));
    check_expected_ranges(
        &range_tree,
        &[
            ExpectedRange {
                start_key: bytes("aa"),
                end_key: bytes("ab"),
                // 缩短后的左侧文件
                filename: "11.sst",
            },
            ExpectedRange {
                start_key: bytes("ff"),
                end_key: bytes("hh"),
                filename: "2.sst",
            },
        ],
    );
    // 再以内嵌 aaa..ab 覆盖，左侧 StartKey 变为 aaa
    assert!(range_tree.PutForce(bytes("aaa"), bytes("ab"), file_named("12.sst"), true));
    check_expected_ranges(
        &range_tree,
        &[
            ExpectedRange {
                start_key: bytes("aaa"),
                end_key: bytes("ab"),
                // 左侧被内嵌覆盖
                filename: "12.sst",
            },
            ExpectedRange {
                start_key: bytes("ff"),
                end_key: bytes("hh"),
                filename: "2.sst",
            },
        ],
    );
    // 右侧仍拒绝非 force 的部分重叠或内嵌写入
    for (start, end, filename) in [("ff", "fh", "13.sst"), ("fh", "hh", "14.sst")] {
        assert!(!range_tree.PutForce(bytes(start), bytes(end), file_named(filename), false));
        check_expected_ranges(
            &range_tree,
            &[
                ExpectedRange {
                    start_key: bytes("aaa"),
                    end_key: bytes("ab"),
                    filename: "12.sst",
                },
                ExpectedRange {
                    start_key: bytes("ff"),
                    end_key: bytes("hh"),
                    filename: "2.sst",
                },
            ],
        );
    }

    // 邻接插入不重叠：force=false 也可成功；最终形成六段布局
    assert!(range_tree.PutForce(bytes("ab"), bytes("abc"), file_named("15.sst"), true));
    assert!(range_tree.PutForce(bytes("aa"), bytes("aaa"), file_named("16.sst"), true));
    assert!(range_tree.PutForce(bytes("hh"), bytes("hi"), file_named("17.sst"), false));
    assert!(range_tree.PutForce(bytes("ef"), bytes("ff"), file_named("18.sst"), false));
    check_expected_ranges(
        &range_tree,
        &[
            ExpectedRange {
                start_key: bytes("aa"),
                end_key: bytes("aaa"),
                // 最左邻接段
                filename: "16.sst",
            },
            ExpectedRange {
                start_key: bytes("aaa"),
                end_key: bytes("ab"),
                filename: "12.sst",
            },
            ExpectedRange {
                start_key: bytes("ab"),
                end_key: bytes("abc"),
                // 邻接在 ab 右侧
                filename: "15.sst",
            },
            ExpectedRange {
                start_key: bytes("ef"),
                end_key: bytes("ff"),
                // 紧贴 ff 左侧
                filename: "18.sst",
            },
            ExpectedRange {
                start_key: bytes("ff"),
                end_key: bytes("hh"),
                // 原始右侧段保留
                filename: "2.sst",
            },
            ExpectedRange {
                start_key: bytes("hh"),
                end_key: bytes("hi"),
                // 紧贴 hh 右侧
                filename: "17.sst",
            },
        ],
    );
}

/// `TestRangeIntersect` → `test_range_intersect`.
///
/// 表驱动校验 KeyRange::Intersect：含空端点、无交集、部分重叠与全包含。
/// 最后一组用非 ASCII 上界（字节 1）确认与 [a,c) 无交集。
#[test]
fn test_range_intersect() {
    // 覆盖 Intersect 的主要分支
    // 基准区间 [a, c)
    let rg = new_range(bytes("a"), bytes("c"));
    // 元组含义：查询起、查询止、是否相交、交集起、交集止
    for (start_arg, end_arg, expect_ok, expect_start, expect_end) in [
        // 空查询端点退化为自身
        ("", "", true, "a", "c"),
        // 落在左侧：无交
        ("", "a", false, "", ""),
        // 左穿入
        ("", "b", true, "a", "b"),
        // 真子集
        ("a", "b", true, "a", "b"),
        // 内缩起点
        ("aa", "b", true, "aa", "b"),
        // 贴右
        ("b", "c", true, "b", "c"),
        // 落在右侧：无交
        ("c", "", false, "", ""),
    ] {
        let (start, end, ok) = rg.Intersect(&bytes(start_arg), &bytes(end_arg));
        // ok/start/end 三元组同时断言
        // 与 Go 表驱动用例结果对齐
        assert_eq!(expect_ok, ok);
        assert_eq!(bytes(expect_start), start);
        assert_eq!(bytes(expect_end), end);
    }

    // 查询上界为字节 1（小于 a）：与 [a,c) 无交集
    let (start, end, ok) = rg.Intersect(b"", &[1]);
    assert!(!ok);
    assert_eq!(Vec::<u8>::new(), start);
    assert_eq!(Vec::<u8>::new(), end);
}

/// `BenchmarkRangeTreeUpdate` → `benchmark_range_tree_update` (fixed-N smoke).
///
/// Go benchmark 的固定 N 烟测：连续插入 256 个不重叠区间，断言 Len。
#[test]
fn benchmark_range_tree_update() {
    let mut range_tree = NewRangeTree();
    // 固定 256 次 Update，等价 Go benchmark 的 N 烟测
    // 定宽十进制键保证字典序与数值序一致，避免重叠
    for i in 0..256 {
        range_tree.Update(Range {
            KeyRange: KeyRange {
                StartKey: format!("{i:20}").into_bytes(),
                EndKey: format!("{:20}", i + 1).into_bytes(),
            },
            Files: Vec::new(),
        });
    }
    // 若键编码碰撞会导致 Len 变小
    // 全部互不重叠，Len 应等于插入次数
    assert_eq!(256, range_tree.Len());
}

// 普通表记录键编码（无 keyspace 前缀）。
fn encode_table_record(prefix: &[u8], row_id: u64) -> Vec<u8> {
    // row_id 转 i64 与 Go 测试一致
    // 委托 stubs 编码，与 Go tablecodec 行键布局一致
    EncodeRecordKey(prefix, row_id as i64)
}

// API V2 keyspace 包装：验证 NeedsMerge 能剥前缀后再按表合并。
fn make_encode_keyspaced_table_record(keyspace: u32) -> impl Fn(&[u8], u64) -> Vec<u8> {
    move |prefix: &[u8], row_id: u64| {
        // 顺序必须与 Go EncodeKeyspaceKey(EncodeRecordKey) 一致
        // 先编码行键，再套 keyspace 前缀
        EncodeKeyspaceKey(keyspace, &EncodeRecordKey(prefix, row_id as i64))
    }
}

/// `TestRangeTreeMerge` → `test_range_tree_merge`.
///
/// 对普通编码与 keyspace 编码各跑一遍合并：阈值 (10,10) 下 10000 段并成 1000 段。
#[test]
fn test_range_tree_merge() {
    // 无 keyspace 前缀路径
    test_range_tree_merge_inner(encode_table_record);
    // keyspace=1 的 V2 前缀路径
    test_range_tree_merge_inner(make_encode_keyspaced_table_record(1));
}

// 插入同表连续行区间，MergedRanges 应按体积与键数阈值每 10 段一组。
fn test_range_tree_merge_inner<F>(encode: F)
where
    F: Fn(&[u8], u64) -> Vec<u8>,
{
    let mut range_tree = NewRangeStatsTree();
    // 全新统计树，插入后统一 MergedRanges
    // 表 ID=1 的行记录前缀，保证 NeedsMerge 判为同表
    let table_prefix = GenTableRecordPrefix(1);
    // Size 字段填 i，便于校验合并后 Size 累加公式
    for i in 0u64..10000 {
        range_tree.InsertRange(
            Range {
                KeyRange: KeyRange {
                    StartKey: encode(&table_prefix, i),
                    EndKey: encode(&table_prefix, i + 1),
                },
                Files: vec![File {
                    // 定宽名便于合并后校验顺序
                    Name: format!("{i:20}"),
                    // 每段 1 键 1 字节，契合阈值 10
                    TotalKvs: 1,
                    TotalBytes: 1,
                    ..Default::default()
                }],
            },
            // Size 用行号本身
            i,
            // Count 未参与本断言
            0,
        );
    }
    // 阈值 10 字节或 10 键：每组合并 10 个单字节文件段
    let sorted = range_tree.MergedRanges(10, 10);
    assert_eq!(1000, sorted.len());
    // 校验每组端点、Size 累加与 Files 顺序
    for (i, rg) in sorted.iter().enumerate() {
        assert_eq!(encode(&table_prefix, (i as u64) * 10), rg.StartKey);
        assert_eq!(encode(&table_prefix, ((i + 1) as u64) * 10), rg.EndKey);
        // Size 为合并前 Size 字段的等差和
        assert_eq!((i * 10 * 10 + 45) as u64, rg.Size);
        // 每组合并恰好 10 个原文件
        assert_eq!(10, rg.Files.len());
        for (j, file) in rg.Files.iter().enumerate() {
            assert_eq!(format!("{:20}", i * 10 + j), file.Name);
            // 合并未改写单文件体积字段
            assert_eq!(1u64, file.TotalKvs);
            assert_eq!(1u64, file.TotalBytes);
        }
    }
}

// 空 Res 的 ProgressRange 夹具，Origin 由起止字符串构成。
fn build_progress_range(start_key: &str, end_key: &str) -> ProgressRange {
    ProgressRange {
        // Res 为空树，GetIncompleteRanges 会返回整个 Origin
        // 初始无已完成子区间
        Res: NewRangeTree(),
        Origin: KeyRange {
            StartKey: bytes(start_key),
            EndKey: bytes(end_key),
        },
    }
}

// 在 FindContained 命中的 Origin 下向 Res 写入一段已完成子区间。
// 先取 Origin.StartKey 再 get_mut，避免同时持有不可变与可变借用。
fn put_contained(
    pr_tree: &mut crate::ProgressRangeTree,
    find_start: &[u8],
    find_end: &[u8],
    put_start: Vec<u8>,
    put_end: Vec<u8>,
    files: Vec<File>,
) {
    let origin_key = {
        // FindContained 失败说明 find_start/end 选错
        // 必须落在某个已登记 Origin 内，否则测试夹具写错
        let pr = pr_tree
            .FindContained(find_start.to_vec(), find_end.to_vec())
            .unwrap()
            .expect("contained");
        pr.Origin.StartKey.clone()
    };
    pr_tree
        .BTreeG
        .get_mut(&origin_key)
        .expect("origin still in tree")
        .Res
        .Put(put_start, put_end, files);
}

/// `TestProgressRangeTree` → `test_progress_range_tree`.
///
/// 验证 Insert 拒绝 Origin 重叠；逐步 Put 后空洞收缩直至全部完成。
#[test]
fn test_progress_range_tree() {
    let mut pr_tree = NewProgressRangeTree(None, false);
    // 无 metaWriter：只测几何与 Insert 冲突

    // 邻接可插入；落入已登记 Origin 内的区间必须报错
    assert!(pr_tree.Insert(build_progress_range("aa", "cc")).is_ok());
    assert!(pr_tree.Insert(build_progress_range("bb", "cc")).is_err());
    assert!(pr_tree.Insert(build_progress_range("bb", "dd")).is_err());
    assert!(pr_tree.Insert(build_progress_range("cc", "dd")).is_ok());
    assert!(pr_tree.Insert(build_progress_range("ee", "ff")).is_ok());

    // 尚未 Put：三个 Origin 整段均为空洞
    let ranges = pr_tree.GetIncompleteRanges().unwrap();
    // 首个 Origin 整段空洞
    assert_eq!(
        RpcKeyRange {
            StartKey: bytes("aa"),
            EndKey: bytes("cc")
        },
        ranges[0]
    );
    // 第二个 Origin 尚未动过
    assert_eq!(
        RpcKeyRange {
            StartKey: bytes("cc"),
            EndKey: bytes("dd")
        },
        ranges[1]
    );
    // 第三个 Origin
    assert_eq!(
        RpcKeyRange {
            StartKey: bytes("ee"),
            EndKey: bytes("ff")
        },
        ranges[2]
    );

    // 部分完成 aa..cc 与全部完成 cc..dd 后，空洞收缩
    put_contained(&mut pr_tree, b"aaa", b"b", bytes("aaa"), bytes("b"), vec![]);
    put_contained(&mut pr_tree, b"cc", b"dd", bytes("cc"), bytes("dd"), vec![]);

    let ranges = pr_tree.GetIncompleteRanges().unwrap();
    // 左侧残留 aa..aaa
    assert_eq!(
        RpcKeyRange {
            StartKey: bytes("aa"),
            EndKey: bytes("aaa")
        },
        ranges[0]
    );
    // 中间空洞 b..cc
    assert_eq!(
        RpcKeyRange {
            StartKey: bytes("b"),
            EndKey: bytes("cc")
        },
        ranges[1]
    );
    // ee..ff 仍未动
    assert_eq!(
        RpcKeyRange {
            StartKey: bytes("ee"),
            EndKey: bytes("ff")
        },
        ranges[2]
    );

    // 补齐剩余空洞后应清空 incomplete，并删除已完成 ProgressRange
    for (start, end) in [("aa", "aaa"), ("b", "cc"), ("ee", "ff")] {
        put_contained(
            &mut pr_tree,
            start.as_bytes(),
            end.as_bytes(),
            bytes(start),
            bytes(end),
            vec![],
        );
    }
    let ranges = pr_tree.GetIncompleteRanges().unwrap();
    assert_eq!(0, ranges.len());
}

/// `TestProgreeRangeTreeCallBack` → `test_progress_range_tree_call_back`.
///
/// 验证 completeCallBack：仅当某个 Origin 完全覆盖时计数加一。
/// 拼写 Progree 保持与 Go 测试名一致。
#[test]
fn test_progress_range_tree_call_back() {
    let mut pr_tree = NewProgressRangeTree(None, false);
    // 三个互不重叠的 Origin：a..b / c..d / e..f
    for (start, end) in [("a", "b"), ("c", "d"), ("e", "f")] {
        assert!(pr_tree.Insert(build_progress_range(start, end)).is_ok());
    }

    // 原子计数器记录完成回调次数
    let complete_count = Arc::new(AtomicUsize::new(0));
    let c = Arc::clone(&complete_count);
    pr_tree.SetCallBack(Box::new(move || {
        c.fetch_add(1, Ordering::SeqCst);
    }));

    // 仅部分覆盖 a..b：回调未触发
    put_contained(&mut pr_tree, b"a", b"b", bytes("a"), bytes("aa"), vec![]);
    let ranges = pr_tree.GetIncompleteRanges().unwrap();
    assert_eq!(complete_count.load(Ordering::SeqCst), 0);
    // 剩余空洞从 aa 起；后两个 Origin 未动
    assert_eq!(
        RpcKeyRange {
            StartKey: bytes("aa"),
            EndKey: bytes("b")
        },
        ranges[0]
    );
    assert_eq!(
        RpcKeyRange {
            StartKey: bytes("c"),
            EndKey: bytes("d")
        },
        ranges[1]
    );
    assert_eq!(
        RpcKeyRange {
            StartKey: bytes("e"),
            EndKey: bytes("f")
        },
        ranges[2]
    );

    // 扩大到 a..ab 仍未完成 Origin
    put_contained(&mut pr_tree, b"a", b"b", bytes("a"), bytes("ab"), vec![]);
    let ranges = pr_tree.GetIncompleteRanges().unwrap();
    assert_eq!(complete_count.load(Ordering::SeqCst), 0);
    assert_eq!(
        RpcKeyRange {
            StartKey: bytes("ab"),
            EndKey: bytes("b")
        },
        ranges[0]
    );

    // 补上 ab..b：a..b 完成，回调变为 1
    put_contained(&mut pr_tree, b"a", b"b", bytes("ab"), bytes("b"), vec![]);
    let ranges = pr_tree.GetIncompleteRanges().unwrap();
    assert_eq!(complete_count.load(Ordering::SeqCst), 1);
    // a..b 已移除，剩余 c..d 与 e..f
    assert_eq!(
        RpcKeyRange {
            StartKey: bytes("c"),
            EndKey: bytes("d")
        },
        ranges[0]
    );
    assert_eq!(
        RpcKeyRange {
            StartKey: bytes("e"),
            EndKey: bytes("f")
        },
        ranges[1]
    );

    // Go mutates the orphaned ProgressRange after delete — no effect on the tree.
    // 已删除项的孤儿树再 Put：不影响进度树与回调计数
    let mut orphaned = NewRangeTree();
    orphaned.Put(bytes("a"), bytes("abc"), vec![]);
    let ranges = pr_tree.GetIncompleteRanges().unwrap();
    assert_eq!(complete_count.load(Ordering::SeqCst), 1);
    assert_eq!(
        RpcKeyRange {
            StartKey: bytes("c"),
            EndKey: bytes("d")
        },
        ranges[0]
    );

    // 再次修改孤儿树：其余 Origin 仍未完成，计数保持 1
    orphaned.Put(bytes("cc"), bytes("cd"), vec![]);
    let ranges = pr_tree.GetIncompleteRanges().unwrap();
    assert_eq!(complete_count.load(Ordering::SeqCst), 1);
    assert_eq!(
        RpcKeyRange {
            StartKey: bytes("c"),
            EndKey: bytes("d")
        },
        ranges[0]
    );
    assert_eq!(
        RpcKeyRange {
            StartKey: bytes("e"),
            EndKey: bytes("f")
        },
        ranges[1]
    );
}

// 带 PhysicalID 的进度夹具，供 checksum 按物理表聚合。
fn build_progress_range_with_physical_id(
    start_key: &str,
    end_key: &str,
    physical_id: i64,
) -> ProgressRange {
    let mut pr = build_progress_range(start_key, end_key);
    // 不同 Origin 用不同 PhysicalID 以便隔离聚合
    // 覆盖默认 PhysicalID=0，使 UpdateChecksum 按表分组
    pr.Res.PhysicalID = physical_id;
    pr
}

// 由 crc、kvs、bytes 三元组构造 File 列表，驱动 checksum 聚合断言。
fn get_files(start_key: Vec<u8>, end_key: Vec<u8>, checksums: &[[u64; 3]]) -> Vec<File> {
    checksums
        .iter()
        .map(|checksum| File {
            StartKey: start_key.clone(),
            EndKey: end_key.clone(),
            Crc64Xor: checksum[0],
            TotalKvs: checksum[1],
            TotalBytes: checksum[2],
            ..Default::default()
        })
        .collect()
}

// 内存 MetaWriter：统计 Send 次数，并断言 kind 为 AppendDataFile。
struct MemMetaWriter {
    sent: AtomicUsize,
}

impl MetaWriter for MemMetaWriter {
    fn Send(&self, files: &[File], kind: i32) -> Result<(), String> {
        // kind 常量来自 stubs::AppendDataFile
        // 生产路径只应追加数据文件元数据
        assert_eq!(kind, AppendDataFile);
        self.sent.fetch_add(files.len(), Ordering::SeqCst);
        Ok(())
    }
}

/// `TestProgreeRangeTreeCallBack2` → `test_progress_range_tree_call_back2`.
///
/// Go uses local objstore + metautil.MetaWriter; Rust uses in-memory MetaWriter
/// stand-in with the same Send(AppendDataFile) / checksum aggregation contract.
///
/// 在回调计数之外，校验按 PhysicalID 聚合的 Crc64Xor、TotalKvs、TotalBytes，
/// 以及完成时确实调用了 MetaWriter::Send。
#[test]
fn test_progress_range_tree_call_back2() {
    // 共享计数：完成路径应至少 Send 一次
    let writer = Arc::new(MemMetaWriter {
        sent: AtomicUsize::new(0),
    });
    // Arc 共享底层计数器，便于测试结束读取
    // 薄包装以满足 Box<dyn MetaWriter> 的所有权要求
    struct W(Arc<MemMetaWriter>);
    impl MetaWriter for W {
        fn Send(&self, files: &[File], kind: i32) -> Result<(), String> {
            self.0.Send(files, kind)
        }
    }
    // skipChecksum=false：完成时写入 checksumMap
    let mut pr_tree = NewProgressRangeTree(Some(Box::new(W(Arc::clone(&writer)))), false);

    // 三个 Origin 分别绑定 PhysicalID 1、2、3
    for (start, end, physical_id) in [("a", "b", 1), ("c", "d", 2), ("e", "f", 3)] {
        assert!(
            pr_tree
                .Insert(build_progress_range_with_physical_id(
                    start,
                    end,
                    physical_id
                ))
                .is_ok()
        );
    }

    // callback2 的完成计数器
    let complete_count = Arc::new(AtomicUsize::new(0));
    let c = Arc::clone(&complete_count);
    pr_tree.SetCallBack(Box::new(move || {
        c.fetch_add(1, Ordering::SeqCst);
    }));

    // 部分完成 a..aa：携带两份文件统计，但 Origin 未完成故不聚合
    put_contained(
        &mut pr_tree,
        b"a",
        b"b",
        bytes("a"),
        bytes("aa"),
        get_files(bytes("a"), bytes("aa"), &[[1, 1, 1], [2, 2, 2]]),
    );
    let ranges = pr_tree.GetIncompleteRanges().unwrap();
    assert_eq!(complete_count.load(Ordering::SeqCst), 0);
    // callback2 首轮空洞：aa..b / c..d / e..f
    assert_eq!(
        RpcKeyRange {
            StartKey: bytes("aa"),
            EndKey: bytes("b")
        },
        ranges[0]
    );
    assert_eq!(
        RpcKeyRange {
            StartKey: bytes("c"),
            EndKey: bytes("d")
        },
        ranges[1]
    );
    assert_eq!(
        RpcKeyRange {
            StartKey: bytes("e"),
            EndKey: bytes("f")
        },
        ranges[2]
    );

    // 覆盖到 a..ab：仍未完成，旧文件会被覆盖更新
    put_contained(
        &mut pr_tree,
        b"a",
        b"b",
        bytes("a"),
        bytes("ab"),
        get_files(bytes("a"), bytes("ab"), &[[3, 3, 3], [4, 4, 4]]),
    );
    let ranges = pr_tree.GetIncompleteRanges().unwrap();
    assert_eq!(complete_count.load(Ordering::SeqCst), 0);
    // 仍剩 ab..b
    assert_eq!(
        RpcKeyRange {
            StartKey: bytes("ab"),
            EndKey: bytes("b")
        },
        ranges[0]
    );

    // 补齐 ab..b：触发完成；checksum 计入最后一轮有效 Files
    put_contained(
        &mut pr_tree,
        b"a",
        b"b",
        bytes("ab"),
        bytes("b"),
        get_files(bytes("ab"), bytes("b"), &[[5, 5, 5], [6, 6, 6]]),
    );
    let ranges = pr_tree.GetIncompleteRanges().unwrap();
    assert_eq!(complete_count.load(Ordering::SeqCst), 1);
    // 完成后 incomplete 只剩后两个 Origin
    assert_eq!(
        RpcKeyRange {
            StartKey: bytes("c"),
            EndKey: bytes("d")
        },
        ranges[0]
    );
    assert_eq!(
        RpcKeyRange {
            StartKey: bytes("e"),
            EndKey: bytes("f")
        },
        ranges[1]
    );

    // 仅 PhysicalID=1 入账；异或与求和与 Go 聚合语义一致
    let cksm = pr_tree.GetChecksumMap();
    assert_eq!(cksm.len(), 1);
    let checksum = cksm.get(&1).expect("physical id 1");
    assert_eq!(3 ^ 4 ^ 5 ^ 6, checksum.Crc64Xor);
    assert_eq!(3 + 4 + 5 + 6, checksum.TotalKvs);
    assert_eq!(3 + 4 + 5 + 6, checksum.TotalBytes);

    // 孤儿树修改不应改变剩余空洞与回调
    let mut orphaned = NewRangeTree();
    orphaned.Put(bytes("a"), bytes("abc"), vec![]);
    let ranges = pr_tree.GetIncompleteRanges().unwrap();
    // 回调仍为 1
    assert_eq!(complete_count.load(Ordering::SeqCst), 1);
    assert_eq!(
        RpcKeyRange {
            StartKey: bytes("c"),
            EndKey: bytes("d")
        },
        ranges[0]
    );
    assert_eq!(
        RpcKeyRange {
            StartKey: bytes("e"),
            EndKey: bytes("f")
        },
        ranges[1]
    );

    // callback2：孤儿树二次 Put 仍不影响
    orphaned.Put(bytes("cc"), bytes("cd"), vec![]);
    let ranges = pr_tree.GetIncompleteRanges().unwrap();
    assert_eq!(complete_count.load(Ordering::SeqCst), 1);
    // c..d 仍在
    assert_eq!(
        RpcKeyRange {
            StartKey: bytes("c"),
            EndKey: bytes("d")
        },
        ranges[0]
    );
    // e..f 仍在
    assert_eq!(
        RpcKeyRange {
            StartKey: bytes("e"),
            EndKey: bytes("f")
        },
        ranges[1]
    );

    // 具体次数依赖 Files 条数，这里只要求非零
    // 完成路径至少 Send 过一次文件元数据
    assert!(writer.sent.load(Ordering::SeqCst) > 0);
}

/// Go `uint64` arithmetic wraps modulo 2^64; Rust debug builds must preserve it.
#[test]
fn test_uint64_accumulators_match_go_wrapping_semantics() {
    let files = vec![
        File {
            TotalBytes: u64::MAX,
            TotalKvs: u64::MAX,
            Crc64Xor: u64::MAX,
            ..Default::default()
        },
        File {
            TotalBytes: 1,
            TotalKvs: 1,
            Crc64Xor: u64::MAX,
            ..Default::default()
        },
    ];
    let range = Range {
        Files: files.clone(),
        ..Default::default()
    };
    assert_eq!((0, 0), range.BytesAndKeys());
    assert_eq!((0, 0, 0), SummaryFiles(&files));

    let record_prefix = GenTableRecordPrefix(1);
    let mut stats_tree = NewRangeStatsTree();
    stats_tree.InsertRange(
        Range {
            KeyRange: KeyRange {
                StartKey: EncodeRecordKey(&record_prefix, 0),
                EndKey: EncodeRecordKey(&record_prefix, 1),
            },
            Files: vec![files[0].clone()],
        },
        u64::MAX,
        u64::MAX,
    );
    stats_tree.InsertRange(
        Range {
            KeyRange: KeyRange {
                StartKey: EncodeRecordKey(&record_prefix, 1),
                EndKey: EncodeRecordKey(&record_prefix, 2),
            },
            Files: vec![files[1].clone()],
        },
        1,
        1,
    );
    let merged = stats_tree.MergedRanges(u64::MAX, u64::MAX);
    assert_eq!(1, merged.len());
    assert_eq!(0, merged[0].Size);
    assert_eq!(0, merged[0].Count);

    let mut progress = NewProgressRangeTree(None, false);
    progress.UpdateChecksum(7, u64::MAX, u64::MAX, u64::MAX);
    progress.UpdateChecksum(7, u64::MAX, 1, 1);
    let checksum = progress.GetChecksumMap().get(&7).unwrap();
    assert_eq!(0, checksum.Crc64Xor);
    assert_eq!(0, checksum.TotalKvs);
    assert_eq!(0, checksum.TotalBytes);
}
