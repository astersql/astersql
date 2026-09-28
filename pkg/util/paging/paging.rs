// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
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

// 分布式 SQL 分页请求：页大小几何增长与 seek 次数估算。
//
// 分页（paging）将大范围扫描拆成多批；首批从小页开始，未扫完则按 2 倍放大，
// 直至上限。相较一次 unary 请求，可尽早返回数据，近似流式处理。
// 详见 https://github.com/pingcap/tidb/issues/36328 。

#![allow(non_snake_case, non_upper_case_globals)]

// This module preserves the exported Go paging API and its uint64 arithmetic semantics.

// A paging request may be separated into multi requests if there are more data than a page.
// The paging size grows from min to max. See https://github.com/pingcap/tidb/issues/36328
// e.g. a paging request scans over range (r1, r200), it requires 128 rows in the first batch,
// if it's not drained, then the paging size grows, the new range is calculated like (r100, r200), then send a request again.
// Compare with the common unary request, paging request allows early access of data, it offers a streaming-like way processing data.
// MinPagingSize 对应 Go 导出常量，表示分页请求首批最小行数。
/// 分页首批最小行数（默认 128）。
pub const MinPagingSize: u64 = 128;

// maxPagingSizeShift、pagingSizeGrow、pagingGrowingSum 保留 Go 的几何级数增长参数。
/// 几何增长的最大移位次数参数。
pub(crate) const maxPagingSizeShift: u32 = 7;
/// 每次增长的倍率（2 倍）。
pub(crate) const pagingSizeGrow: u64 = 2;

// MinAllowedMaxPagingSize 对应 Go 导出常量，表示允许的最大分页上限下界。
/// 允许的最大分页上限之下界（调用方 max 过小时会被抬到此值）。
pub const MinAllowedMaxPagingSize: u64 = 50000;
/// 几何级数前若干项之和：用于 CalculateSeekCnt 分段。
pub(crate) const pagingGrowingSum: u64 = ((2_u64 << maxPagingSizeShift) - 1) * MinPagingSize;

// Threshold 对应 Go 导出常量，作为分页策略阈值。
/// 分页策略相关阈值（对齐 Go 导出常量）。
pub const Threshold: u64 = 960;

// GrowPagingSize grows the paging size and ensures it does not exceed
// max(maxv, MinAllowedMaxPagingSize).
// GrowPagingSize 对应 Go 的分页大小增长函数，输入输出都是纯数值，没有外部依赖。
/// 将分页大小左移一位（×2），且不超过 max(maxv, MinAllowedMaxPagingSize)。
pub fn GrowPagingSize(mut size: u64, mut maxv: u64) -> u64 {
    if maxv < MinAllowedMaxPagingSize {
        // Defensive programing, for example, call with max = 0.
        // max should never less than MinAllowedMaxPagingSize.
        // Otherwise, the session variable maybe wrong, or the distsql request
        // does not obey the session variable setting.
        // Go 代码在上限过小时防御性抬高，避免调用方传入 0 等异常值导致分页上限过低。
        maxv = MinAllowedMaxPagingSize;
    }

    // Go 的 size <<= 1 映射为左移一位，表示分页大小按 2 倍增长。
    size <<= 1;
    if size > maxv {
        return maxv;
    }
    size
}

// CalculateSeekCnt calculates the seek count from expect count
// CalculateSeekCnt 对应 Go 的 seek 次数估算函数，按期望行数落入的增长区间返回浮点估值。
/// 由期望行数估算 seek 次数；超过几何总和后按上限向上取整追加。
pub fn CalculateSeekCnt(expectCnt: u64) -> f64 {
    if expectCnt == 0 {
        // 期望行数为 0 时不需要 seek，沿用 Go 的早返回。
        return 0.0;
    }
    if expectCnt > pagingGrowingSum {
        // if the expectCnt is larger than pagingGrowingSum, calculate the seekCnt for the excess.
        // 超过几何增长总和后，额外部分按 MinAllowedMaxPagingSize 向上取整估算。
        let excess = (expectCnt - pagingGrowingSum).wrapping_add(MinAllowedMaxPagingSize - 1)
            / MinAllowedMaxPagingSize;
        return (8 + excess) as f64;
    }
    if expectCnt > MinPagingSize {
        // if the expectCnt is less than pagingGrowingSum,
        // calculate the seekCnt(number of terms) from the sum of a geometric progression.
        // expectCnt = minPagingSize * (pagingSizeGrow ^ seekCnt - 1) / (pagingSizeGrow - 1)
        // simplify (pagingSizeGrow ^ seekCnt - 1) to pagingSizeGrow ^ seekCnt, we can infer that
        // seekCnt = log((pagingSizeGrow - 1) * expectCnt / minPagingSize) / log(pagingSizeGrow)
        // Go 使用 int(...) 截断对数结果；用 as u64 保留向零截断的整数化形状。
        let ratio = ((pagingSizeGrow - 1) * expectCnt) as f64 / MinPagingSize as f64;
        return 1.0 + (ratio.ln() / (pagingSizeGrow as f64).ln()) as u64 as f64;
    }
    1.0
}
