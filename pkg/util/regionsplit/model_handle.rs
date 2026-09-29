// Copyright 2026 AsterSQL.

// 真实表元信息与 Datum 的 Region split handle 构造。

use crate::split_handle::{
    MinRegionStepValue, SplitError, StatementContext, encode_i64, gen_table_record_prefix,
    get_values_list,
};
use astersql_kv::Handle as _;
use astersql_meta_model as model;
use astersql_tablecodec as tablecodec;
use astersql_types as types;
use astersql_util_codec as codec;
use std::sync::atomic::Ordering;

/// 返回表句柄的列。整数主键缺失和 common handle 缺少主索引时返回空列表。
pub fn GetHandleColumnInfos(table: &model::TableInfo) -> Vec<model::ColumnInfo> {
    if table.PKIsHandle {
        return table.GetPkColInfo().cloned().into_iter().collect();
    }
    if table.IsCommonHandle {
        let Some(primary) = table.Indices.iter().find(|index| index.Primary) else {
            return Vec::new();
        };
        return primary
            .Columns
            .iter()
            .map(|column| table.Columns[column.Offset as usize].clone())
            .collect();
    }
    vec![model::NewExtraHandleColInfo()]
}

/// 转换拆分边界值，并将截断类错误格式化为 Go 的列名错误。
pub fn ConvertValueToColumnType(
    value: &types::datum::Datum,
    column: &model::ColumnInfo,
    context: types::Context,
) -> Result<types::datum::Datum, types::datum::errors::Error> {
    match value.ConvertTo(context, &column.FieldType) {
        Ok(converted) => Ok(converted),
        Err(error) => {
            // The current Datum conversion path still renders some Go
            // ErrTruncatedWrongVal instances as plain errors. Recognize that
            // exact conversion family until the scalar crate preserves the
            // typed cause, while leaving unrelated errors untouched.
            let rendered = error.to_string();
            let rendered_truncation =
                rendered.starts_with("truncated incorrect ") && rendered.contains(" value:");
            if !rendered_truncation
                && !error.Equal(&types::errors::ErrTruncated)
                && !error.Equal(&types::errors::ErrTruncatedWrongVal)
                && !error.Equal(&types::errors::ErrBadNumber)
            {
                return Err(error);
            }
            let Ok(value_string) = value.ToString() else {
                return Err(error);
            };
            let value_bytes = value_string.as_bytes();
            let name_bytes = column.Name.O.as_bytes();
            let truncated_value =
                String::from_utf8_lossy(&value_bytes[..value_bytes.len().min(128)]);
            let truncated_name = String::from_utf8_lossy(&name_bytes[..name_bytes.len().min(192)]);
            Err(types::errors::ErrTruncated
                .GenWithStack(
                    &format!("Incorrect value: '{truncated_value}' for column '{truncated_name}'"),
                    &[],
                )
                .into())
        }
    }
}

/// Go `BuildHandleColsForSplit` 的真实元信息路径。
#[derive(Clone)]
pub struct ModelHandleCols {
    table: model::TableInfo,
    primary: Option<model::IndexInfo>,
}

/// 在 common handle 表上缓存主索引，以便按索引前缀截断边界值。
pub fn BuildModelHandleColsForSplit(table: &model::TableInfo) -> ModelHandleCols {
    ModelHandleCols {
        table: table.clone(),
        primary: table.Indices.iter().find(|index| index.Primary).cloned(),
    }
}

impl ModelHandleCols {
    /// 从真实 Datum 构造编码后的 handle；调用方输入保持不变。
    pub fn BuildHandleByDatums(&self, row: &[types::datum::Datum]) -> Result<Vec<u8>, String> {
        self.BuildHandleByDatumsAt(row, codec::time::UTC)
    }

    /// 与 Go StatementContext 时区一致的句柄编码入口。
    pub fn BuildHandleByDatumsAt(
        &self,
        row: &[types::datum::Datum],
        location: codec::time::Location,
    ) -> Result<Vec<u8>, String> {
        if !self.table.IsCommonHandle {
            let Some(first) = row.first() else {
                return Err("integer handle requires one datum".to_owned());
            };
            return Ok(((first.GetInt64() as u64) ^ (1_u64 << 63))
                .to_be_bytes()
                .to_vec());
        }
        let mut values = row.to_vec();
        if let Some(primary) = &self.primary {
            if values.len() == primary.Columns.len() {
                tablecodec::TruncateIndexValues(
                    Box::new(self.table.clone()),
                    Box::new(primary.clone()),
                    &mut values,
                );
            }
        }
        let encoded =
            codec::EncodeKey(location, Vec::new(), values).map_err(|error| error.to_string())?;
        astersql_kv::NewCommonHandle(encoded)
            .map(|handle| handle.Encoded())
            .map_err(|error| error.to_string())
    }

    /// 是否采用整数 handle。
    pub fn IsInt(&self) -> bool {
        !self.table.IsCommonHandle
    }
}

/// 使用完整模型与 Datum 生成表记录切分键，包含前缀主键截断。
pub fn GetSplitTableKeysForModel(
    statement_context: &StatementContext,
    table: &model::TableInfo,
    physical_id: i64,
    lower: &[types::datum::Datum],
    upper: &[types::datum::Datum],
    number: usize,
    mut keys: Vec<Vec<u8>>,
) -> Result<Vec<Vec<u8>>, SplitError> {
    if number == 0 || lower.is_empty() || upper.is_empty() {
        return Err(SplitError::InvalidRanges(
            "split region bounds and count must be non-empty".to_owned(),
        ));
    }
    let record_prefix = gen_table_record_prefix(physical_id);
    if !table.Indices.is_empty() && !(table.IsCommonHandle && table.Indices.len() == 1) {
        keys.push(record_prefix.clone());
    }
    let handle_columns = BuildModelHandleColsForSplit(table);
    if handle_columns.IsInt() {
        let unsigned = table.PKIsHandle
            && table.GetPkColInfo().is_some_and(|column| {
                astersql_parser_mysql::r#type::HasUnsignedFlag(column.GetFlag())
            });
        let (low, step) = if unsigned {
            let low = lower[0].GetUint64();
            let high = upper[0].GetUint64();
            if high <= low {
                return Err(SplitError::InvalidRanges(format!(
                    "lower value {low} should less than the upper value {high}"
                )));
            }
            (low as i64, ((high - low) / number as u64) as i64)
        } else {
            let low = lower[0].GetInt64();
            let high = upper[0].GetInt64();
            if high <= low {
                return Err(SplitError::InvalidRanges(format!(
                    "lower value {low} should less than the upper value {high}"
                )));
            }
            (low, (high.wrapping_sub(low) as u64 / number as u64) as i64)
        };
        let minimum = MinRegionStepValue.load(Ordering::Acquire);
        if step < minimum {
            return Err(SplitError::InvalidRanges(format!(
                "the region size is too small, expected at least {minimum}, but got {step}"
            )));
        }
        let mut current = low;
        for _ in 1..number {
            current = current.wrapping_add(step);
            let mut key = record_prefix.clone();
            key.extend_from_slice(&encode_i64(current));
            keys.push(key);
        }
        return Ok(keys);
    }
    let location = if statement_context.time_zone.is_empty() {
        codec::time::UTC
    } else {
        statement_context
            .time_zone
            .parse()
            .map_err(|_| SplitError::InvalidDatum("invalid statement time zone".to_owned()))?
    };
    let low = handle_columns
        .BuildHandleByDatumsAt(lower, location)
        .map_err(SplitError::InvalidDatum)?;
    let high = handle_columns
        .BuildHandleByDatumsAt(upper, location)
        .map_err(SplitError::InvalidDatum)?;
    if low >= high {
        let format_row = |row: &[types::datum::Datum]| {
            format!(
                "({})",
                row.iter()
                    .map(|datum| datum.ToString().unwrap_or_default())
                    .collect::<Vec<_>>()
                    .join(",")
            )
        };
        return Err(SplitError::InvalidRanges(format!(
            "Split table `{}` region lower value {} should less than the upper value {}",
            table.Name.O,
            format_row(lower),
            format_row(upper)
        )));
    }
    let mut low_key = record_prefix.clone();
    low_key.extend_from_slice(&low);
    let mut high_key = record_prefix;
    high_key.extend_from_slice(&high);
    get_values_list(&low_key, &high_key, number, keys)
}
