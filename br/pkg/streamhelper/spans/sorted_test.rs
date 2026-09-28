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

//! 对应 Go `sorted_test.go`：验证有序 valued span 树的 Merge / Traverse 语义。
//! 覆盖全键空间与子区间两种初始化；断言用 `ValuedSetEquals` 容忍等价切分。
//! 本文件只解释测试意图与场景，不改变用例数据或断言行为。
//! Merge 语义与 Go 一致：重叠区取较大值，空洞保留初始化初值。
//! 空 EndKey / 空 StartKey 表示 ±∞，用例字符串键仅作可读占位。

use crate::{Full, NewFullWith, Span, Value, Valued, ValuedSetEquals};

/// 构造半开区间 `[a,b)` 的测试 Span；空串表示 ±∞，与 Go 本地 helper `s` 一致。
/// 供本文件与 `value_sorted_test` 共用，避免重复定义。
pub fn s(a: &str, b: &str) -> Span {
    Span {
        StartKey: a.as_bytes().to_vec(),
        EndKey: b.as_bytes().to_vec(),
    }
}

/// 将 Span 与检查点 Value 打包为 `Valued`，对应 Go helper `kv`。
/// Value 在日志备份场景表示 region/区间检查点 TS。
pub fn kv(span: Span, v: Value) -> Valued {
    Valued {
        Key: span,
        Value: v,
    }
}

/// 对应 Go `TestBasic`：从全键空间（`Full()`，初值 0）顺序 Merge，核对最终覆盖。
/// 每步 Merge 后打印中间态，最终用集合等价断言。
#[test]
fn test_basic() {
    // 用例：输入 Merge 序列与期望的最终 valued 覆盖。
    // input_sequence 按时间顺序；result 为全部 Merge 完成后的 Traverse 快照。
    struct Case {
        input_sequence: Vec<Valued>,
        result: Vec<Valued>,
    }

    let run = |c: Case| {
        // 全键空间初值 0：未写入区间保持 0，与 Go NewFullWith(Full(), 0) 对齐。
        let mut full = NewFullWith(&Full(), 0);
        println!("test_basic");
        for i in &c.input_sequence {
            // 逐步 Merge，并打印中间 Traverse 结果便于对照 Go 调试输出。
            // 回调恒返回 true，表示遍历不提前中止。
            full.Merge(i.clone());
            let mut result: Vec<Valued> = Vec::new();
            full.Traverse(|v| {
                result.push(v);
                true
            });
            println!("{:?} -> {:?}", i, result);
        }

        // 最终遍历：用集合等价比较，避免因相邻同值切分不同而误失败。
        // 不要求分段边界字节级一致，只要求覆盖与取值一致。
        let mut result: Vec<Valued> = Vec::new();
        full.Traverse(|v| {
            result.push(v);
            true
        });

        assert!(
            ValuedSetEquals(result.clone(), c.result.clone()),
            "{:?}\nvs\n{:?}",
            result,
            c.result
        );
    };

    let cases = vec![
        // 相邻不重叠：两端保留初值 0 的空隙区间。
        // 验证最简单的拼接与空隙保留。
        Case {
            input_sequence: vec![kv(s("0001", "0002"), 1), kv(s("0002", "0003"), 2)],
            result: vec![
                kv(s("", "0001"), 0),
                kv(s("0001", "0002"), 1),
                kv(s("0002", "0003"), 2),
                kv(s("0003", ""), 0),
            ],
        },
        // 后写覆盖前写：`[0001,0003)=4` 合并两段，消灭中间边界。
        Case {
            input_sequence: vec![
                kv(s("0001", "0002"), 1),
                kv(s("0002", "0003"), 2),
                kv(s("0001", "0003"), 4),
            ],
            result: vec![
                kv(s("", "0001"), 0),
                kv(s("0001", "0003"), 4),
                kv(s("0003", ""), 0),
            ],
        },
        // 重叠与左端延伸：较大值在重叠区胜出；左端 `[ ,0002)=2` 抬高前缀。
        Case {
            input_sequence: vec![
                kv(s("0001", "0004"), 3),
                kv(s("0004", "0008"), 5),
                kv(s("0001", "0007"), 4),
                kv(s("", "0002"), 2),
            ],
            result: vec![
                kv(s("", "0001"), 2),
                kv(s("0001", "0004"), 4),
                kv(s("0004", "0008"), 5),
                kv(s("0008", ""), 0),
            ],
        },
        // 右侧延伸超出已有右端：`[0008,0009)` 取覆盖值 4，其后仍为 0。
        Case {
            input_sequence: vec![
                kv(s("0001", "0004"), 3),
                kv(s("0004", "0008"), 5),
                kv(s("0001", "0009"), 4),
            ],
            result: vec![
                kv(s("", "0001"), 0),
                kv(s("0001", "0004"), 4),
                kv(s("0004", "0008"), 5),
                kv(s("0008", "0009"), 4),
                kv(s("0009", ""), 0),
            ],
        },
    ];

    for (i, c) in cases.into_iter().enumerate() {
        // Go 子测试名：fmt.Sprintf("#%d", i+1)
        println!("#{}", i + 1);
        run(c);
    }
}

/// 对应 Go `TestSubRange`：用非连续子区间初始化，Merge 只作用于这些窗口。
/// 用于模拟日志备份只关心部分表/键空间时的 span 推进。
#[test]
fn test_sub_range() {
    // range 限定可写窗口；窗外键空间不在树中，与 Go NewFullWith(range, 0) 一致。
    // 多段 range 之间的空洞永远不会出现在 Traverse 结果里。
    struct Case {
        range: Vec<Span>,
        input_sequence: Vec<Valued>,
        result: Vec<Valued>,
    }

    let run = |c: Case| {
        // 初值仍为 0，但树节点只覆盖 `c.range` 并集。
        let mut full = NewFullWith(&c.range, 0);
        println!("test_sub_range");
        for i in &c.input_sequence {
            // 输入可跨越窗口外；实现按窗口裁剪后写入。
            // 裁剪后若无交集则该次 Merge 对树无可见影响。
            full.Merge(i.clone());
            let mut result: Vec<Valued> = Vec::new();
            full.Traverse(|v| {
                result.push(v);
                true
            });
            println!("{:?} -> {:?}", i, result);
        }

        // 断言仅覆盖初始化 range 内的最终分段。
        // 与 TestBasic 相同，使用 ValuedSetEquals 做覆盖等价。
        let mut result: Vec<Valued> = Vec::new();
        full.Traverse(|v| {
            result.push(v);
            true
        });

        assert!(
            ValuedSetEquals(result.clone(), c.result.clone()),
            "{:?}\nvs\n{:?}",
            result,
            c.result
        );
    };

    let cases = vec![
        // 双窗口 `[0001,0004)` 与 `[0008,)`：跨窗写入被裁到各窗内最高值。
        // 中间空洞 `[0004,0008)` 不在树中，不会被填充。
        Case {
            range: vec![s("0001", "0004"), s("0008", "")],
            input_sequence: vec![
                kv(s("0001", "0007"), 42),
                kv(s("0000", "0009"), 41),
                kv(s("0002", "0005"), 43),
            ],
            result: vec![
                kv(s("0001", "0002"), 42),
                kv(s("0002", "0004"), 43),
                kv(s("0008", "0009"), 41),
                kv(s("0009", ""), 0),
            ],
        },
        // 全键写入 `("", "")`：两个窗口整体被同一值填满。
        Case {
            range: vec![s("0001", "0004"), s("0008", "")],
            input_sequence: vec![kv(s("", ""), 42)],
            result: vec![kv(s("0001", "0004"), 42), kv(s("0008", ""), 42)],
        },
        // 中间空洞 `[0004,0005)` 不在 range：写入不会在空洞落段。
        Case {
            range: vec![s("0001", "0004"), s("0005", "0008")],
            input_sequence: vec![
                kv(s("0001", "0002"), 42),
                kv(s("0002", "0008"), 43),
                kv(s("0004", "0007"), 45),
                kv(s("0000", "00015"), 48),
            ],
            result: vec![
                kv(s("0001", "00015"), 48),
                kv(s("00015", "0002"), 42),
                kv(s("0002", "0004"), 43),
                kv(s("0005", "0007"), 45),
                kv(s("0007", "0008"), 43),
            ],
        },
        // 空洞两侧互不污染：落在空洞的写入不影响左侧未触达区间的初值。
        Case {
            range: vec![s("0001", "0004"), s("0005", "0008")],
            input_sequence: vec![
                kv(s("0004", "0008"), 32),
                kv(s("00041", "0007"), 33),
                kv(s("0004", "00041"), 99999),
                kv(s("0005", "0006"), 34),
            ],
            result: vec![
                kv(s("0001", "0004"), 0),
                kv(s("0005", "0006"), 34),
                kv(s("0006", "0007"), 33),
                kv(s("0007", "0008"), 32),
            ],
        },
    ];

    for (i, c) in cases.into_iter().enumerate() {
        // 与 Go 子测试编号一致，便于对照失败日志。
        println!("#{}", i + 1);
        run(c);
    }
}
