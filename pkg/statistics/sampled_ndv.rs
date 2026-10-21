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

use crate::{FMSketch, MaxSketchSize};

/// GEE 在 FM sketch 之外需要的行数。
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct NdvCounts {
    /// 扫描到的全部可见行。
    pub(crate) rows: i64,
    /// TiKV 用于 NDV 的样本行，包含 NULL 行。
    pub(crate) samples: i64,
    /// 按全部行数缩放后的 NULL 行。
    pub(crate) nulls: i64,
}

pub(crate) fn newSampledFMSketch(proto: &tipb::FmSketch, counts: NdvCounts) -> FMSketch {
    let mut hashset =
        HashMap::with_capacity(proto.get_hashset().len() + proto.get_multi_hashset().len());
    for hash in proto.get_hashset() {
        hashset.insert(*hash, false);
    }
    for hash in proto.get_multi_hashset() {
        hashset.insert(*hash, true);
    }
    FMSketch {
        hashset,
        ndvCounts: Some(counts),
        mask: proto.get_mask(),
        maxSize: MaxSketchSize,
    }
}

impl FMSketch {
    pub(crate) fn sampledNDV(&self) -> i64 {
        let counts = self.ndvCounts.expect("sampled NDV requires row counts");
        if counts.samples == 0 {
            return 0;
        }

        let weight = (self.mask + 1) as f64;
        let distinct = (counts.samples as f64).min(weight * self.hashset.len() as f64);
        let singles = self.hashset.values().filter(|repeated| !**repeated).count();
        let f1 = distinct.min(weight * singles as f64);
        let estimate = (distinct + ((counts.rows as f64 / counts.samples as f64).sqrt() - 1.0) * f1)
            .round() as i64;
        estimate.min(counts.rows - counts.nulls)
    }
}
