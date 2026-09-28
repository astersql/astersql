// Copyright 2026 AsterSQL.
// Copyright 2015 PingCAP, Inc.
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

//! CREATE TABLE ordinary-index length validation.

use super::*;

fn default_mysql_type_length(column_type: u8) -> Option<usize> {
    astersql_parser_mysql::r#const::DefaultLengthOfMysqlTypes
        .iter()
        .find_map(|(candidate, length)| (*candidate == column_type).then_some(*length))
}

fn decimal_index_length(precision: usize) -> usize {
    (precision / 9) * 4 + ((precision % 9) + 1) / 2
}

fn index_column_length(
    column: &astersql_meta_model::ColumnInfo,
    index_length: isize,
    columnar: astersql_meta_model::ColumnarIndexType,
) -> SessionResult<usize> {
    use astersql_parser_mysql::r#type as mysql;

    if columnar != astersql_meta_model::ColumnarIndexType::NA {
        // Go uses one byte here so columnar indexes remain non-zero in callers
        // while avoiding the row-store key-length limit.
        return Ok(1);
    }

    let length = if index_length != astersql_parser_types::UnspecifiedLength {
        index_length
    } else {
        column.GetFlen()
    };
    let length = usize::try_from(length).map_err(|_| {
        SessionError::new(format!(
            "invalid index length for column '{}'",
            column.Name.O
        ))
    })?;

    match column.GetType() {
        mysql::TypeBit => Ok(length.div_ceil(8)),
        mysql::TypeVarchar
        | mysql::TypeString
        | mysql::TypeVarString
        | mysql::TypeTinyBlob
        | mysql::TypeMediumBlob
        | mysql::TypeBlob
        | mysql::TypeLongBlob => {
            let charset = astersql_parser_charset::charset::GetCharsetInfo(column.GetCharset())
                .map_err(|_| {
                    SessionError::new(format!(
                        "Unsupported charset {}, collate {}",
                        column.GetCharset(),
                        column.GetCollate()
                    ))
                })?;
            length
                .checked_mul(usize::try_from(charset.Maxlen).unwrap_or_default())
                .ok_or_else(|| SessionError::new("index column length overflow"))
        }
        mysql::TypeTiny
        | mysql::TypeInt24
        | mysql::TypeLong
        | mysql::TypeLonglong
        | mysql::TypeDouble
        | mysql::TypeShort
        | mysql::TypeYear
        | mysql::TypeDate
        | mysql::TypeDuration
        | mysql::TypeDatetime
        | mysql::TypeTimestamp => default_mysql_type_length(column.GetType())
            .ok_or_else(|| SessionError::new("missing default MySQL type length")),
        mysql::TypeFloat => default_mysql_type_length(
            if length <= astersql_parser_mysql::r#const::MaxFloatPrecisionLength {
                mysql::TypeFloat
            } else {
                mysql::TypeDouble
            },
        )
        .ok_or_else(|| SessionError::new("missing default MySQL float length")),
        mysql::TypeNewDecimal => Ok(decimal_index_length(length)),
        _ => Ok(length),
    }
}

impl ConcreteSession {
    /// Go `buildIndexColumns`: enforce the configured cumulative row-index
    /// length and preserve its non-strict single-index truncation behavior.
    pub(super) fn validate_create_table_index_lengths(
        &self,
        table: &mut astersql_meta_model::TableInfo,
    ) -> SessionResult<()> {
        let maximum = usize::try_from(astersql_config::get_global_config().max_index_length)
            .map_err(|_| SessionError::new("max-index-length must not be negative"))?;
        let strict = astersql_parser_mysql::r#const::GetSQLMode(&self.state.borrow().sql_mode)
            .map_err(|error| SessionError::new(error.to_string()))?
            .HasStrictMode();

        for index in &mut table.Indices {
            let columnar = index.GetColumnarIndexType();
            let key_part_count = index.Columns.len();
            let mut total = 0usize;
            for index_column in &mut index.Columns {
                let column = table
                    .Columns
                    .iter()
                    .find(|column| column.Name.L == index_column.Name.L)
                    .ok_or_else(|| {
                        SessionError::new(format!("column does not exist: {}", index_column.Name.O))
                    })?;
                let column_length = index_column_length(column, index_column.Length, columnar)?;
                total = total
                    .checked_add(column_length)
                    .ok_or_else(|| SessionError::new("index length overflow"))?;
                if total <= maximum {
                    continue;
                }

                let message = format!(
                    "[ddl:1071]Specified key was too long ({total} bytes); max key length is {maximum} bytes"
                );
                if strict
                    || index.Unique
                    || index.Primary
                    || astersql_parser_mysql::r#type::HasUniKeyFlag(column.GetFlag())
                    || key_part_count > 1
                {
                    return Err(SessionError::new(message));
                }

                let bytes_per_unit = index_column_length(column, 1, columnar)?;
                if bytes_per_unit == 0 {
                    return Err(SessionError::new(message));
                }
                index_column.Length = isize::try_from(maximum / bytes_per_unit)
                    .map_err(|_| SessionError::new("index prefix length is out of range"))?;
                self.set_warning_with_code(1071, message);
            }
        }
        Ok(())
    }
}
