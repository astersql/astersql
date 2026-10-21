// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
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

use std::collections::HashMap;

use crate::FMSketch;

/// 跨分区合并 FM sketches，并为每个 sampled singleton 保留所属分区的 GEE scale。
#[derive(Debug, Default)]
pub struct PartitionNDVMerger {
    sketch: Option<FMSketch>,
    distinct_per_hash: Option<HashMap<u64, f64>>,
}

impl PartitionNDVMerger {
    /// 消费一个分区 sketch；同一 hash 出现在多处或样本内重复时只计一个 distinct。
    pub fn Merge(&mut self, sketch: FMSketch) {
        if self.distinct_per_hash.is_none() {
            let Some(current) = self.sketch.as_mut() else {
                self.sketch = Some(sketch);
                return;
            };
            if !current.Sampled() && !sketch.Sampled() {
                current.MergeFMSketch(&sketch);
                return;
            }
            self.initDistinctPerHash();
        }

        let current = self.sketch.as_mut().expect("merger sketch initialized");
        let distinct = self
            .distinct_per_hash
            .as_mut()
            .expect("per-hash counts initialized");
        if current.mask < sketch.mask {
            current.mask = sketch.mask;
            distinct.retain(|hash, _| *hash & current.mask == 0);
        }
        let scale = singletonScale(&sketch);
        for (hash, repeated) in sketch.hashset {
            if hash & current.mask != 0 {
                continue;
            }
            let value = if repeated || distinct.contains_key(&hash) {
                1.0
            } else {
                scale
            };
            distinct.insert(hash, value);
            if distinct.len() > current.maxSize {
                current.mask = current.mask * 2 + 1;
                distinct.retain(|hash, _| *hash & current.mask == 0);
            }
        }
    }

    /// 返回跨分区 NDV；单分区时保留 sampled NDV 自身的上下界。
    pub fn NDV(&self) -> i64 {
        let Some(sketch) = self.sketch.as_ref() else {
            return 0;
        };
        let Some(distinct) = self.distinct_per_hash.as_ref() else {
            return sketch.NDV();
        };
        ((sketch.mask + 1) as f64 * distinct.values().sum::<f64>()).round() as i64
    }

    /// 是否合并过 sampled sketch。
    pub fn Sampled(&self) -> bool {
        self.distinct_per_hash.is_some() || self.sketch.as_ref().is_some_and(FMSketch::Sampled)
    }

    /// 当前采样 mask，供诊断与回归断言使用。
    pub fn mask(&self) -> u64 {
        self.sketch.as_ref().map_or(0, |sketch| sketch.mask)
    }

    fn initDistinctPerHash(&mut self) {
        let sketch = self.sketch.as_mut().expect("merger sketch initialized");
        let scale = singletonScale(sketch);
        self.distinct_per_hash = Some(
            sketch
                .hashset
                .iter()
                .map(|(hash, repeated)| (*hash, if *repeated { 1.0 } else { scale }))
                .collect(),
        );
        sketch.hashset.clear();
        sketch.ndvCounts = None;
    }
}

fn singletonScale(sketch: &FMSketch) -> f64 {
    let Some(counts) = sketch.ndvCounts else {
        return 1.0;
    };
    (counts.rows as f64 / counts.samples as f64).sqrt()
}
