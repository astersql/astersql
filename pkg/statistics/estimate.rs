// Copyright 2026 AsterSQL.
// Copyright 2019 PingCAP, Inc.
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

// NDV（Number of Distinct Values，不同值个数）估算。
//
// 基于采样 TopN 辅助结构，用 GEE（Guaranteed Error Estimator）公式
// 把样本 NDV 外推到全表。

/// 采样中某个编码值及其出现次数。
#[derive(Clone, Debug)]
pub(crate) struct dataCnt {
    /// 编码后的列/索引键字节。
    pub data: Vec<u8>,
    /// 该值在样本中的出现次数。
    pub cnt: u64,
}

/// 由样本构造的 TopN 辅助信息，供 NDV 估算使用。
#[derive(Clone, Debug)]
pub(crate) struct topNHelper {
    /// 按频率降序排列的样本取值。
    pub sorted: Vec<dataCnt>,
    /// 样本总行数。
    pub sampleSize: u64,
    /// 样本中仅出现一次的取值个数（singleton）。
    pub singletonItems: u64,
    /// TopN 条目的频率之和。
    pub sumTopN: u64,
    /// 实际纳入 TopN 的条目数。
    pub actualNumTop: u32,
}

/// 根据 TopN 辅助结构估算全表 NDV，并返回样本到全表的缩放比。
///
/// 返回值 `(estimated_ndv, scale_ratio)`：`scale_ratio = row_count / sample_size`。
/// 若样本全是 singleton，则 NDV 近似等于全表行数；若无 singleton，则 NDV 取样本 NDV。
pub(crate) fn calculateEstimateNDV(helper: &topNHelper, row_count: u64) -> (u64, u64) {
    let sample_size = helper.sampleSize;
    let sample_ndv = helper.sorted.len() as u64;
    let singleton_items = helper.singletonItems;
    let scale_ratio = row_count / sample_size;
    // 样本每个值都只出现一次：外推 NDV ≈ 全表行数，缩放比为 1。
    if singleton_items == sample_size {
        return (row_count, 1);
    }
    // 无 singleton：样本已覆盖的不同值可直接作为 NDV，频率按 scale_ratio 放大。
    if singleton_items == 0 {
        return (sample_ndv, scale_ratio);
    }
    (
        EstimateNDVByGEE(sample_ndv, singleton_items, sample_size, row_count),
        scale_ratio,
    )
}

/// 使用 GEE 公式由样本 NDV / singleton 数估算全表 NDV。
///
/// 公式：`sample_ndv + (sqrt(row_count/sample_size) - 1) * singleton_items`，
/// 再四舍五入，并夹在 `[sample_ndv, row_count]` 区间内。
pub fn EstimateNDVByGEE(
    sample_ndv: u64,
    singleton_items: u64,
    sample_size: u64,
    row_count: u64,
) -> u64 {
    assert!(sample_size > 0, "sampleSize should be greater than 0");
    assert!(sample_ndv > 0, "sampleNDV should be greater than 0");
    assert!(
        row_count >= sample_ndv,
        "rowCount should be greater than or equal to sampleNDV"
    );
    let estimate = sample_ndv as f64
        + ((row_count as f64 / sample_size as f64).sqrt() - 1.0) * singleton_items as f64;
    let mut ndv = (estimate + 0.5) as u64;
    ndv = ndv.max(sample_ndv);
    if row_count > 0 {
        ndv = ndv.min(row_count);
    }
    ndv
}
