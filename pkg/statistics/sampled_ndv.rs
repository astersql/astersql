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

/// 从持久化/ANALYZE 的 sampled protobuf 与计数构造 sketch。
pub fn NewSampledFMSketch(proto: &tipb::FmSketch, rows: i64, samples: i64, nulls: i64) -> FMSketch {
    newSampledFMSketch(
        proto,
        NdvCounts {
            rows,
            samples,
            nulls,
        },
    )
}

/// 用 singleton/repeated hashes 构造 sampled sketch，供分区统计接线使用。
pub fn NewSampledFMSketchFromHashes(
    singles: &[u64],
    multis: &[u64],
    rows: i64,
    samples: i64,
    nulls: i64,
) -> FMSketch {
    let mut proto = tipb::FmSketch::new();
    proto.set_hashset(singles.to_vec());
    proto.set_multi_hashset(multis.to_vec());
    NewSampledFMSketch(&proto, rows, samples, nulls)
}

impl FMSketch {
    /// 是否由 sampled rows 估算 NDV。
    pub fn Sampled(&self) -> bool {
        self.ndvCounts.is_some()
    }

    /// ANALYZE 观察到的非 NULL 行数；完整输入草图退化为自身 NDV。
    pub fn NotNullRows(&self) -> i64 {
        self.ndvCounts
            .map_or_else(|| self.NDV(), |counts| counts.rows - counts.nulls)
    }

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

    pub(crate) fn encodeSampled(&self) -> Result<Vec<u8>, astersql_errors::SharedError> {
        let counts = self.ndvCounts.expect("sampled NDV requires row counts");
        let mut collector = tipb::RowSampleCollector::new();
        collector.set_count(counts.rows);
        collector.set_ndv_sample_count(counts.samples);
        collector.set_null_counts(vec![counts.nulls]);
        collector.set_fm_sketch(protobuf::RepeatedField::from_vec(vec![
            crate::FMSketchToProto(Some(self)),
        ]));
        let payload = protobuf::Message::write_to_bytes(&collector)
            .map_err(|error| astersql_errors::New(error.to_string()))?;
        let mut data = Vec::with_capacity(sampledFMSketchHeaderSize + payload.len());
        data.extend([sampledFMSketchMarker, sampledFMSketchVersion]);
        data.extend(payload);
        Ok(data)
    }
}

pub(crate) const sampledFMSketchMarker: u8 = 0;
const sampledFMSketchVersion: u8 = 1;
const sampledFMSketchHeaderSize: usize = 2;

pub(crate) fn decodeSampledFMSketch(data: &[u8]) -> Result<FMSketch, astersql_errors::SharedError> {
    if data.len() < sampledFMSketchHeaderSize || data[1] != sampledFMSketchVersion {
        return Err(astersql_errors::New(
            "unsupported sampled NDV sketch format",
        ));
    }
    let collector =
        protobuf::parse_from_bytes::<tipb::RowSampleCollector>(&data[sampledFMSketchHeaderSize..])
            .map_err(|error| astersql_errors::New(error.to_string()))?;
    if collector.get_fm_sketch().len() != 1
        || !collector.has_ndv_sample_count()
        || collector.get_null_counts().len() != 1
    {
        return Err(astersql_errors::New("invalid sampled NDV sketch payload"));
    }
    Ok(newSampledFMSketch(
        &collector.get_fm_sketch()[0],
        NdvCounts {
            rows: collector.get_count(),
            samples: collector.get_ndv_sample_count(),
            nulls: collector.get_null_counts()[0],
        },
    ))
}
