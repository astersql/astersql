// Copyright 2024 PingCAP, Inc.
// Copyright 2026 AsterSQL.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//	http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// 统计导出用 JSON 对象：表/列/谓词列结构及内存占用估算。
//
// 用于 dump/load 统计信息（含分区表全局统计），字段布局对齐 Go 侧 JSON 序列化。

#![allow(non_snake_case, non_upper_case_globals)]

use protobuf::Message as _;
use tipb;

// TiDBGlobalStats represents the global-stats for a partitioned table.
/// 分区表全局统计在 JSON 中的分区名占位键。
pub const TiDBGlobalStats: &str = "global";

// JSONTable is used for dumping statistics.
/// 导出统计用的表级 JSON 结构：列、索引、分区、谓词列与行数版本元数据。
pub struct JSONTable {
    pub Columns: std::collections::HashMap<String, Box<JSONColumn>>,
    pub Indices: std::collections::HashMap<String, Box<JSONColumn>>,
    pub Partitions: std::collections::HashMap<String, Box<JSONTable>>,
    pub DatabaseName: String,
    pub TableName: String,
    pub PredicateColumns: Vec<Box<JSONPredicateColumn>>,
    pub Count: i64,
    pub ModifyCount: i64,
    pub Version: u64,
    pub IsHistoricalStats: bool,
}

impl JSONTable {
    // Sort is used to sort the object in the JSONTable. it is used for testing to avoid flaky test.
    // Go 使用 slices.SortFunc 按 ID 升序排序；这里保留同一稳定输出目的。
    /// 按谓词列 ID 排序，保证测试输出稳定、避免 flaky。
    pub fn Sort(&mut self) {
        self.PredicateColumns.sort_by(|a, b| a.ID.cmp(&b.ID));
    }
}

// JSONColumn is used for dumping statistics.
/// 导出用列/索引统计：直方图、CM/FM Sketch、版本与空值等元数据。
pub struct JSONColumn {
    pub Histogram: Option<Box<tipb::Histogram>>,
    pub CMSketch: Option<Box<tipb::CmSketch>>,
    pub FMSketch: Option<Box<tipb::FmSketch>>,
    // StatsVer is a pointer here since the old version json file would not contain version information.
    /// 统计版本；旧 JSON 可能缺失，故用 Option。
    pub StatsVer: Option<i64>,
    pub NullCount: i64,
    pub TotColSize: i64,
    pub LastUpdateVersion: u64,
    pub Correlation: f64,
}

impl JSONColumn {
    // TotalMemoryUsage returns the total memory usage of this column.
    /// 汇总直方图与各 Sketch 的 protobuf 序列化尺寸作为内存占用近似。
    pub fn TotalMemoryUsage(&self) -> i64 {
        let mut size = 0_i64;
        if let Some(histogram) = &self.Histogram {
            size += histogram.compute_size() as i64;
        }
        if let Some(cm_sketch) = &self.CMSketch {
            size += cm_sketch.compute_size() as i64;
        }
        if let Some(fm_sketch) = &self.FMSketch {
            size += fm_sketch.compute_size() as i64;
        }
        size
    }
}

// JSONPredicateColumn contains the information of the columns used in the predicate.
/// 谓词列元数据：最近使用/分析时间与列 ID。
pub struct JSONPredicateColumn {
    pub LastUsedAt: Option<String>,
    pub LastAnalyzedAt: Option<String>,
    pub ID: i64,
}
