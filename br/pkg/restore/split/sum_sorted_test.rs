// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc. Licensed under Apache-2.0.
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

//! 对应 Go `br/pkg/restore/split/sum_sorted_test.go`。
//! 纯算法：验证重叠有值区间的 Merge/Traverse，不依赖 PD/TiKV。
//! 期望 Size 以 MB 为单位的整数序列；Display 字符串用 hex 编码起止键。
//! 首尾 0 来自全空间哨兵，Traverse 会一并吐出。

use crate::sum_sorted::{NewSplitHelper, Span, Value, Valued, join};

/// 测试辅助：用 ASCII 起止键构造 Valued。
/// 键用单字节字母，便于与 hex 展示 "61"/"66" 对照。
fn v(s: &str, e: &str, val: Value) -> Valued {
    Valued {
        Key: Span {
            StartKey: s.as_bytes().to_vec(),
            EndKey: e.as_bytes().to_vec(),
        },
        Value: val,
    }
}

/// 将“MB 数”换算为 Size/Number（与 Go 用例中 `mb` 一致）。
/// Size 与 Number 同取 b，方便断言同时命中两字段。
fn mb(b: u64) -> Value {
    Value {
        Size: b * 1024 * 1024,
        Number: b as i64,
    }
}

/// 拼出与 Valued::String 同形的期望串；hex 键如 "61"="a"。
/// 小数位固定两位，对齐 Display 的 `{:.2}`。
fn export_string(start_key: &str, end_key: &str, size: &str, number: i64) -> String {
    format!("([{start_key}, {end_key}), {size} MB, {number})")
}

/// 表驱动：逐次 Merge 后 Traverse 的 Value 序列须与 result 一致，
/// 且每次 Merge 前的 Display 与 strs[i] 对齐（锁定切分前展示格式）。
#[test]
fn test_sum_sorted() {
    // 每组：(待合并区间, Traverse 期望 MB 序列含哨兵 0, 各次 Merge 前 String)
    // 用例顺序与 Go 表驱动一致，便于对照失败索引。
    let cases: Vec<(Vec<Valued>, Vec<u64>, Vec<String>)> = vec![
        (
            // 跨段叠加：右端超出 [a,f)，产生多段均摊。
            // g 落在 f 右侧，右 trail 与中间段权重分离。
            vec![
                v("a", "f", mb(100)),
                v("a", "c", mb(200)),
                v("d", "g", mb(100)),
            ],
            // 含首尾哨兵 0；中间为切分后的 MB 序列。
            vec![0, 250, 25, 75, 50, 0],
            vec![
                export_string("61", "66", "100.00", 100),
                export_string("61", "63", "200.00", 200),
                export_string("64", "67", "100.00", 100),
            ],
        ),
        (
            // 右端恰好落在 f：与上组相比少一段 trail。
            // 验证右边界闭合时段数收缩。
            vec![
                v("a", "f", mb(100)),
                v("a", "c", mb(200)),
                v("d", "f", mb(100)),
            ],
            vec![0, 250, 25, 125, 0],
            vec![
                export_string("61", "66", "100.00", 100),
                export_string("61", "63", "200.00", 200),
                export_string("64", "66", "100.00", 100),
            ],
        ),
        (
            // 邻接 [a,c)+[c,f)：无内部空洞，段数更少。
            // 邻接合并后中间权重直接相加。
            vec![
                v("a", "f", mb(100)),
                v("a", "c", mb(200)),
                v("c", "f", mb(100)),
            ],
            vec![0, 250, 150, 0],
            vec![
                export_string("61", "66", "100.00", 100),
                export_string("61", "63", "200.00", 200),
                export_string("63", "66", "100.00", 100),
            ],
        ),
        (
            // 再嵌套 [da,db)：验证细粒度子区间切分。
            // "da"/"db" 的 hex 为 6461/6462。
            vec![
                v("a", "f", mb(100)),
                v("a", "c", mb(200)),
                v("c", "f", mb(100)),
                v("da", "db", mb(100)),
            ],
            vec![0, 250, 50, 150, 50, 0],
            vec![
                export_string("61", "66", "100.00", 100),
                export_string("61", "63", "200.00", 200),
                export_string("63", "66", "100.00", 100),
                export_string("6461", "6462", "100.00", 100),
            ],
        ),
        (
            // [cb,db) 与 [da,db) 部分重叠，权重继续细分。
            // "cb"=6362，覆盖到 db 前。
            vec![
                v("a", "f", mb(100)),
                v("a", "c", mb(200)),
                v("c", "f", mb(100)),
                v("da", "db", mb(100)),
                v("cb", "db", mb(100)),
            ],
            vec![0, 250, 25, 75, 200, 50, 0],
            vec![
                export_string("61", "66", "100.00", 100),
                export_string("61", "63", "200.00", 200),
                export_string("63", "66", "100.00", 100),
                export_string("6461", "6462", "100.00", 100),
                export_string("6362", "6462", "100.00", 100),
            ],
        ),
        (
            // [cb,f) 延伸到原区间右端。
            // 右边界对齐 f，末段权重 100。
            vec![
                v("a", "f", mb(100)),
                v("a", "c", mb(200)),
                v("c", "f", mb(100)),
                v("da", "db", mb(100)),
                v("cb", "f", mb(150)),
            ],
            vec![0, 250, 25, 75, 200, 100, 0],
            vec![
                export_string("61", "66", "100.00", 100),
                export_string("61", "63", "200.00", 200),
                export_string("63", "66", "100.00", 100),
                export_string("6461", "6462", "100.00", 100),
                export_string("6362", "66", "150.00", 150),
            ],
        ),
        (
            // [cb,df) 越过 db，产生右侧剩余 25MB 段。
            // "df"=6466，落在 f 之前。
            vec![
                v("a", "f", mb(100)),
                v("a", "c", mb(200)),
                v("c", "f", mb(100)),
                v("da", "db", mb(100)),
                v("cb", "df", mb(150)),
            ],
            vec![0, 250, 25, 75, 200, 75, 25, 0],
            vec![
                export_string("61", "66", "100.00", 100),
                export_string("61", "63", "200.00", 200),
                export_string("63", "66", "100.00", 100),
                export_string("6461", "6462", "100.00", 100),
                export_string("6362", "6466", "150.00", 150),
            ],
        ),
        (
            // 与上一组输入相同：锁定幂等期望（Go 重复用例）。
            // 防止回归时误删重复 case 导致覆盖缺口。
            vec![
                v("a", "f", mb(100)),
                v("a", "c", mb(200)),
                v("c", "f", mb(100)),
                v("da", "db", mb(100)),
                v("cb", "df", mb(150)),
            ],
            vec![0, 250, 25, 75, 200, 75, 25, 0],
            vec![
                export_string("61", "66", "100.00", 100),
                export_string("61", "63", "200.00", 200),
                export_string("63", "66", "100.00", 100),
                export_string("6461", "6462", "100.00", 100),
                export_string("6362", "6466", "150.00", 150),
            ],
        ),
        (
            // 从 c 起覆盖到 df：左边界对齐 c，均摊结果不同。
            // 与从 cb 起的用例对照左边界效应。
            vec![
                v("a", "f", mb(100)),
                v("a", "c", mb(200)),
                v("c", "f", mb(100)),
                v("da", "db", mb(100)),
                v("c", "df", mb(150)),
            ],
            vec![0, 250, 100, 200, 75, 25, 0],
            vec![
                export_string("61", "66", "100.00", 100),
                export_string("61", "63", "200.00", 200),
                export_string("63", "66", "100.00", 100),
                export_string("6461", "6462", "100.00", 100),
                export_string("63", "6466", "150.00", 150),
            ],
        ),
        (
            // 再次覆盖完整 [c,f)：与邻接合并场景对照。
            // 末次 Merge 权重 150，与前序叠加后中段为 200。
            vec![
                v("a", "f", mb(100)),
                v("a", "c", mb(200)),
                v("c", "f", mb(100)),
                v("da", "db", mb(100)),
                v("c", "f", mb(150)),
            ],
            vec![0, 250, 100, 200, 100, 0],
            vec![
                export_string("61", "66", "100.00", 100),
                export_string("61", "63", "200.00", 200),
                export_string("63", "66", "100.00", 100),
                export_string("6461", "6462", "100.00", 100),
                export_string("63", "66", "150.00", 150),
            ],
        ),
    ];

    for (values, result, strs) in cases {
        // 每组独立 Helper，避免用例间状态泄漏。
        let mut full = NewSplitHelper();
        for (i, val) in values.into_iter().enumerate() {
            // Merge 前锁定 Display；Merge 后由 Traverse 校验聚合。
            assert_eq!(val.String(), strs[i], "string case index {i}");
            full.Merge(val);
        }
        let mut i = 0;
        full.Traverse(|got| {
            // result[i] 以 MB 整数表达期望 Size/Number。
            assert_eq!(mb(result[i]), got.Value, "traverse index {i}");
            i += 1;
            // 始终继续，确保走完全部段。
            true
        });
        // 须遍历完所有期望段（含首尾哨兵零值）。
        assert_eq!(i, result.len(), "traverse count");
    }
}

#[test]
fn test_go_integer_overflow_semantics() {
    assert_eq!(
        join(
            Value {
                Size: u64::MAX,
                Number: i64::MAX,
            },
            Value { Size: 1, Number: 1 },
        ),
        Value {
            Size: 0,
            Number: i64::MIN,
        }
    );

    let mut helper = NewSplitHelper();
    helper.Merge(v(
        "a",
        "d",
        Value {
            Size: u64::MAX,
            Number: i64::MAX,
        },
    ));
    helper.Merge(v("b", "c", Value::default()));

    let mut values = Vec::new();
    helper.Traverse(|got| {
        values.push(got.Value);
        true
    });
    let go_adjusted = Value {
        Size: u64::MAX.wrapping_mul(2) / 3 / 2,
        Number: i64::MAX.wrapping_mul(2) / 3 / 2,
    };
    assert_eq!(
        values,
        vec![
            Value::default(),
            go_adjusted,
            go_adjusted,
            go_adjusted,
            Value::default(),
        ]
    );
}
