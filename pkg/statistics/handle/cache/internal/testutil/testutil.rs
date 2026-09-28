// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
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

// 统计信息表（`Table`）的测试用 mock 构造工具。
//
// 用于在缓存相关单测中快速生成带列/索引直方图、CMSketch、TopN 等可选载荷的
// 统计表，并支持向已有表追加列或索引。直方图（Histogram）描述列/索引值分布；
// CMSketch 用于基数估计；TopN 记录最高频取值。

use statistics::{Column, ColumnInfo, Histogram, Index, IndexInfo, Table};

/// 构造 BLOB 字段类型，作为 mock 直方图的默认列类型。
fn blob_type() -> types::FieldType {
    *types::NewFieldType(mysql::r#type::TypeBlob)
}

/// 按开关生成直方图：Go helper 的 Histogram ID 固定为 0；禁用时保持零值类型。
fn mock_histogram(enabled: bool) -> Histogram {
    if enabled {
        statistics::NewHistogram(0, 10, 0, 0, &blob_type(), 1, 0)
    } else {
        statistics::NewHistogram(0, 0, 0, 0, &types::FieldType::default(), 0, 0)
    }
}

/// 按开关生成 TopN：启用时追加一条空编码、频次为 1 的条目。
fn mock_top_n(enabled: bool) -> Option<statistics::TopN> {
    enabled.then(|| {
        let mut top_n = statistics::NewTopN(1);
        top_n.AppendTopN(Vec::new(), 1);
        top_n
    })
}

/// Creates a statistics table with the same optional payloads as the Go test helper.
///
/// 创建带指定列数、索引数的 mock 统计表；`withCMS`/`withTopN`/`withHist`
/// 分别控制是否附带 CMSketch、TopN、非空直方图，语义与 Go 侧测试辅助一致。
pub fn NewMockStatisticsTable(
    columns: i32,
    indices: i32,
    withCMS: bool,
    withTopN: bool,
    withHist: bool,
) -> Table {
    let mut table = Table::New(0, 0, 0);
    // 按列 ID 1..=columns 写入列统计，可选载荷由开关决定。
    for id in 1..=i64::from(columns) {
        table.HistColl.SetCol(
            id,
            Box::new(Column {
                CMSketch: withCMS.then(|| statistics::NewCMSketch(1, 1)),
                TopN: mock_top_n(withTopN),
                FMSketch: None,
                Info: Some(ColumnInfo {
                    ID: id,
                    Name: String::new(),
                    FieldType: types::FieldType::default(),
                    IsPrimaryKey: false,
                }),
                Histogram: mock_histogram(withHist),
                StatsLoadedStatus: statistics::NewStatsFullLoadStatus(),
                PhysicalID: 0,
                StatsVer: 0,
                IsHandle: false,
            }),
        );
    }
    // 按索引 ID 1..=indices 写入索引统计，载荷开关与列侧相同。
    for id in 1..=i64::from(indices) {
        table.HistColl.SetIdx(
            id,
            Box::new(Index {
                CMSketch: withCMS.then(|| statistics::NewCMSketch(1, 1)),
                TopN: mock_top_n(withTopN),
                FMSketch: None,
                Info: Some(IndexInfo {
                    ID: id,
                    Name: String::new(),
                    Columns: Vec::new(),
                    MVIndex: false,
                    Unique: false,
                    ConditionExprString: String::new(),
                }),
                Histogram: mock_histogram(withHist),
                StatsLoadedStatus: statistics::NewStatsFullLoadStatus(),
                PhysicalID: 0,
                StatsVer: 0,
            }),
        );
    }
    table
}

/// 向 mock 表追加一列：ID 为当前列数 + 1，默认带 CMSketch、无 TopN/直方图。
pub fn MockTableAppendColumn(table: &mut Table) {
    let id = table.ColNum() as i64 + 1;
    table.HistColl.SetCol(
        id,
        Box::new(Column {
            CMSketch: Some(statistics::NewCMSketch(1, 1)),
            TopN: None,
            FMSketch: None,
            Info: Some(ColumnInfo {
                ID: id,
                Name: String::new(),
                FieldType: types::FieldType::default(),
                IsPrimaryKey: false,
            }),
            Histogram: mock_histogram(false),
            StatsLoadedStatus: Default::default(),
            PhysicalID: 0,
            StatsVer: 0,
            IsHandle: false,
        }),
    );
}

/// 向 mock 表追加一个索引：ID 为当前索引数 + 1，默认带 CMSketch、无 TopN/直方图。
pub fn MockTableAppendIndex(table: &mut Table) {
    let id = table.IdxNum() as i64 + 1;
    table.HistColl.SetIdx(
        id,
        Box::new(Index {
            CMSketch: Some(statistics::NewCMSketch(1, 1)),
            TopN: None,
            FMSketch: None,
            Info: Some(IndexInfo {
                ID: id,
                Name: String::new(),
                Columns: Vec::new(),
                MVIndex: false,
                Unique: false,
                ConditionExprString: String::new(),
            }),
            Histogram: mock_histogram(false),
            StatsLoadedStatus: Default::default(),
            PhysicalID: 0,
            StatsVer: 0,
        }),
    );
}
