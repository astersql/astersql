// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc. Licensed under Apache-2.0.

//! 按 Value 建二次索引的 span 集合，对齐 Go `value_sorted.go`。
//! 主结构仍是 `ValuedFull`（键序）；`valueIdx` 以 `(Value, StartKey)` 排序，
//! 供推进器按“检查点更小优先”扫描落后区间。

use std::collections::BTreeMap;

use crate::sorted::{NewFullWith, Value, Valued, ValuedFull};
use crate::utils::Full;

/// 键序主树 + 值序索引；Merge 时同步维护二者，避免脏索引。
pub struct ValueSortedFull {
    full: ValuedFull,
    // (value, startKey) -> Valued；同值按 StartKey 稳定排序。
    valueIdx: BTreeMap<(Value, Vec<u8>), Valued>,
}

/// 由已有 `ValuedFull` 构建：遍历一次填入 valueIdx，对应 Go `Sorted`。
pub fn Sorted(f: ValuedFull) -> ValueSortedFull {
    let mut vf = ValueSortedFull {
        full: f,
        valueIdx: BTreeMap::new(),
    };
    vf.full.Traverse(|v| {
        vf.valueIdx.insert((v.Value, v.Key.StartKey.clone()), v);
        true
    });
    vf
}

impl ValueSortedFull {
    /// 单条 Merge，委托 `MergeAll`。
    pub fn Merge(&mut self, newItem: Valued) {
        self.MergeAll(vec![newItem]);
    }

    /// 批量 Merge：先摘除重叠旧索引，主树 merge 后再插入新段索引。
    pub fn MergeAll(&mut self, newItems: Vec<Valued>) {
        for item in newItems {
            let mut overlapped = Vec::new();
            let mut inserted = Vec::new();
            self.full.overlapped(&item.Key, &mut overlapped);
            // rebuild via temporary full merge capturing inserted
            // Use ValuedFull::Merge then rebuild index for overlapped region.
            // 重叠旧段必须先从 valueIdx 删除，否则残留过期 (Value, StartKey)。
            for o in &overlapped {
                self.valueIdx.remove(&(o.Value, o.Key.StartKey.clone()));
            }
            self.full
                .mergeWithOverlap(item, overlapped, Some(&mut inserted));
            // 仅把本次插入的新段写回索引，保持与主树一致。
            for i in inserted {
                self.valueIdx.insert((i.Value, i.Key.StartKey.clone()), i);
            }
        }
    }

    /// 按值升序遍历所有 `Value < n` 的段；回调返回 false 则提前停止。
    pub fn TraverseValuesLessThan<F: FnMut(Valued) -> bool>(&self, n: Value, mut action: F) {
        // range 上界为 (n, [])：BTreeMap 序下恰好排除 Value >= n。
        for ((val, _), v) in self.valueIdx.range(..(n, Vec::new())) {
            let _ = val;
            if !action(v.clone()) {
                break;
            }
        }
    }

    /// 值序最小的一段（最落后检查点），无数据时返回 None。
    pub fn Min(&self) -> Option<Valued> {
        self.valueIdx.values().next().cloned()
    }

    /// `Min` 的 Value 投影，供推进器判断全局最小检查点。
    pub fn MinValue(&self) -> Option<Value> {
        self.Min().map(|v| v.Value)
    }

    /// 键序遍历，直接转发主树 `Traverse`。
    pub fn Traverse<F: FnMut(Valued) -> bool>(&self, m: F) {
        self.full.Traverse(m);
    }
}

/// 全键空间且统一初值 `init` 的空 `ValueSortedFull`，对应 Go `NewSortedFull`。
pub fn NewSortedFull(init: Value) -> ValueSortedFull {
    Sorted(NewFullWith(&Full(), init))
}
