// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc. Licensed under Apache-2.0.

//! Span 工具函数，对齐 Go `utils.go`。
//! 提供重叠判定、带无穷语义的字节比较、区间折叠，以及 valued 集合等价比较。
//! 空 EndKey 在比较中视为 +∞（`a_inf`/`b_inf`），与 kv 半开区间约定一致。
//! 这些工具被 sorted / value_sorted 测试与推进器路径共用。

use crate::sorted::{Span, Valued};
use crate::value_sorted::ValueSortedFull;

/// 判断两 Span 是否在半开区间语义下重叠；空 EndKey 表示延伸到 +∞。
/// 有限区间使用标准 `start < other.end && other.start < end`。
pub fn Overlaps(a: &Span, b: &Span) -> bool {
    // 任一方右端为 ∞ 时，只要另一方右端越过对方 StartKey 即重叠。
    if b.EndKey.is_empty() {
        return a.EndKey.is_empty() || a.EndKey.as_slice() > b.StartKey.as_slice();
    }
    if a.EndKey.is_empty() {
        return b.EndKey.is_empty() || b.EndKey.as_slice() > a.StartKey.as_slice();
    }
    // 双方均有限：标准半开区间相交条件。
    a.StartKey.as_slice() < b.EndKey.as_slice() && b.StartKey.as_slice() < a.EndKey.as_slice()
}

/// 带“空且 inf 表示 ∞”的字典序比较；返回 -1/0/1，对应 Go `CompareBytesExt`。
pub fn CompareBytesExt(a: &[u8], a_inf: bool, b: &[u8], b_inf: bool) -> i32 {
    match (a.is_empty() && a_inf, b.is_empty() && b_inf) {
        (true, true) => 0,
        // ∞ 大于任意有限键。
        (true, false) => 1,
        (false, true) => -1,
        (false, false) => match a.cmp(b) {
            std::cmp::Ordering::Less => -1,
            std::cmp::Ordering::Equal => 0,
            std::cmp::Ordering::Greater => 1,
        },
    }
}

/// 对应 Go `Debug`：打印主键序 Traverse 与按值索引 `TraverseValuesLessThan` 两份视图。
pub fn Debug(full: &ValueSortedFull) {
    let mut result: Vec<Valued> = Vec::new();
    full.Traverse(|v| {
        result.push(v);
        true
    });
    let mut idx: Vec<Valued> = Vec::new();
    // u64::MAX 使按值遍历覆盖全部条目，便于对照索引是否与主树一致。
    full.TraverseValuesLessThan(u64::MAX, |v| {
        idx.push(v);
        true
    });
    println!("{:?}\n\tidx = {:?}", result, idx);
}

/// 将可能重叠/相邻的 Span 列表折叠为不相交覆盖；排序键为 StartKey，再比 EndKey（∞）。
pub fn Collapse(spans: &[Span]) -> Vec<Span> {
    let mut frs = spans.to_vec();
    frs.sort_by(|x, y| {
        let start = x.StartKey.cmp(&y.StartKey);
        if start != std::cmp::Ordering::Equal {
            return start;
        }
        // 同起点时较短区间排前，便于后续并集扩张右端。
        let c = CompareBytesExt(&x.EndKey, true, &y.EndKey, true);
        c.cmp(&0)
    });
    let mut result = Vec::new();
    let mut i = 0;
    while i < frs.len() {
        let mut item = frs[i].clone();
        loop {
            i += 1;
            // 下一段起点越过当前右端且当前非 ∞：无法再合并。
            if i >= frs.len()
                || (!item.EndKey.is_empty() && frs[i].StartKey.as_slice() > item.EndKey.as_slice())
            {
                break;
            }
            // 扩张右端：取更大 EndKey，或对方为 ∞。
            if (!item.EndKey.is_empty() && item.EndKey.as_slice() < frs[i].EndKey.as_slice())
                || frs[i].EndKey.is_empty()
            {
                item.EndKey = frs[i].EndKey.clone();
            }
        }
        result.push(item);
    }
    result
}

/// 全键空间占位：单个默认 Span（空起止）表示 `(-∞,+∞)`，对应 Go `Full()`。
pub fn Full() -> Vec<Span> {
    vec![Span::default()]
}

impl Valued {
    /// 严格相等：Value 与起止键均相同（不同于集合覆盖等价）。
    pub fn Equals(&self, y: &Valued) -> bool {
        self.Value == y.Value
            && self.Key.StartKey == y.Key.StartKey
            && self.Key.EndKey == y.Key.EndKey
    }
}

/// 对应 Go `ValuedSetEquals`。
/// 允许同值相邻区间切分不同，只要覆盖的键空间与取值一致。
pub fn ValuedSetEquals(mut xs: Vec<Valued>, mut ys: Vec<Valued>) -> bool {
    // 双方皆空才相等；一侧空另一侧非空立即失败。
    if xs.is_empty() || ys.is_empty() {
        return ys.len() == xs.len();
    }

    // 与 Collapse 相同排序：先 StartKey，再带 ∞ 语义的 EndKey。
    xs.sort_by(|a, b| {
        let start = a.Key.StartKey.cmp(&b.Key.StartKey);
        if start != std::cmp::Ordering::Equal {
            return start;
        }
        CompareBytesExt(&a.Key.EndKey, true, &b.Key.EndKey, true).cmp(&0)
    });
    ys.sort_by(|a, b| {
        let start = a.Key.StartKey.cmp(&b.Key.StartKey);
        if start != std::cmp::Ordering::Equal {
            return start;
        }
        CompareBytesExt(&a.Key.EndKey, true, &b.Key.EndKey, true).cmp(&0)
    });

    let mut xi = 0usize;
    let mut yi = 0usize;

    loop {
        // 任一侧耗尽时，另一侧也必须耗尽。
        if xi >= xs.len() || yi >= ys.len() {
            return (xi >= xs.len()) == (yi >= ys.len());
        }
        let x = &xs[xi];
        let y = &ys[yi];

        // 当前段起点必须对齐，否则覆盖不一致。
        if x.Key.StartKey != y.Key.StartKey {
            return false;
        }

        loop {
            if xi >= xs.len() || yi >= ys.len() {
                return (xi >= xs.len()) == (yi >= ys.len());
            }
            let x = xs[xi].clone();
            let y = ys[yi].clone();

            // 同覆盖段上取值必须相同。
            if x.Value != y.Value {
                return false;
            }

            let c = CompareBytesExt(&x.Key.EndKey, true, &y.Key.EndKey, true);
            if c == 0 {
                // 两端同时结束本段，推进两侧指针。
                xi += 1;
                yi += 1;
                break;
            }
            if c < 0 {
                // x 段更短：推进 x，且下一段必须紧邻（无空洞）。
                xi += 1;
                // If not adjacent key, return false directly.
                if xi < xs.len()
                    && CompareBytesExt(&x.Key.EndKey, true, &xs[xi].Key.StartKey, false) != 0
                {
                    return false;
                }
            }
            if c > 0 {
                // y 段更短：对称检查 y 侧邻接。
                yi += 1;
                if yi < ys.len()
                    && CompareBytesExt(&y.Key.EndKey, true, &ys[yi].Key.StartKey, false) != 0
                {
                    return false;
                }
            }
        }
    }
}
