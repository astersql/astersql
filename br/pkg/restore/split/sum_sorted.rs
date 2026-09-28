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

//! 带权区间树 / SplitHelper，对齐 Go `sum_sorted.go`。
//! 在恢复拆分路径中把重叠的备份文件区间按 key 切成互不重叠的片段，
//! 并把 Size/Number 按切分规则均摊到各片段，供后续按阈值决定 split 点。
//! 以 `BTreeMap<StartKey, Valued>` 维护有序覆盖；空 StartKey 哨兵表示全空间起点。

use std::collections::BTreeMap;
use std::fmt;

use astersql_br_pkg_restore_utils::AppliedFile;

use crate::stubs::{CompareBytesExt, KeyRange, logutil};

/// 区间树上挂载的聚合值：字节量与条目数，对应 Go `Value`。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Value {
    /// 区间覆盖的近似字节量，用于按阈值触发 split。
    pub Size: u64,
    /// 区间覆盖的近似键/文件条数，与 Size 一并均摊。
    pub Number: i64,
}

/// 合并两段 Value：取上界语义下的分量相加（Go `join`）。
pub fn join(a: Value, b: Value) -> Value {
    Value {
        Size: a.Size.wrapping_add(b.Size),
        Number: a.Number.wrapping_add(b.Number),
    }
}

/// 相邻子 key 空间，即 `kv.KeyRange` 别名。
pub type Span = KeyRange;

/// 区间与聚合值的绑定，对应 Go `Valued`。
#[derive(Clone, Debug, Default)]
pub struct Valued {
    /// 半开区间 [StartKey, EndKey)；空 EndKey 表示正无穷。
    pub Key: Span,
    /// 挂在该区间上的 Size/Number。
    pub Value: Value,
}

/// 构造有界 Valued；调用方需保证 StartKey/EndKey 非空才会被 Merge 接纳。
pub fn NewValued(startKey: Vec<u8>, endKey: Vec<u8>, value: Value) -> Valued {
    Valued {
        Key: Span {
            StartKey: startKey,
            EndKey: endKey,
        },
        Value: value,
    }
}

impl Valued {
    /// 估算堆上占用：两端 key 字节 + 两个 Vec 元数据 + Size/Number 各 8 字节。
    pub fn MemSize(&self) -> usize {
        self.Key.StartKey.len() + self.Key.EndKey.len() + std::mem::size_of::<Vec<u8>>() * 2 + 8 + 8
    }

    pub fn GetStartKey(&self) -> Vec<u8> {
        self.Key.StartKey.clone()
    }

    pub fn GetEndKey(&self) -> Vec<u8> {
        self.Key.EndKey.clone()
    }
}

// 接入 AppliedFile，便于与 restore utils 的按文件键范围接口共用。
impl AppliedFile for Valued {
    fn GetStartKey(&self) -> Vec<u8> {
        Valued::GetStartKey(self)
    }
    fn GetEndKey(&self) -> Vec<u8> {
        Valued::GetEndKey(self)
    }
}

impl fmt::Display for Valued {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // 展示用：区间字符串化 + Size 转 MB（两位小数）+ Number。
        write!(
            f,
            "({}, {:.2} MB, {})",
            logutil::StringifyRange(&self.Key.StartKey, &self.Key.EndKey),
            self.Value.Size as f64 / 1024.0 / 1024.0,
            self.Value.Number
        )
    }
}

impl Valued {
    /// Go 风格 `String()`，与 Display 一致，便于测试断言。
    pub fn String(&self) -> String {
        self.to_string()
    }
}

/// 非重叠有值区间覆盖全 key 空间的辅助结构，对应 Go `SplitHelper`。
/// 内部按 StartKey 索引；Merge 时先找重叠段再切边、均摊权重。
#[derive(Clone)]
pub struct SplitHelper {
    /// StartKey → Valued；哨兵键为空 Vec。
    inner: BTreeMap<Vec<u8>, Valued>,
}

/// 初始化时插入空 StartKey/EndKey 的零值哨兵，保证任意有界区间都能找到左邻。
pub fn NewSplitHelper() -> SplitHelper {
    let mut t = BTreeMap::new();
    t.insert(
        Vec::new(),
        Valued {
            Value: Value { Size: 0, Number: 0 },
            Key: Span {
                StartKey: Vec::new(),
                EndKey: Vec::new(),
            },
        },
    );
    SplitHelper { inner: t }
}

impl SplitHelper {
    /// 合并一段有界 Valued：空起止直接忽略（与 Go 一致，避免污染哨兵）。
    pub fn Merge(&mut self, val: Valued) {
        // 空起止视为非法有界输入，直接丢弃。
        if val.Key.StartKey.is_empty() || val.Key.EndKey.is_empty() {
            return;
        }
        let mut overlaps = Vec::with_capacity(8);
        // 先收集重叠，再一次性改写，避免遍历中突变树。
        self.overlapped(&val.Key, &mut overlaps);
        self.mergeWithOverlap(val, overlaps);
    }

    /// 按 StartKey 序遍历；回调返回 false 时提前停止（Go `Traverse`）。
    pub fn Traverse<F>(&self, mut m: F)
    where
        F: FnMut(Valued) -> bool,
    {
        for v in self.inner.values() {
            if !m(v.clone()) {
                break;
            }
        }
    }

    /// 核心切分：先摘掉重叠段，再按左右 trail 裁边，并把新值均摊到重叠片段。
    fn mergeWithOverlap(&mut self, val: Valued, mut overlapped: Vec<Valued>) {
        // 无重叠则无需改树（调用方通常已保证有命中）。
        if overlapped.is_empty() {
            return;
        }

        // 从树中移除即将被改写的重叠段，稍后按切分结果重新插入。
        for r in &overlapped {
            self.inner.remove(&r.Key.StartKey);
        }

        // 新区间权重按重叠段个数均摊，对齐 Go 的整数除法截断语义。
        let appendValue = Value {
            Size: val.Value.Size / overlapped.len() as u64,
            Number: val.Value.Number / overlapped.len() as i64,
        };
        let mut rightTrail: Option<Valued> = None;
        let mut leftTrail: Option<Valued> = None;

        // 左 trail：原段起点早于新区间，保留 [left.Start, val.Start)，值不变。
        let leftmost = overlapped[0].clone();
        if leftmost.Key.StartKey.as_slice() < val.Key.StartKey.as_slice() {
            leftTrail = Some(Valued {
                Key: Span {
                    StartKey: leftmost.Key.StartKey,
                    EndKey: val.Key.StartKey.clone(),
                },
                Value: leftmost.Value,
            });
            overlapped[0].Key.StartKey = val.Key.StartKey.clone();
        }

        // 右 trail：原段终点晚于新区间，保留 [val.End, right.End)。
        let last = overlapped.len() - 1;
        let rightmost = overlapped[last].clone();
        if CompareBytesExt(&rightmost.Key.EndKey, true, &val.Key.EndKey, true) > 0 {
            rightTrail = Some(Valued {
                Key: Span {
                    StartKey: val.Key.EndKey.clone(),
                    EndKey: rightmost.Key.EndKey,
                },
                Value: rightmost.Value,
            });
            overlapped[last].Key.EndKey = val.Key.EndKey.clone();

            // 单段同时被左右裁切时，Go 用 2/3 比例重分配，避免中间段被双重减半。
            if overlapped.len() == 1 && leftTrail.is_some() {
                let adjusted = Value {
                    Size: rightTrail.as_ref().unwrap().Value.Size.wrapping_mul(2) / 3,
                    Number: rightTrail.as_ref().unwrap().Value.Number.wrapping_mul(2) / 3,
                };
                leftTrail.as_mut().unwrap().Value = adjusted;
                overlapped[0].Value = adjusted;
                rightTrail.as_mut().unwrap().Value = adjusted;
            }
        }

        // emit：standalone trail 只做 split 减半；重叠体则 join(appendValue, …)。
        let emit = |inner: &mut BTreeMap<Vec<u8>, Valued>,
                    mut rng: Valued,
                    standalone: bool,
                    split: bool| {
            let mut merged = rng.Value;
            if split {
                merged.Size /= 2;
                merged.Number /= 2;
            }
            if !standalone {
                merged = join(appendValue, merged);
            }
            rng.Value = merged;
            inner.insert(rng.Key.StartKey.clone(), rng);
        };

        if let Some(left) = leftTrail.clone() {
            // 左 trail 为 standalone+split，与 Go 一致。
            emit(&mut self.inner, left, true, true);
        }

        for (i, rng) in overlapped.into_iter().enumerate() {
            // 与 trail 相邻的端点段需要 split，避免权重被 trail 与重叠体重复计入。
            let split = (i == 0 && leftTrail.is_some()) || (i == last && rightTrail.is_some());
            emit(&mut self.inner, rng, false, split);
        }

        if let Some(right) = rightTrail {
            // 右 trail 同样 standalone+split。
            emit(&mut self.inner, right, true, true);
        }
    }

    /// 收集与 `k` 重叠的已有段：从 ≤StartKey 的最近节点起向右扫描直到不再重叠。
    fn overlapped(&self, k: &Span, result: &mut Vec<Valued>) {
        // 定位可能覆盖 StartKey 的最右已有段起点。
        let first_key = self
            .inner
            .range(..=k.StartKey.clone())
            .next_back()
            .map(|(key, _)| key.clone())
            .unwrap_or_default();

        for (_, r) in self.inner.range(first_key..) {
            // 一旦不相交即可停止：树按 StartKey 有序。
            if !checkOverlaps(&r.Key, k) {
                break;
            }
            result.push(r.clone());
        }
    }
}

/// 判断 `a` 与有界区间 `ap` 是否重叠；`a.EndKey` 为空表示正无穷右界。
pub fn checkOverlaps(a: &Span, ap: &Span) -> bool {
    if a.EndKey.is_empty() {
        // 开区间右界：ap 终点必须严格大于 a 起点才算相交。
        return ap.EndKey.as_slice() > a.StartKey.as_slice();
    }
    a.StartKey.as_slice() < ap.EndKey.as_slice() && ap.StartKey.as_slice() < a.EndKey.as_slice()
}
