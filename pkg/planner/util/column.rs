// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
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

// 索引元信息到表达式列的映射工具。
//
// 将 `IndexInfo` 的索引列解析为规划器侧的 `expression::Column`，区分前缀列
// （连续可解析前缀）与完整列列表（缺失列用 None 占位），并处理前缀索引长度。

/// 按索引列名在表列信息中定位，并标记是否为前缀列（IsPrefix）。
fn indexCol2Col(
    column_infos: &[model::ColumnInfo],
    columns: &[expression::Column],
    index_column: &model::IndexColumn,
) -> Option<expression::Column> {
    column_infos
        .iter()
        .position(|info| info.Name.L == index_column.Name.L)
        .map(|index| {
            let mut column = columns[index].clone();
            // 索引定义了前缀长度且小于列完整长度时，标记为前缀索引列。
            if index_column.Length > 0
                && column_infos[index].FieldType.GetFlen() > index_column.Length
            {
                column.IsPrefix = true;
            }
            column
        })
}

/// 控制 `indexInfo2ColsImpl` 提取前缀列、完整列或两者。
#[derive(Clone, Copy)]
struct IndexInfo2ColsFlags(u8);

/// 提取连续可解析的前缀列。
const extractPrefixCols: IndexInfo2ColsFlags = IndexInfo2ColsFlags(1);
/// 提取完整索引列列表（缺失处填 None）。
const extractFullCols: IndexInfo2ColsFlags = IndexInfo2ColsFlags(2);

/// (前缀列, 前缀长度, 完整列 Option 列表, 完整长度)。
type IndexColumns = (
    Vec<expression::Column>,
    Vec<isize>,
    Vec<Option<expression::Column>>,
    Vec<isize>,
);

/// 按 flags 将 IndexInfo 映射为表达式列与长度；遇缺失列则截断前缀、完整侧填 None。
fn indexInfo2ColsImpl(
    column_infos: &[model::ColumnInfo],
    columns: &[expression::Column],
    index: &model::IndexInfo,
    flags: IndexInfo2ColsFlags,
) -> IndexColumns {
    assert_ne!(
        flags.0, 0,
        "at least one of indexInfo2ColsFlags must be set"
    );
    let wants_prefix = flags.0 & extractPrefixCols.0 != 0;
    let wants_full = flags.0 & extractFullCols.0 != 0;
    let mut prefix_columns = Vec::with_capacity(if wants_prefix { index.Columns.len() } else { 0 });
    let mut prefix_lengths = Vec::with_capacity(prefix_columns.capacity());
    let mut full_columns = Vec::with_capacity(if wants_full { index.Columns.len() } else { 0 });
    let mut full_lengths = Vec::with_capacity(full_columns.capacity());
    let mut prefix_complete = false;

    for index_column in &index.Columns {
        let Some(column) = indexCol2Col(column_infos, columns, index_column) else {
            // 列不在当前 Schema：前缀截断；若只要前缀则提前结束。
            prefix_complete = true;
            if !wants_full {
                break;
            }
            full_columns.push(None);
            full_lengths.push(-1);
            continue;
        };

        let mut length = index_column.Length;
        let field_length = column
            .RetType
            .as_ref()
            .map_or(-1, |field_type| field_type.GetFlen());
        // 前缀长度等于列完整长度时视为非前缀（Length = -1）。
        if length != -1 && length == field_length {
            length = -1;
        }
        if wants_prefix && !prefix_complete {
            prefix_columns.push(column.clone());
            prefix_lengths.push(length);
        }
        if wants_full {
            full_columns.push(Some(column));
            full_lengths.push(length);
        }
    }
    (prefix_columns, prefix_lengths, full_columns, full_lengths)
}

/// 仅返回索引的连续前缀列及其长度。
pub fn IndexInfo2PrefixCols(
    column_infos: &[model::ColumnInfo],
    columns: &[expression::Column],
    index: &model::IndexInfo,
) -> (Vec<expression::Column>, Vec<isize>) {
    let (columns, lengths, _, _) =
        indexInfo2ColsImpl(column_infos, columns, index, extractPrefixCols);
    (columns, lengths)
}

/// 返回完整索引列列表（缺失为 None）及其长度。
pub fn IndexInfo2FullCols(
    column_infos: &[model::ColumnInfo],
    columns: &[expression::Column],
    index: &model::IndexInfo,
) -> (Vec<Option<expression::Column>>, Vec<isize>) {
    let (_, _, columns, lengths) =
        indexInfo2ColsImpl(column_infos, columns, index, extractFullCols);
    (columns, lengths)
}

/// 同时返回前缀列与完整列结果。
pub fn IndexInfo2Cols(
    column_infos: &[model::ColumnInfo],
    columns: &[expression::Column],
    index: &model::IndexInfo,
) -> IndexColumns {
    indexInfo2ColsImpl(
        column_infos,
        columns,
        index,
        IndexInfo2ColsFlags(extractPrefixCols.0 | extractFullCols.0),
    )
}
