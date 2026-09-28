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

// 行级采样收集器：水库抽样与伯努利抽样，并维护列/列组 FMSketch 与空值计数。
//
// ANALYZE 在 TiKV 侧按行采样后序列化为 tipb，再在 TiDB 侧合并构建直方图与 NDV。

use std::mem::size_of;

use protobuf::RepeatedField;

use crate::{FMSketch, FMSketchFromProto, FMSketchToProto, NewFMSketch};

/// 行采样收集器接口：合并、采样单行、访问基础字段。
pub trait RowSampleCollector {
    /// 合并另一收集器的样本与统计。
    fn MergeCollector(&mut self, collector: &dyn RowSampleCollector);
    /// 以给定权重采样一行 Datum。
    fn SampleRow(&mut self, row: Vec<types::Datum>, weight: i64);
    /// 借用底层 baseCollector。
    fn Base(&self) -> &baseCollector;
    /// 可变借用底层 baseCollector。
    fn BaseMut(&mut self) -> &mut baseCollector;
}

#[derive(Clone)]
/// 采样公共状态：样本堆、空值计数、FMSketch、列总字节与行数。
pub struct baseCollector {
    pub Samples: WeightedRowSampleHeap,
    pub NullCount: Vec<i64>,
    pub FMSketches: Vec<FMSketch>,
    pub TotalSizes: Vec<i64>,
    pub Count: i64,
    pub MemSize: i64,
}

impl baseCollector {
    /// `CollectColumns` 的小写别名。
    pub fn collectColumns(
        &mut self,
        statement_context: &stmtctx::StatementContext,
        columns: &[types::Datum],
    ) -> Result<(), astersql_errors::SharedError> {
        self.CollectColumns(statement_context, columns)
    }

    /// 累计非空列值到 FMSketch，并统计空值与字节总长。
    pub fn CollectColumns(
        &mut self,
        statement_context: &stmtctx::StatementContext,
        columns: &[types::Datum],
    ) -> Result<(), astersql_errors::SharedError> {
        for (index, value) in columns.iter().enumerate() {
            if value.IsNull() {
                self.NullCount[index] += 1;
                continue;
            }
            self.FMSketches[index].InsertValue(statement_context, value.clone())?;
            // Go receives the original encoded datum size and excludes its flag byte.
            self.TotalSizes[index] += value.GetBytes().len().saturating_sub(1) as i64;
        }
        Ok(())
    }

    /// 对列组编码后写入对应 FMSketch；任一分量为 NULL 则计空值。
    pub fn collectColumnGroups(
        &mut self,
        statement_context: &stmtctx::StatementContext,
        columns: &[types::Datum],
        column_groups: &[Vec<usize>],
    ) -> Result<(), astersql_errors::SharedError> {
        let offset = columns.len();
        for (group_index, group) in column_groups.iter().enumerate() {
            // A single-column group is copied from its source column after collection.
            if group.len() == 1 {
                continue;
            }
            let values = group
                .iter()
                .map(|index| columns[*index].clone())
                .collect::<Vec<_>>();
            for value in &values {
                if !value.IsNull() {
                    self.TotalSizes[offset + group_index] +=
                        value.GetBytes().len().saturating_sub(1) as i64;
                }
            }
            self.FMSketches[offset + group_index].InsertRowValue(statement_context, &values)?;
        }
        Ok(())
    }

    /// 清空内容以便回收（对应 Go 对象池归还）。
    pub fn destroyAndPutToPool(&mut self) {
        // Go releases only the sketches; the collector fields remain observable.
        self.FMSketches.clear();
    }

    /// 序列化为 tipb 行采样收集器。
    pub fn ToProto(&self) -> tipb::RowSampleCollector {
        let mut result = tipb::RowSampleCollector::new();
        result.set_samples(RepeatedField::from_vec(RowSamplesToProto(&self.Samples)));
        result.set_null_counts(self.NullCount.clone());
        result.set_count(self.Count);
        result.set_fm_sketch(RepeatedField::from_vec(
            self.FMSketches
                .iter()
                .map(|sketch| FMSketchToProto(Some(sketch)))
                .collect(),
        ));
        result.set_total_size(self.TotalSizes.clone());
        result
    }

    /// 从 tipb 反序列化。
    pub fn FromProto(proto: &tipb::RowSampleCollector) -> baseCollector {
        let samples: WeightedRowSampleHeap = proto
            .get_samples()
            .iter()
            .map(|sample| ReservoirRowSampleItem {
                Handle: 0,
                Columns: sample
                    .get_row()
                    .iter()
                    .map(|value| types::NewBytesDatum(value.clone()))
                    .collect(),
                Weight: sample.get_weight(),
            })
            .collect();
        let mem_size = samples.iter().map(ReservoirRowSampleItem::MemUsage).sum();
        baseCollector {
            Samples: samples,
            NullCount: proto.get_null_counts().to_vec(),
            FMSketches: proto
                .get_fm_sketch()
                .iter()
                .filter_map(|sketch| FMSketchFromProto(Some(sketch)))
                .collect(),
            TotalSizes: proto.get_total_size().to_vec(),
            Count: proto.get_count(),
            MemSize: mem_size,
        }
    }
}

#[derive(Clone)]
/// 水库（加权）行采样：固定容量，权重大于堆顶则替换。
pub struct ReservoirRowSampleCollector {
    pub baseCollector: baseCollector,
    pub MaxSampleSize: usize,
}

#[derive(Clone)]
/// 一条带权重的行样本。
pub struct ReservoirRowSampleItem {
    pub Handle: i64,
    pub Columns: Vec<types::Datum>,
    pub Weight: i64,
}

/// 空样本项结构体固定大小。
pub const EmptyReservoirSampleItemSize: i64 = size_of::<ReservoirRowSampleItem>() as i64;

impl ReservoirRowSampleItem {
    /// 样本项内存占用（结构体 + 各列 Datum）。
    pub fn MemUsage(&self) -> i64 {
        EmptyReservoirSampleItemSize + self.Columns.iter().map(types::Datum::MemUsage).sum::<i64>()
    }
}

#[derive(Clone, Default)]
/// 按 Weight 维护的样本堆（实现上为排序 Vec）。
pub struct WeightedRowSampleHeap(pub Vec<ReservoirRowSampleItem>);

/// 解引用到内部 Vec。
impl std::ops::Deref for WeightedRowSampleHeap {
    type Target = Vec<ReservoirRowSampleItem>;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

/// 可变解引用到内部 Vec。
impl std::ops::DerefMut for WeightedRowSampleHeap {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

/// 由迭代器收集构造。
impl FromIterator<ReservoirRowSampleItem> for WeightedRowSampleHeap {
    fn from_iter<T: IntoIterator<Item = ReservoirRowSampleItem>>(iterator: T) -> Self {
        WeightedRowSampleHeap(iterator.into_iter().collect())
    }
}

/// 消费型迭代。
impl IntoIterator for WeightedRowSampleHeap {
    type Item = ReservoirRowSampleItem;
    type IntoIter = std::vec::IntoIter<ReservoirRowSampleItem>;
    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

/// 借用型迭代。
impl<'a> IntoIterator for &'a WeightedRowSampleHeap {
    type Item = &'a ReservoirRowSampleItem;
    type IntoIter = std::slice::Iter<'a, ReservoirRowSampleItem>;
    fn into_iter(self) -> Self::IntoIter {
        self.0.iter()
    }
}

impl WeightedRowSampleHeap {
    /// 样本个数。
    pub fn Len(&self) -> usize {
        self.len()
    }
    /// 交换两位置。
    pub fn Swap(&mut self, left: usize, right: usize) {
        self.swap(left, right);
    }
    /// 按 Weight 比较（小顶堆序）。
    pub fn Less(&self, left: usize, right: usize) -> bool {
        self[left].Weight < self[right].Weight
    }
    /// 推入后按 Weight 排序。
    pub fn Push(&mut self, item: ReservoirRowSampleItem) {
        self.push(item);
        self.sort_by_key(|item| item.Weight);
    }
    /// 弹出末尾元素。
    pub fn Pop(&mut self) -> Option<ReservoirRowSampleItem> {
        self.pop()
    }
}

/// 创建指定容量与 FMSketch 尺寸的水库采样收集器。
pub fn NewReservoirRowSampleCollector(
    max_sample_size: usize,
    total_length: usize,
    max_fm_sketch_size: usize,
) -> ReservoirRowSampleCollector {
    ReservoirRowSampleCollector {
        baseCollector: baseCollector {
            Samples: WeightedRowSampleHeap(Vec::with_capacity(max_sample_size)),
            NullCount: vec![0; total_length],
            FMSketches: (0..total_length)
                .map(|_| NewFMSketch(max_fm_sketch_size))
                .collect(),
            TotalSizes: vec![0; total_length],
            Count: 0,
            MemSize: 0,
        },
        MaxSampleSize: max_sample_size,
    }
}

impl RowSampleCollector for ReservoirRowSampleCollector {
    /// 合并另一收集器的计数/草图，并按权重重采样其样本行。
    fn MergeCollector(&mut self, collector: &dyn RowSampleCollector) {
        let other = collector.Base();
        assert_eq!(self.baseCollector.NullCount.len(), other.NullCount.len());
        assert_eq!(self.baseCollector.TotalSizes.len(), other.TotalSizes.len());
        assert_eq!(self.baseCollector.FMSketches.len(), other.FMSketches.len());
        let old_sample_count = self.baseCollector.Samples.len();
        let old_mem_size = self.baseCollector.MemSize;
        self.baseCollector.Count += other.Count;
        for (destination, source) in self
            .baseCollector
            .NullCount
            .iter_mut()
            .zip(&other.NullCount)
        {
            *destination += source;
        }
        for (destination, source) in self
            .baseCollector
            .TotalSizes
            .iter_mut()
            .zip(&other.TotalSizes)
        {
            *destination += source;
        }
        for (destination, source) in self
            .baseCollector
            .FMSketches
            .iter_mut()
            .zip(&other.FMSketches)
        {
            destination.MergeFMSketch(source);
        }
        for sample in &other.Samples {
            self.SampleRow(sample.Columns.clone(), sample.Weight);
        }
        let total_sample_count = old_sample_count + other.Samples.len();
        self.baseCollector.MemSize = if total_sample_count == 0 {
            0
        } else {
            (old_mem_size + other.MemSize) * self.baseCollector.Samples.len() as i64
                / total_sample_count as i64
        };
    }

    /// 未满则加入；已满且权重大于当前最小则替换堆顶。
    fn SampleRow(&mut self, row: Vec<types::Datum>, weight: i64) {
        let sample = ReservoirRowSampleItem {
            Handle: 0,
            Columns: row,
            Weight: weight,
        };
        if self.baseCollector.Samples.len() < self.MaxSampleSize {
            self.baseCollector.Samples.push(sample);
            self.baseCollector
                .Samples
                .sort_by_key(|sample| sample.Weight);
        } else if self.MaxSampleSize > 0 && weight > self.baseCollector.Samples[0].Weight {
            self.baseCollector.MemSize -= self.baseCollector.Samples[0].MemUsage();
            self.baseCollector.Samples[0] = sample;
            self.baseCollector
                .Samples
                .sort_by_key(|sample| sample.Weight);
        }
        self.baseCollector.MemSize = self
            .baseCollector
            .Samples
            .iter()
            .map(ReservoirRowSampleItem::MemUsage)
            .sum();
    }

    fn Base(&self) -> &baseCollector {
        &self.baseCollector
    }

    fn BaseMut(&mut self) -> &mut baseCollector {
        &mut self.baseCollector
    }
}

impl ReservoirRowSampleCollector {
    /// 小写别名。
    pub fn sampleRow(&mut self, row: Vec<types::Datum>, weight: i64) {
        self.SampleRow(row, weight);
    }

    /// 对已打包的样本项再采样。
    pub fn sampleZippedRow(&mut self, sample: ReservoirRowSampleItem) {
        self.SampleRow(sample.Columns, sample.Weight);
    }

    /// 清空以便归还对象池。
    pub fn DestroyAndPutToPool(&mut self) {
        // Go only releases sketches here; the remaining fields belong to the collector.
        self.baseCollector.FMSketches.clear();
    }
}

#[derive(Clone)]
/// 伯努利行采样：按 SampleRate 独立决定是否保留每行。
pub struct BernoulliRowSampleCollector {
    pub baseCollector: baseCollector,
    pub SampleRate: f64,
}

/// 创建给定采样率的伯努利收集器。
pub fn NewBernoulliRowSampleCollector(
    sample_rate: f64,
    total_length: usize,
    max_fm_sketch_size: usize,
) -> BernoulliRowSampleCollector {
    BernoulliRowSampleCollector {
        baseCollector: baseCollector {
            Samples: WeightedRowSampleHeap::default(),
            NullCount: vec![0; total_length],
            FMSketches: (0..total_length)
                .map(|_| NewFMSketch(max_fm_sketch_size))
                .collect(),
            TotalSizes: vec![0; total_length],
            Count: 0,
            MemSize: 0,
        },
        SampleRate: sample_rate,
    }
}

impl RowSampleCollector for BernoulliRowSampleCollector {
    /// 合并计数/草图并拼接样本列表。
    fn MergeCollector(&mut self, collector: &dyn RowSampleCollector) {
        let other = collector.Base();
        assert_eq!(self.baseCollector.NullCount.len(), other.NullCount.len());
        assert_eq!(self.baseCollector.TotalSizes.len(), other.TotalSizes.len());
        assert_eq!(self.baseCollector.FMSketches.len(), other.FMSketches.len());
        self.baseCollector.Count += other.Count;
        self.baseCollector.Samples.extend(other.Samples.clone());
        for (destination, source) in self
            .baseCollector
            .NullCount
            .iter_mut()
            .zip(&other.NullCount)
        {
            *destination += source;
        }
        for (destination, source) in self
            .baseCollector
            .TotalSizes
            .iter_mut()
            .zip(&other.TotalSizes)
        {
            *destination += source;
        }
        for (destination, source) in self
            .baseCollector
            .FMSketches
            .iter_mut()
            .zip(&other.FMSketches)
        {
            destination.MergeFMSketch(source);
        }
        self.baseCollector.MemSize += other.MemSize;
    }

    /// 用权重哈希映射到 [0,1)，小于 SampleRate 则保留。
    fn SampleRow(&mut self, row: Vec<types::Datum>, weight: i64) {
        let normalized = (weight as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15);
        let probability = normalized as f64 / u64::MAX as f64;
        if probability <= self.SampleRate {
            self.baseCollector.Samples.push(ReservoirRowSampleItem {
                Handle: 0,
                Columns: row,
                Weight: weight,
            });
        }
    }

    fn Base(&self) -> &baseCollector {
        &self.baseCollector
    }

    fn BaseMut(&mut self) -> &mut baseCollector {
        &mut self.baseCollector
    }
}

impl BernoulliRowSampleCollector {
    /// 小写别名。
    pub fn sampleRow(&mut self, row: Vec<types::Datum>, weight: i64) {
        self.SampleRow(row, weight);
    }

    /// 清空以便归还对象池。
    pub fn DestroyAndPutToPool(&mut self) {
        // Go only releases sketches here; the remaining fields belong to the collector.
        self.baseCollector.FMSketches.clear();
    }
}

/// 行采样收集器枚举：水库或伯努利。
pub enum RowSampleCollectorKind {
    Reservoir(ReservoirRowSampleCollector),
    Bernoulli(BernoulliRowSampleCollector),
}

/// 按 max_sample_size / sample_rate 选择水库或伯努利收集器。
pub fn NewRowSampleCollector(
    max_sample_size: usize,
    sample_rate: f64,
    total_length: usize,
    max_fm_sketch_size: usize,
) -> Option<RowSampleCollectorKind> {
    if max_sample_size > 0 {
        Some(RowSampleCollectorKind::Reservoir(
            NewReservoirRowSampleCollector(max_sample_size, total_length, max_fm_sketch_size),
        ))
    } else if sample_rate > 0.0 {
        Some(RowSampleCollectorKind::Bernoulli(
            NewBernoulliRowSampleCollector(sample_rate, total_length, max_fm_sketch_size),
        ))
    } else {
        None
    }
}

/// 将样本列表转为 tipb::RowSample。
pub fn RowSamplesToProto(samples: &[ReservoirRowSampleItem]) -> Vec<tipb::RowSample> {
    samples
        .iter()
        .map(|sample| {
            let mut result = tipb::RowSample::new();
            result.set_row(RepeatedField::from_vec(
                sample
                    .Columns
                    .iter()
                    .map(|column| {
                        if column.IsNull() {
                            vec![codec::NilFlag]
                        } else {
                            column.GetBytes()
                        }
                    })
                    .collect(),
            ));
            result.set_weight(sample.Weight);
            result
        })
        .collect()
}

#[derive(Clone)]
/// 行采样构建参数：列组、容量/采样率与 FMSketch 尺寸。
pub struct RowSampleBuilder {
    pub ColGroups: Vec<Vec<usize>>,
    pub MaxSampleSize: usize,
    pub SampleRate: f64,
    pub MaxFMSketchSize: usize,
}

impl RowSampleBuilder {
    /// 对输入行逐行收集列/列组草图并采样。
    pub fn Collect(
        &self,
        statement_context: &stmtctx::StatementContext,
        rows: Vec<Vec<types::Datum>>,
    ) -> Result<Option<RowSampleCollectorKind>, astersql_errors::SharedError> {
        let columns = rows.first().map_or(0, Vec::len);
        let total_length = columns + self.ColGroups.len();
        let mut collector = NewRowSampleCollector(
            self.MaxSampleSize,
            self.SampleRate,
            total_length,
            self.MaxFMSketchSize,
        );
        // 用行序号派生伪随机权重，驱动水库/伯努利决策。
        for (index, row) in rows.into_iter().enumerate() {
            let weight = ((index as u64 + 1).wrapping_mul(0x9e37_79b9_7f4a_7c15) >> 1) as i64;
            match collector.as_mut() {
                Some(RowSampleCollectorKind::Reservoir(value)) => {
                    value.BaseMut().Count += 1;
                    value.BaseMut().CollectColumns(statement_context, &row)?;
                    value.BaseMut().collectColumnGroups(
                        statement_context,
                        &row,
                        &self.ColGroups,
                    )?;
                    value.SampleRow(row, weight);
                }
                Some(RowSampleCollectorKind::Bernoulli(value)) => {
                    value.BaseMut().Count += 1;
                    value.BaseMut().CollectColumns(statement_context, &row)?;
                    value.BaseMut().collectColumnGroups(
                        statement_context,
                        &row,
                        &self.ColGroups,
                    )?;
                    value.SampleRow(row, weight);
                }
                None => break,
            }
        }
        if let Some(collector) = collector.as_mut() {
            let base = match collector {
                RowSampleCollectorKind::Reservoir(value) => value.BaseMut(),
                RowSampleCollectorKind::Bernoulli(value) => value.BaseMut(),
            };
            for (group_index, group) in self.ColGroups.iter().enumerate() {
                if group.len() != 1 {
                    continue;
                }
                let column_index = group[0];
                let target_index = columns + group_index;
                base.FMSketches[target_index] = base.FMSketches[column_index].Copy();
                base.NullCount[target_index] = base.NullCount[column_index];
                base.TotalSizes[target_index] = base.TotalSizes[column_index];
            }
        }
        Ok(collector)
    }
}
