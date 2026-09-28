// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc. Licensed under Apache-2.0.

//! 带值的有序 span 树，对齐 Go `sorted.go`。
//!
//! `ValuedFull` 以 StartKey 为索引维护互不重叠的区间；`Merge` 对重叠段取 `max` 合并值，
//! 并切分左右余量，供日志备份推进全局最小检查点。
//!
//! 合并后相邻同值区间会粘连，降低碎片；无重叠时 `mergeWithOverlap` 直接返回。
//! `overlapped` 先找可能跨入 StartKey 的前驱，再向右扫描直至不再重叠。
//! Valued span tree matching `sorted.go`.

use std::collections::BTreeMap;

use crate::utils::{Collapse, CompareBytesExt, Overlaps};

/// 区间上附着的进度值（通常为 checkpoint TS）。
pub type Value = u64;

/// 重叠段取值策略：取较大者（推进单调不减）。
/// 与 Go `join` 一致：检查点水位只升不降。
fn join(a: Value, b: Value) -> Value {
    a.max(b)
}

/// 半开键区间 `[StartKey, EndKey)`；空 EndKey 语义由调用方解释。
/// 比较时常用 `CompareBytesExt` 处理空键作为 ±∞ 的约定。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Span {
    pub StartKey: Vec<u8>,
    pub EndKey: Vec<u8>,
}

/// 带进度值的区间单元。
/// `Value` 在 Merge 重叠区经 `join` 合并。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Valued {
    pub Key: Span,
    pub Value: Value,
}

impl Valued {
    /// 按 StartKey 字典序比较，供排序辅助。
    pub fn Less(&self, other: &Valued) -> bool {
        self.Key.StartKey < other.Key.StartKey
    }
}

/// 全量覆盖树：`inner` 以 StartKey 为键，区间互不重叠且按序排列。
pub struct ValuedFull {
    // keyed by StartKey
    /// 按 StartKey 索引的互不重叠 valued 区间。
    inner: BTreeMap<Vec<u8>, Valued>,
}

/// 用折叠后的初始 spans 填满树，每段赋予同一 `init` 值。
/// 典型用法：`Full()` 得到全键空间再 `NewFullWith(..., 初始TS)`。
pub fn NewFullWith(initSpans: &[Span], init: Value) -> ValuedFull {
    let mut t = BTreeMap::new();
    // Collapse 先合并重叠输入，避免初始树出现交错空洞。
    // 折叠后再插入，保证树不变量从一开始成立。
    for r in Collapse(initSpans) {
        t.insert(
            r.StartKey.clone(),
            Valued {
                Value: init,
                Key: r,
            },
        );
    }
    ValuedFull { inner: t }
}

impl ValuedFull {
    /// 将新 valued 区间并入树：先找重叠段，再切分/合并。
    /// `newItems` 在此路径传 None；测试/差分路径可收集新段。
    pub fn Merge(&mut self, val: Valued) {
        let mut overlaps = Vec::new();
        self.overlapped(&val.Key, &mut overlaps);
        self.mergeWithOverlap(val, overlaps, None);
    }

    /// 按 StartKey 顺序遍历；回调返回 false 时提前停止。
    /// 用于聚合最小水位或导出当前覆盖图。
    pub fn Traverse<F: FnMut(Valued) -> bool>(&self, mut m: F) {
        for v in self.inner.values() {
            if !m(v.clone()) {
                break;
            }
        }
    }

    /// 核心合并：删除重叠旧段，按左余量/重叠体/右余量重写，相邻同值自动粘连。
    pub(crate) fn mergeWithOverlap(
        &mut self,
        val: Valued,
        mut overlapped: Vec<Valued>,
        mut newItems: Option<&mut Vec<Valued>>,
    ) {
        // 无重叠则无需改动（调用方保证仅在有覆盖时合并）。
        if overlapped.is_empty() {
            return;
        }
        // 先移除旧段，避免与重写结果键冲突。
        for r in &overlapped {
            self.inner.remove(&r.Key.StartKey);
        }

        let mut initialized = false;
        let mut collected = Valued {
            Key: Span::default(),
            Value: 0,
        };
        // 右侧余量暂存，待中间段处理完再 emit。
        let mut rightTrail: Option<Valued> = None;

        // 将已累计段写回树，并可选记入 newItems。
        let flushCollected =
            |this: &mut ValuedFull,
             initialized: &bool,
             collected: &Valued,
             newItems: &mut Option<&mut Vec<Valued>>| {
                if *initialized {
                    this.inner
                        .insert(collected.Key.StartKey.clone(), collected.clone());
                    if let Some(ni) = newItems.as_mut() {
                        ni.push(collected.clone());
                    }
                }
            };

        // standalone=true 保留原值（左右余量）；否则与 val.Value 取 max。
        let mut emitToCollected =
            |this: &mut ValuedFull,
             rng: Valued,
             standalone: bool,
             initialized: &mut bool,
             collected: &mut Valued,
             newItems: &mut Option<&mut Vec<Valued>>| {
                let merged = if standalone {
                    rng.Value
                } else {
                    join(val.Value, rng.Value)
                };
                if !*initialized {
                    *collected = rng;
                    collected.Value = merged;
                    *initialized = true;
                    return;
                }
                // 同值且首尾相接则扩展 EndKey，减少碎片。
                if merged == collected.Value
                    && CompareBytesExt(&collected.Key.EndKey, true, &rng.Key.StartKey, false) == 0
                {
                    collected.Key.EndKey = rng.Key.EndKey;
                } else {
                    flushCollected(this, initialized, collected, newItems);
                    *collected = Valued {
                        Key: rng.Key,
                        Value: merged,
                    };
                }
            };

        // 左侧余量：首重叠段起点早于 val，先单独吐出未覆盖左半。
        if overlapped[0].Key.StartKey.as_slice() < val.Key.StartKey.as_slice() {
            let left = overlapped[0].clone();
            emitToCollected(
                self,
                Valued {
                    Key: Span {
                        StartKey: left.Key.StartKey,
                        EndKey: val.Key.StartKey.clone(),
                    },
                    Value: left.Value,
                },
                true,
                &mut initialized,
                &mut collected,
                &mut newItems,
            );
            overlapped[0].Key.StartKey = val.Key.StartKey.clone();
        }

        // 右侧余量：末重叠段终点晚于 val，截断后记入 rightTrail。
        let last = overlapped.len() - 1;
        if CompareBytesExt(&overlapped[last].Key.EndKey, true, &val.Key.EndKey, true) > 0 {
            rightTrail = Some(Valued {
                Key: Span {
                    StartKey: val.Key.EndKey.clone(),
                    EndKey: overlapped[last].Key.EndKey.clone(),
                },
                Value: overlapped[last].Value,
            });
            overlapped[last].Key.EndKey = val.Key.EndKey.clone();
        }

        // 中间重叠体：与 val 做 join。
        for rng in overlapped {
            emitToCollected(
                self,
                rng,
                false,
                &mut initialized,
                &mut collected,
                &mut newItems,
            );
        }
        if let Some(rt) = rightTrail {
            emitToCollected(
                self,
                rt,
                true,
                &mut initialized,
                &mut collected,
                &mut newItems,
            );
        }
        flushCollected(self, &initialized, &collected, &mut newItems);
    }

    /// 收集与 `k` 重叠的全部树中区间；先定位可能覆盖 StartKey 的前驱。
    /// 结果按 StartKey 升序追加，供 `mergeWithOverlap` 顺序切分。
    pub(crate) fn overlapped(&self, k: &Span, result: &mut Vec<Valued>) {
        let mut first = k.clone();
        let mut hasFirst = false;
        // floor(StartKey)：可能从左侧跨入的区间。
        if let Some((_, v)) = self.inner.range(..=k.StartKey.clone()).next_back() {
            first = v.Key.clone();
            hasFirst = true;
        }
        // 前驱不重叠则从 k 自身起点开始向右扫。
        if !hasFirst || !Overlaps(&first, k) {
            first = k.clone();
        }
        for (_, r) in self.inner.range(first.StartKey.clone()..) {
            if !Overlaps(&r.Key, k) {
                break;
            }
            result.push(r.clone());
        }
    }
}
