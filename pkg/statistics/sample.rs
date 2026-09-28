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

// 统计采样：蓄水池采样收集器、FM/CM Sketch 与 TopN 的样本侧组装及 protobuf 编解码。
//
// ANALYZE 扫描过程中用 `SampleCollector` 收集列值样本并估计 NDV（不同值个数）；
// 合并多路收集器时走 merger 路径，避免重复累计空值与 sketch。

use std::mem::size_of;

use protobuf::RepeatedField;

use crate::{
    CMSketch, CMSketchAndTopNFromProto, CMSketchToProto, FMSketch, FMSketchFromProto,
    FMSketchToProto, NewCMSketch, NewFMSketch, NewTopN, TopN,
};

#[derive(Clone)]
/// 单个采样项：列值 Datum、行句柄与出现序号。
pub struct SampleItem {
    pub Value: types::Datum,
    pub Handle: i64,
    pub Ordinal: i32,
}

/// 空 `SampleItem` 结构体本身占用的字节数（不含 Datum 载荷）。
pub const EmptySampleItemSize: i64 = size_of::<SampleItem>() as i64;

/// 按 Datum 二进制序排序采样项；比较出错时记录并返回错误。
pub fn sortSampleItems(items: &mut [SampleItem]) -> Result<(), astersql_errors::SharedError> {
    let mut error = None;
    items.sort_by(|left, right| {
        match left.Value.Compare(
            (*types::DefaultStmtNoWarningContext).clone(),
            &right.Value,
            collate::GetBinaryCollator().as_ref(),
        ) {
            Ok(value) if value < 0 => std::cmp::Ordering::Less,
            Ok(value) if value > 0 => std::cmp::Ordering::Greater,
            Ok(_) => std::cmp::Ordering::Equal,
            Err(cause) => {
                error = Some(astersql_errors::New(cause.to_string()));
                std::cmp::Ordering::Equal
            }
        }
    });
    error.map_or(Ok(()), Err)
}

#[derive(Clone)]
/// 列值采样收集器：维护蓄水池样本、FM/CM Sketch、TopN 及计数统计。
pub struct SampleCollector {
    pub FMSketch: FMSketch,
    pub CMSketch: Option<CMSketch>,
    pub TopN: Option<TopN>,
    pub Samples: Vec<SampleItem>,
    pub seenValues: i64,
    pub NullCount: i64,
    pub Count: i64,
    pub MaxSampleSize: i64,
    pub TotalSize: i64,
    pub MemSize: i64,
    pub IsMerger: bool,
    randomState: u64,
}

impl SampleCollector {
    /// 创建指定最大样本数与 FM Sketch 容量的空收集器。
    pub fn New(max_sample_size: i64, max_fm_sketch_size: usize) -> SampleCollector {
        SampleCollector {
            FMSketch: NewFMSketch(max_fm_sketch_size),
            CMSketch: None,
            TopN: None,
            Samples: Vec::with_capacity(max_sample_size.max(0) as usize),
            seenValues: 0,
            NullCount: 0,
            Count: 0,
            MaxSampleSize: max_sample_size,
            TotalSize: 0,
            MemSize: 0,
            IsMerger: false,
            randomState: 0x9e37_79b9_7f4a_7c15,
        }
    }

    /// 清空 sketch、样本与计数，释放收集器内部状态。
    pub fn Destroy(&mut self) {
        self.FMSketch = NewFMSketch(0);
        self.CMSketch = None;
        self.TopN = None;
        self.Samples.clear();
        self.seenValues = 0;
        self.NullCount = 0;
        self.Count = 0;
        self.MaxSampleSize = 0;
        self.TotalSize = 0;
        self.MemSize = 0;
        self.IsMerger = false;
    }

    /// xorshift 伪随机，供蓄水池替换决策使用。
    fn nextRandom(&mut self) -> u64 {
        let mut value = self.randomState;
        value ^= value << 13;
        value ^= value >> 7;
        value ^= value << 17;
        self.randomState = value;
        value
    }

    /// 收集一个 Datum：非 merger 时更新空值/计数/sketch；始终做蓄水池采样。
    pub fn Collect(
        &mut self,
        statement_context: &stmtctx::StatementContext,
        value: types::Datum,
    ) -> Result<(), astersql_errors::SharedError> {
        if !self.IsMerger {
            if value.IsNull() {
                self.NullCount += 1;
                return Ok(());
            }
            self.Count += 1;
            self.FMSketch
                .InsertValue(statement_context, value.clone())?;
            if let Some(cmsketch) = self.CMSketch.as_mut() {
                cmsketch.InsertBytes(&value.GetBytes());
            }
            self.TotalSize += value.GetBytes().len().saturating_sub(1) as i64;
        }
        self.seenValues += 1;
        // 蓄水池未满则直接放入；已满则以 1/seenValues 概率随机替换一项。
        if self.Samples.len() < self.MaxSampleSize.max(0) as usize {
            self.Samples.push(SampleItem {
                Value: value,
                Handle: 0,
                Ordinal: self.seenValues as i32 - 1,
            });
        } else if self.MaxSampleSize > 0
            && self.nextRandom() % (self.seenValues as u64) < self.MaxSampleSize as u64
        {
            let index = (self.nextRandom() % self.MaxSampleSize as u64) as usize;
            self.Samples.remove(index);
            self.Samples.push(SampleItem {
                Value: value,
                Handle: 0,
                Ordinal: self.seenValues as i32 - 1,
            });
        }
        self.MemSize = self.Samples.len() as i64 * EmptySampleItemSize;
        Ok(())
    }

    /// `Collect` 的内部别名，保持与 Go 侧小写方法对应。
    fn collect(
        &mut self,
        statement_context: &stmtctx::StatementContext,
        value: types::Datum,
    ) -> Result<(), astersql_errors::SharedError> {
        self.Collect(statement_context, value)
    }

    /// 合并另一收集器的计数、sketch 与样本（样本以 merger 模式再 Collect，避免重复计空值）。
    pub fn MergeSampleCollector(
        &mut self,
        statement_context: &stmtctx::StatementContext,
        other: &SampleCollector,
    ) -> Result<(), astersql_errors::SharedError> {
        self.NullCount += other.NullCount;
        self.Count += other.Count;
        self.TotalSize += other.TotalSize;
        self.FMSketch.MergeFMSketch(&other.FMSketch);
        if let (Some(destination), Some(source)) = (&mut self.CMSketch, &other.CMSketch) {
            destination.MergeCMSketch(source)?;
        }
        let old_merger = self.IsMerger;
        self.IsMerger = true;
        for item in &other.Samples {
            self.Collect(statement_context, item.Value.clone())?;
        }
        self.IsMerger = old_merger;
        Ok(())
    }

    /// 按当前样本字节长度重算 TotalSize。
    pub fn CalcTotalSize(&mut self) {
        self.TotalSize = self
            .Samples
            .iter()
            .map(|item| item.Value.GetBytes().len() as i64)
            .sum();
    }

    /// 从样本频次与 CMSketch 查询结果提取 TopN 高频值。
    pub fn ExtractTopN(&mut self, num_top: usize) {
        if num_top == 0 {
            return;
        }
        let mut counts = std::collections::HashMap::<Vec<u8>, u64>::new();
        for item in &self.Samples {
            *counts.entry(item.Value.GetBytes()).or_default() += 1;
        }
        let mut entries = counts.into_iter().collect::<Vec<_>>();
        entries.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(&right.0)));
        let mut top_n = NewTopN(num_top.min(entries.len()));
        for (data, _) in entries.into_iter().take(num_top) {
            let count = self
                .CMSketch
                .as_ref()
                .map_or(0, |cmsketch| cmsketch.QueryBytes(&data));
            if let Some(cmsketch) = self.CMSketch.as_mut() {
                let (h1, h2) = crate::murmur3Sum128(&data);
                cmsketch.SubValue(h1, h2, count);
            }
            top_n.AppendTopN(data, count);
        }
        top_n.Sort();
        self.TopN = Some(top_n);
    }
}

/// 将采样收集器序列化为 tipb protobuf（含 FM/CM Sketch 与样本字节）。
pub fn SampleCollectorToProto(collector: &SampleCollector) -> tipb::SampleCollector {
    let mut result = tipb::SampleCollector::new();
    result.set_null_count(collector.NullCount);
    result.set_count(collector.Count);
    result.set_fm_sketch(FMSketchToProto(Some(&collector.FMSketch)));
    result.set_total_size(collector.TotalSize);
    result.set_samples(RepeatedField::from_vec(
        collector
            .Samples
            .iter()
            .map(|item| item.Value.GetBytes())
            .collect(),
    ));
    if let Some(cmsketch) = collector.CMSketch.as_ref() {
        result.set_cm_sketch(CMSketchToProto(Some(cmsketch), None));
    }
    result
}

/// 样本值长度上限：超过则在 FromProto 时丢弃，防止异常大载荷。
pub const MaxSampleValueLength: usize = 32_767;

/// 从 tipb protobuf 还原采样收集器；过长样本会被过滤。
pub fn SampleCollectorFromProto(proto: &tipb::SampleCollector) -> SampleCollector {
    let mut result = SampleCollector::New(0, crate::MaxSketchSize);
    result.NullCount = proto.get_null_count();
    result.Count = proto.get_count();
    result.TotalSize = proto.get_total_size();
    if proto.has_fm_sketch() {
        result.FMSketch = FMSketchFromProto(Some(proto.get_fm_sketch()))
            .unwrap_or_else(|| NewFMSketch(crate::MaxSketchSize));
    }
    if proto.has_cm_sketch() {
        let (cmsketch, top_n) = CMSketchAndTopNFromProto(Some(proto.get_cm_sketch()));
        result.CMSketch = cmsketch;
        result.TopN = top_n;
    }
    result.Samples = proto
        .get_samples()
        .iter()
        .filter(|value| value.len() <= MaxSampleValueLength)
        .map(|value| SampleItem {
            Value: types::NewBytesDatum(value.clone()),
            Handle: 0,
            Ordinal: 0,
        })
        .collect();
    result
}

#[derive(Clone, Debug)]
/// 按列批量采样的构建参数：样本上限、FM/CM Sketch 尺寸。
pub struct SampleBuilder {
    pub MaxSampleSize: i64,
    pub MaxFMSketchSize: usize,
    pub CMSketchDepth: i32,
    pub CMSketchWidth: i32,
}

impl SampleBuilder {
    /// 对多行多列数据逐列 Collect，返回每列一个 `SampleCollector`。
    pub fn CollectColumnStats(
        &self,
        statement_context: &stmtctx::StatementContext,
        rows: Vec<Vec<types::Datum>>,
    ) -> Result<Vec<SampleCollector>, astersql_errors::SharedError> {
        let column_count = rows.first().map_or(0, Vec::len);
        let mut collectors = (0..column_count)
            .map(|_| {
                let mut collector = SampleCollector::New(self.MaxSampleSize, self.MaxFMSketchSize);
                collector.CMSketch = Some(NewCMSketch(self.CMSketchDepth, self.CMSketchWidth));
                collector
            })
            .collect::<Vec<_>>();
        for row in rows {
            for (collector, value) in collectors.iter_mut().zip(row) {
                collector.Collect(statement_context, value)?;
            }
        }
        Ok(collectors)
    }
}

/// 将 chunk 行按字段类型抽出为 Datum 向量。
pub fn RowToDatums(row: &chunk::Row, field_types: &[types::FieldType]) -> Vec<types::Datum> {
    field_types
        .iter()
        .enumerate()
        .map(|(index, field_type)| row.GetDatum(index, field_type))
        .collect()
}
