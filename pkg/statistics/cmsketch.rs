// Copyright 2026 AsterSQL.
// Copyright 2017 PingCAP, Inc.
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

// Count-Min Sketch（CMSketch）与 TopN：频数估计、高频值提取及编解码。
//
// CMSketch 用 depth×width 的哈希计数矩阵近似统计各值出现次数；
// TopN 单独精确记录最高频的若干值，查询时优先命中 TopN，未命中再查 sketch。

use std::cmp::Ordering;
use std::collections::HashMap;
use std::fmt;

use protobuf::{Message as _, RepeatedField};

use crate::{calculateEstimateNDV, dataCnt, murmur3Sum128, topNHelper};

/// 启用 TopN 的阈值：样本中 TopN 合计频次需至少达到 `sampleSize / 该阈值`。
pub const topNThreshold: u64 = 10;

#[derive(Clone, Debug, PartialEq, Eq)]
/// Count-Min Sketch：多行独立哈希的计数器矩阵，用于近似点查询频数。
pub struct CMSketch {
    table: Vec<Vec<u32>>,
    count: u64,
    defaultValue: u64,
    depth: i32,
    width: i32,
}

/// 创建指定深度与宽度的空 CMSketch。
pub fn NewCMSketch(depth: i32, width: i32) -> CMSketch {
    assert!(depth > 0, "CMSketch depth must be positive");
    assert!(width > 0, "CMSketch width must be positive");
    CMSketch {
        table: vec![vec![0; width as usize]; depth as usize],
        count: 0,
        defaultValue: 0,
        depth,
        width,
    }
}

/// 从样本统计各值频次，选出实际 TopN 候选并汇总辅助量（供 NDV/默认值估算）。
fn newTopNHelper(sample: &[Vec<u8>], num_top: u32) -> topNHelper {
    let mut counter = HashMap::<Vec<u8>, u64>::with_capacity(sample.len());
    for value in sample {
        *counter.entry(value.clone()).or_default() += 1;
    }
    // 统计每个编码值出现次数，并按频次降序排列。
    let mut singleton_items = 0;
    let mut sorted = counter
        .into_iter()
        .map(|(data, cnt)| {
            singleton_items += u64::from(cnt == 1);
            dataCnt { data, cnt }
        })
        .collect::<Vec<_>>();
    sorted.sort_by(|left, right| {
        right
            .cnt
            .cmp(&left.cnt)
            .then_with(|| left.data.cmp(&right.data))
    });

    let sample_ndv = sorted.len() as u32;
    let num_top = num_top.min(sample_ndv);
    let mut actual_num_top = 0_u32;
    let mut sum_top_n = 0_u64;
    // 最多考察 2*num_top 个候选；频次骤降或遇到 singleton 则停止扩展。
    while actual_num_top < sample_ndv && actual_num_top < num_top.saturating_mul(2) {
        let index = actual_num_top as usize;
        if actual_num_top >= num_top
            && sorted[index].cnt.saturating_mul(3)
                < sorted[num_top as usize - 1].cnt.saturating_mul(2)
        {
            break;
        }
        if sorted[index].cnt == 1 {
            break;
        }
        sum_top_n += sorted[index].cnt;
        actual_num_top += 1;
    }

    topNHelper {
        sorted,
        sampleSize: sample.len() as u64,
        singletonItems: singleton_items,
        sumTopN: sum_top_n,
        actualNumTop: actual_num_top,
    }
}

/// 由样本同时构建 CMSketch 与 TopN，并返回估计 NDV 与缩放比。
pub fn NewCMSketchAndTopN(
    depth: i32,
    width: i32,
    sample: &[Vec<u8>],
    num_top: u32,
    row_count: u64,
) -> (Option<CMSketch>, Option<TopN>, u64, u64) {
    // 空表或空样本：无需 sketch/TopN。
    if row_count == 0 || sample.is_empty() {
        return (None, None, 0, 0);
    }
    let mut helper = newTopNHelper(sample, num_top);
    let row_count = row_count.max(sample.len() as u64);
    let (estimated_ndv, scale_ratio) = calculateEstimateNDV(&helper, row_count);
    let default_value = calculateDefaultVal(&helper, estimated_ndv, scale_ratio, row_count);
    let (cmsketch, top_n) = buildCMSAndTopN(&mut helper, depth, width, scale_ratio, default_value);
    (Some(cmsketch), top_n, estimated_ndv, scale_ratio)
}

/// 按 helper 结果写入 TopN（若启用）并把其余/全部值灌入 CMSketch。
fn buildCMSAndTopN(
    helper: &mut topNHelper,
    depth: i32,
    width: i32,
    scale_ratio: u64,
    default_value: u64,
) -> (CMSketch, Option<TopN>) {
    let mut cmsketch = NewCMSketch(depth, width);
    // 仅当 TopN 合计频次足够高时才启用精确 TopN 路径。
    let enable_top_n = helper.sampleSize / topNThreshold <= helper.sumTopN;
    let top_n = if enable_top_n {
        let mut top_n = NewTopN(helper.actualNumTop as usize);
        for item in helper.sorted.drain(..helper.actualNumTop as usize) {
            top_n.AppendTopN(item.data, item.cnt * scale_ratio);
        }
        top_n.Sort();
        Some(top_n)
    } else {
        None
    };
    cmsketch.defaultValue = default_value;
    for item in &helper.sorted {
        let count = if item.cnt > 1 {
            item.cnt * scale_ratio
        } else {
            default_value
        };
        cmsketch.InsertBytesByCount(&item.data, count);
    }
    (cmsketch, top_n)
}

/// 估算未在样本中重复出现的值的默认计数（defaultValue）。
fn calculateDefaultVal(
    helper: &topNHelper,
    estimated_ndv: u64,
    scale_ratio: u64,
    row_count: u64,
) -> u64 {
    let sample_ndv = helper.sorted.len() as u64;
    let repeated_count = (helper.sampleSize - helper.singletonItems) * scale_ratio;
    if row_count <= repeated_count {
        return 1;
    }
    (row_count - repeated_count) / (estimated_ndv - sample_ndv + helper.singletonItems).max(1)
}

impl CMSketch {
    /// 估算 sketch 矩阵占用的内存字节数（depth*width*4）。
    pub fn MemoryUsage(&self) -> i64 {
        i64::from(self.depth) * i64::from(self.width) * 4
    }

    /// 将字节键计数加一。
    pub fn InsertBytes(&mut self, bytes: &[u8]) {
        self.InsertBytesByCount(bytes, 1);
    }

    /// 将字节键计数增加 `count`：对每一行用 Murmur3 派生列下标并累加。
    pub fn InsertBytesByCount(&mut self, bytes: &[u8], count: u64) {
        let (h1, h2) = murmur3Sum128(bytes);
        self.count += count;
        for (index, row) in self.table.iter_mut().enumerate() {
            let column =
                (h1.wrapping_add(h2.wrapping_mul(index as u64)) % self.width as u64) as usize;
            row[column] = row[column].wrapping_add(count as u32);
        }
    }

    /// 判断查询结果是否应回落到 defaultValue（噪声过大或过小）。
    fn considerDefVal(&self, count: u64) -> bool {
        (count == 0 || (count > self.defaultValue && count < 2 * (self.count / self.width as u64)))
            && self.defaultValue > 0
    }

    /// 按已有哈希对 (h1,h2) 从各行计数器扣减 `count`。
    pub fn SubValue(&mut self, h1: u64, h2: u64, count: u64) {
        self.count -= count;
        for (index, row) in self.table.iter_mut().enumerate() {
            let column =
                (h1.wrapping_add(h2.wrapping_mul(index as u64)) % self.width as u64) as usize;
            row[column] -= count as u32;
        }
    }

    /// 查询字节键的估计频数。
    pub fn QueryBytes(&self, data: &[u8]) -> u64 {
        let (h1, h2) = murmur3Sum128(data);
        self.queryHashValue(h1, h2)
    }

    /// 对哈希对做 noise 校正与中位数估计，必要时回落 defaultValue。
    fn queryHashValue(&self, h1: u64, h2: u64) -> u64 {
        let mut values = Vec::with_capacity(self.depth as usize);
        let mut min_value = u32::MAX;
        const TEMP: u32 = 1;
        // 每行取计数，减去估计噪声后收集，再取近似中位数作为结果。
        for (index, row) in self.table.iter().enumerate() {
            let column =
                (h1.wrapping_add(h2.wrapping_mul(index as u64)) % self.width as u64) as usize;
            let original = row[column];
            min_value = min_value.min(original);
            let noise = (self.count - u64::from(original)) / (self.width as u64 - 1);
            values.push(if original == 0 {
                0
            } else if u64::from(original) < noise {
                TEMP
            } else {
                original - noise as u32 + TEMP
            });
        }
        values.sort_unstable();
        let left = values[(self.depth as usize - 1) / 2];
        let right = values[self.depth as usize / 2];
        let result = (left + (right - left) / 2).min(min_value + TEMP);
        if result == 0 {
            return 0;
        }
        let result = u64::from(result - TEMP);
        if self.considerDefVal(result) {
            self.defaultValue
        } else {
            result
        }
    }

    /// 维数相同的两个 sketch 按单元相加合并；维数不同则报错。
    pub fn MergeCMSketch(&mut self, other: &CMSketch) -> Result<(), astersql_errors::SharedError> {
        if self.depth != other.depth || self.width != other.width {
            return Err(astersql_errors::New(
                "Dimensions of Count-Min Sketch should be the same",
            ));
        }
        self.count += other.count;
        for (destination, source) in self.table.iter_mut().zip(&other.table) {
            for (destination, source) in destination.iter_mut().zip(source) {
                *destination = destination.wrapping_add(*source);
            }
        }
        Ok(())
    }

    /// 返回已插入的总计数。
    pub fn TotalCount(&self) -> u64 {
        self.count
    }

    /// 判断两个 sketch 是否完全相等。
    pub fn Equal(&self, other: &CMSketch) -> bool {
        self == other
    }

    /// 深拷贝 sketch。
    pub fn Copy(&self) -> CMSketch {
        self.clone()
    }

    /// 返回 (width, depth)。
    pub fn GetWidthAndDepth(&self) -> (i32, i32) {
        (self.width, self.depth)
    }

    /// ANALYZE 路径下按总计数/NDV 设置 defaultValue。
    pub fn CalcDefaultValForAnalyze(&mut self, ndv: u64) {
        self.defaultValue = self.count / ndv.max(1);
    }

    /// 返回当前 defaultValue。
    pub fn DefaultValue(&self) -> u64 {
        self.defaultValue
    }
}

/// 先查 TopN，未命中再查 CMSketch；Datum 需按语句时区编码。
pub fn QueryValue(
    statement_context: Option<&stmtctx::StatementContext>,
    cmsketch: &CMSketch,
    top_n: Option<&TopN>,
    value: types::Datum,
) -> Result<u64, astersql_errors::SharedError> {
    let timezone = statement_context
        .map(stmtctx::StatementContext::TimeZone)
        .unwrap_or(chrono_tz::UTC);
    let raw = match codec::EncodeValue(timezone, Vec::new(), vec![value]) {
        Ok(raw) => raw,
        Err(error) => match statement_context {
            Some(context) => match context.HandleError(Some(error)) {
                Some(error) => return Err(error),
                None => Vec::new(),
            },
            None => return Err(error),
        },
    };
    if let Some((count, true)) = top_n.map(|top_n| top_n.QueryTopN(&raw)) {
        return Ok(count);
    }
    Ok(cmsketch.QueryBytes(&raw))
}

/// 合并两个 TopN，溢出项写回 CMSketch，返回被 spill 的元数据。
pub fn MergeTopNAndUpdateCMSketch(
    destination: &mut TopN,
    source: &TopN,
    cmsketch: &mut CMSketch,
    num_top: u32,
) -> Vec<TopNMeta> {
    let (merged, spilled) = MergeTopN(&[source, destination], num_top);
    let Some(merged) = merged else {
        return spilled;
    };
    destination.TopN = merged.TopN;
    for item in &spilled {
        cmsketch.InsertBytesByCount(&item.Encoded, item.Count);
    }
    spilled
}

/// 将 CMSketch 与可选 TopN 序列化为 tipb::CmSketch protobuf。
pub fn CMSketchToProto(cmsketch: Option<&CMSketch>, top_n: Option<&TopN>) -> tipb::CmSketch {
    let mut result = tipb::CmSketch::new();
    if let Some(cmsketch) = cmsketch {
        let rows = cmsketch
            .table
            .iter()
            .map(|counters| {
                let mut row = tipb::CmSketchRow::new();
                row.set_counters(counters.clone());
                row
            })
            .collect();
        result.set_rows(RepeatedField::from_vec(rows));
        result.set_default_value(cmsketch.defaultValue);
    }
    if let Some(top_n) = top_n {
        let entries = top_n
            .TopN
            .iter()
            .map(|item| {
                let mut entry = tipb::CmSketchTopN::new();
                entry.set_data(item.Encoded.clone());
                entry.set_count(item.Count);
                entry
            })
            .collect();
        result.set_top_n(RepeatedField::from_vec(entries));
    }
    result
}

/// 从 tipb::CmSketch 反序列化出 CMSketch 与 TopN。
pub fn CMSketchAndTopNFromProto(
    proto: Option<&tipb::CmSketch>,
) -> (Option<CMSketch>, Option<TopN>) {
    let Some(proto) = proto else {
        return (None, None);
    };
    let top_n = TopNFromProto(proto.get_top_n());
    if proto.get_rows().is_empty() {
        return (None, top_n);
    }
    let mut cmsketch = NewCMSketch(
        proto.get_rows().len() as i32,
        proto.get_rows()[0].get_counters().len() as i32,
    );
    for (destination, source) in cmsketch.table.iter_mut().zip(proto.get_rows()) {
        for (column, counter) in source.get_counters().iter().enumerate() {
            destination[column] = *counter;
        }
        cmsketch.count = source
            .get_counters()
            .iter()
            .map(|value| u64::from(*value))
            .sum();
    }
    cmsketch.defaultValue = proto.get_default_value();
    (Some(cmsketch), top_n)
}

/// 仅从 protobuf TopN 条目列表重建 TopN。
pub fn TopNFromProto(entries: &[tipb::CmSketchTopN]) -> Option<TopN> {
    if entries.is_empty() {
        return None;
    }
    let mut top_n = NewTopN(entries.len());
    for entry in entries {
        top_n.AppendTopN(entry.get_data().to_vec(), entry.get_count());
    }
    top_n.Sort();
    Some(top_n)
}

/// 编码 CMSketch（不含 TopN）为 protobuf 字节；空 sketch 返回空 Vec。
pub fn EncodeCMSketchWithoutTopN(
    cmsketch: Option<&CMSketch>,
) -> Result<Vec<u8>, astersql_errors::SharedError> {
    let Some(cmsketch) = cmsketch else {
        return Ok(Vec::new());
    };
    CMSketchToProto(Some(cmsketch), None)
        .write_to_bytes()
        .map_err(|error| astersql_errors::New(error.to_string()))
}

/// 从存储字节与 TopN 行集解码 CMSketch 与 TopN。
pub fn DecodeCMSketchAndTopN(
    data: Option<&[u8]>,
    top_n_rows: &[chunk::Row],
) -> Result<(Option<CMSketch>, Option<TopN>), astersql_errors::SharedError> {
    if data.is_none() && top_n_rows.is_empty() {
        return Ok((None, None));
    }
    let top_n = DecodeTopN(top_n_rows);
    let Some(data) = data.filter(|data| !data.is_empty()) else {
        return Ok((None, top_n));
    };
    Ok((DecodeCMSketch(data)?, top_n))
}

/// 从 chunk 行（编码字节 + 计数）重建 TopN。
pub fn DecodeTopN(rows: &[chunk::Row]) -> Option<TopN> {
    if rows.is_empty() {
        return None;
    }
    let mut top_n = NewTopN(rows.len());
    for row in rows {
        top_n.AppendTopN(row.GetBytes(0), row.GetUint64(1));
    }
    top_n.Sort();
    Some(top_n)
}

/// 解析 protobuf 字节为 CMSketch（忽略 TopN 部分）。
pub fn DecodeCMSketch(data: &[u8]) -> Result<Option<CMSketch>, astersql_errors::SharedError> {
    if data.is_empty() {
        return Ok(None);
    }
    let proto = protobuf::parse_from_bytes::<tipb::CmSketch>(data)
        .map_err(|error| astersql_errors::New(error.to_string()))?;
    Ok(CMSketchAndTopNFromProto(Some(&proto)).0)
}

#[derive(Clone, Debug, PartialEq, Eq)]
/// 高频值精确表：按编码字节有序存放 `(Encoded, Count)`。
pub struct TopN {
    pub TopN: Vec<TopNMeta>,
}

impl fmt::Display for TopN {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "TopN{{length: {}, [", self.TopN.len())?;
        for (index, item) in self.TopN.iter().enumerate() {
            if index > 0 {
                write!(formatter, ", ")?;
            }
            write!(formatter, "({:?}, {})", item.Encoded, item.Count)?;
        }
        write!(formatter, "]}}")
    }
}

impl TopN {
    /// 调试用字符串表示。
    pub fn String(&self) -> String {
        self.to_string()
    }

    /// 追加一条 TopN 记录（调用方随后应 Sort）。
    pub fn AppendTopN(&mut self, data: Vec<u8>, count: u64) {
        self.TopN.push(TopNMeta {
            Encoded: data,
            Count: count,
        });
    }

    /// 返回 TopN 条目数。
    pub fn Num(&self) -> usize {
        self.TopN.len()
    }

    /// 深拷贝 TopN。
    pub fn Copy(&self) -> TopN {
        self.clone()
    }

    /// 用自定义解码器把 Encoded 转成可读字符串后格式化。
    pub fn DecodedString(
        &self,
        mut decode: impl FnMut(&[u8]) -> Result<String, astersql_errors::SharedError>,
    ) -> Result<String, astersql_errors::SharedError> {
        let mut entries = Vec::with_capacity(self.TopN.len());
        for item in &self.TopN {
            entries.push(format!("({}, {})", decode(&item.Encoded)?, item.Count));
        }
        Ok(format!(
            "TopN{{length: {}, [{}]}}",
            self.TopN.len(),
            entries.join(", ")
        ))
    }

    /// 返回 TopN 中的最小 Count。
    pub fn MinCount(&self) -> u64 {
        self.calculateMinCountAndCountInternal().0
    }

    /// 计算 (最小 Count, 总 Count)。
    fn calculateMinCountAndCount(&self) -> (u64, u64) {
        self.calculateMinCountAndCountInternal()
    }

    /// 与 calculateMinCountAndCount 相同（对应 Go once 语义占位）。
    fn onceCalculateMinCountAndCount(&self) -> (u64, u64) {
        self.calculateMinCountAndCountInternal()
    }

    /// 内部：一次遍历得到最小 Count 与总和。
    fn calculateMinCountAndCountInternal(&self) -> (u64, u64) {
        (
            self.TopN.iter().map(|item| item.Count).min().unwrap_or(0),
            self.TopN.iter().map(|item| item.Count).sum(),
        )
    }

    /// 精确查询 TopN：返回 (计数, 是否命中)。
    pub fn QueryTopN(&self, data: &[u8]) -> (u64, bool) {
        let index = self.FindTopN(data);
        if index < 0 {
            (0, false)
        } else {
            (self.TopN[index as usize].Count, true)
        }
    }

    /// 二分查找编码键，命中返回下标，否则 -1。
    pub fn FindTopN(&self, data: &[u8]) -> isize {
        self.TopN
            .binary_search_by(|item| item.Encoded.as_slice().cmp(data))
            .map_or(-1, |index| index as isize)
    }

    /// 返回不小于 `data` 的下界下标，以及是否精确命中。
    pub fn LowerBound(&self, data: &[u8]) -> (usize, bool) {
        match self
            .TopN
            .binary_search_by(|item| item.Encoded.as_slice().cmp(data))
        {
            Ok(index) => (index, true),
            Err(index) => (index, false),
        }
    }

    /// 统计编码落在 [lower, upper) 的 TopN 计数之和。
    pub fn BetweenCount(&self, lower: &[u8], upper: &[u8]) -> u64 {
        let (lower, _) = self.LowerBound(lower);
        let (upper, _) = self.LowerBound(upper);
        self.TopN[lower..upper].iter().map(|item| item.Count).sum()
    }

    /// 按 Encoded 字节升序排序，供二分查询。
    pub fn Sort(&mut self) {
        self.TopN
            .sort_by(|left, right| left.Encoded.cmp(&right.Encoded));
    }

    /// 返回 TopN 全部条目的 Count 之和。
    pub fn TotalCount(&self) -> u64 {
        self.onceCalculateMinCountAndCount().1
    }

    /// 两侧总计数皆为 0，或内容完全相等时视为 Equal。
    pub fn Equal(&self, other: &TopN) -> bool {
        (self.TotalCount() == 0 && other.TotalCount() == 0) || self == other
    }

    /// 估算 TopN 结构内存占用。
    pub fn MemoryUsage(&self) -> i64 {
        32 + self
            .TopN
            .iter()
            .map(|item| 32 + item.Encoded.capacity() as i64)
            .sum::<i64>()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
/// 单条 TopN 元数据：编码值与精确计数。
pub struct TopNMeta {
    pub Encoded: Vec<u8>,
    pub Count: u64,
}

/// 预分配容量创建空 TopN。
pub fn NewTopN(capacity: usize) -> TopN {
    TopN {
        TopN: Vec::with_capacity(capacity),
    }
}

/// 合并多个 TopN：同键累加后按频次截取前 `count` 条，其余作为 spill 返回。
pub fn MergeTopN(top_ns: &[&TopN], count: u32) -> (Option<TopN>, Vec<TopNMeta>) {
    if CheckEmptyTopNs(top_ns) {
        return (None, Vec::new());
    }
    let mut counter = HashMap::<Vec<u8>, u64>::new();
    for top_n in top_ns {
        for item in &top_n.TopN {
            *counter.entry(item.Encoded.clone()).or_default() += item.Count;
        }
    }
    if counter.is_empty() {
        return (None, Vec::new());
    }
    let sorted = counter
        .into_iter()
        .map(|(Encoded, Count)| TopNMeta { Encoded, Count })
        .collect();
    let (merged, spilled) = GetMergedTopNFromSortedSlice(sorted, count);
    (Some(merged), spilled)
}

/// 判断一组 TopN 是否全部为空（TotalCount 均为 0）。
pub fn CheckEmptyTopNs(top_ns: &[&TopN]) -> bool {
    top_ns.iter().all(|top_n| top_n.TotalCount() == 0)
}

/// 按 Count 降序、Encoded 升序排序 TopNMeta 切片。
pub fn SortTopnMeta(items: &mut [TopNMeta]) {
    items.sort_by(TopnMetaCompare);
}

/// TopNMeta 比较：Count 降序，相同则 Encoded 升序。
pub fn TopnMetaCompare(left: &TopNMeta, right: &TopNMeta) -> Ordering {
    right
        .Count
        .cmp(&left.Count)
        .then_with(|| left.Encoded.cmp(&right.Encoded))
}

/// 对已按比较器可排序的切片截取 TopN，并对保留部分按 Encoded 再排序。
pub fn GetMergedTopNFromSortedSlice(
    mut sorted: Vec<TopNMeta>,
    count: u32,
) -> (TopN, Vec<TopNMeta>) {
    SortTopnMeta(&mut sorted);
    let count = (count as usize).min(sorted.len());
    let mut spilled = sorted.split_off(count);
    let mut final_top_n = TopN { TopN: sorted };
    final_top_n.Sort();
    spilled.shrink_to_fit();
    (final_top_n, spilled)
}
