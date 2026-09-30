// Copyright 2026 AsterSQL.
// Copyright 2018 PingCAP, Inc.
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

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use expression_dependency::BuildContext;
use kv_dependency::Handle;
use model_dependency::TableInfo;
use types_dependency::datum::{self as types, Datum};

use crate::{Column, GetChangingColVal, GetColDefaultValue, Table};

/// Decode a stored row, restoring handle columns and schema-change defaults.
pub fn DecodeRawRowData(
    ctx: &dyn BuildContext,
    table: &dyn Table,
    handle: &dyn Handle,
    columns: &[Arc<Column>],
    value: &[u8],
) -> Result<(Vec<Datum>, HashMap<i64, Datum>), types::errors::Error> {
    DecodeRawRowDataWithMeta(
        ctx,
        table.Meta(),
        table.UseNewCollate(),
        handle,
        columns,
        value,
    )
}

/// Metadata-level form used by table implementations and focused row tests.
pub fn DecodeRawRowDataWithMeta(
    ctx: &dyn BuildContext,
    meta: &TableInfo,
    use_new_collation: bool,
    handle: &dyn Handle,
    columns: &[Arc<Column>],
    value: &[u8],
) -> Result<(Vec<Datum>, HashMap<i64, Datum>), types::errors::Error> {
    let mut result = vec![Datum::default(); columns.len()];
    let mut column_types = HashMap::with_capacity(columns.len());
    let mut prefix_columns = HashSet::new();
    for (position, column) in columns.iter().enumerate() {
        let info = &column.ColumnInfo;
        if column.IsPKHandleColumn(meta) {
            result[position] = if types::mysql::HasUnsignedFlag(column.GetFlag()) {
                types::NewUintDatum(handle.IntValue() as u64)
            } else {
                types::NewIntDatum(handle.IntValue())
            };
            continue;
        }
        if column.IsCommonHandleColumn(meta)
            && !types_dependency::metadata::NeedRestoredDataWithCollate(
                &info.FieldType,
                use_new_collation,
            )
        {
            let handle_column = meta
                .Indices
                .iter()
                .find(|index| index.Primary)
                .and_then(|index| {
                    index
                        .Columns
                        .iter()
                        .enumerate()
                        .find(|(_, index_column)| {
                            meta.Columns
                                .get(index_column.Offset as usize)
                                .is_some_and(|candidate| candidate.ID == info.ID)
                        })
                        .map(|(offset, index_column)| (offset, index_column.Length))
                });
            if let Some((offset, -1)) = handle_column {
                let (_, raw) = tablecodec_dependency::codec::DecodeOne(&handle.EncodedCol(offset))
                    .map_err(|error| types::errors::New(error.to_string()))?;
                result[position] = tablecodec_dependency::Unflatten(
                    raw,
                    Box::new(info.FieldType.clone()),
                    Some(ctx.GetEvalCtx().Location()),
                )
                .map_err(|error| types::errors::New(error.to_string()))?;
                continue;
            }
            prefix_columns.insert(info.ID);
        }
        column_types.insert(info.ID, Box::new(info.FieldType.clone()));
    }
    let row_map = tablecodec_dependency::DecodeRowToDatumMap(
        Some(value.to_vec()),
        column_types,
        Some(ctx.GetEvalCtx().Location()),
    )
    .map_err(|error| types::errors::New(error.to_string()))?;
    let mut default_values = vec![None; meta.Columns.len()];
    for (position, column) in columns.iter().enumerate() {
        let info = &column.ColumnInfo;
        if (column.IsPKHandleColumn(meta)
            || (column.IsCommonHandleColumn(meta)
                && !types_dependency::metadata::NeedRestoredDataWithCollate(
                    &info.FieldType,
                    use_new_collation,
                )))
            && !prefix_columns.contains(&info.ID)
        {
            continue;
        }
        if let Some(value) = row_map.get(&info.ID) {
            result[position] = value.clone();
            continue;
        }
        if info.IsVirtualGenerated() {
            continue;
        }
        let offset = usize::try_from(info.Offset)
            .map_err(|_| types::errors::New("negative column offset"))?;
        let cached = default_values
            .get_mut(offset)
            .ok_or_else(|| types::errors::New("column offset out of range"))?;
        result[position] = if info.ChangeStateInfo.is_some() {
            GetChangingColVal(ctx, columns, column, &row_map, &mut default_values)?.0
        } else {
            if cached.is_none() {
                *cached = Some(GetColDefaultValue(ctx, column)?);
            }
            cached.as_ref().expect("default cached").clone()
        };
    }
    Ok((result, row_map))
}
