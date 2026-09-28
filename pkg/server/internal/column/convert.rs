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

// 规划器结果字段到 MySQL 协议列元数据的转换。
//
// 将 `resolve::ResultField`（解析/规划阶段的结果列描述）转为线协议
// 用的 `Info`，包括显示宽度、小数位与 VARCHAR→VAR_STRING 兼容映射。

use crate::{Info, charset, mysql, resolve, types};

/// Converts a resolved planner result field to MySQL protocol column metadata.
/// 把规划器结果列转为协议侧 `Info`：别名、原始名、长度、精度与默认值。
pub fn ConvertColumnInfo(field: &resolve::ResultField) -> Info {
    let column = field
        .column
        .as_ref()
        .expect("ResultField.column must be present when converting protocol metadata");
    let mut info = Info {
        Name: field.column_as_name.O.clone(),
        OrgName: column.Name.O.clone(),
        Table: field.table_as_name.O.clone(),
        Schema: field.db_name.O.clone(),
        Flag: column.GetFlag() as u16,
        Charset: mysql::CharsetNameToID(column.GetCharset()) as u16,
        Type: column.GetType(),
        DefaultValue: column.GetDefaultValue(),
        ..Info::default()
    };

    // empty_org_name 表示对外隐藏原始列名（如表达式列）。
    if field.empty_org_name {
        info.OrgName.clear();
    }
    if let Some(table) = &field.table {
        info.OrgTable = table.Name.O.clone();
    }

    if column.GetFlen() != types::UnspecifiedLength as isize {
        info.ColumnLength = column.GetFlen() as u32;
        if column.GetType() == mysql::TypeNewDecimal {
            // Account for the sign and, when present, the decimal point.
            // DECIMAL 显示宽度需额外计入符号位，以及小数点（有小数位时）。
            info.ColumnLength = info.ColumnLength.wrapping_add(1);
            if column.GetDecimal() > types::DefaultFsp as isize {
                info.ColumnLength = info.ColumnLength.wrapping_add(1);
            }
        } else if types::IsString(column.GetType())
            || column.GetType() == mysql::TypeEnum
            || column.GetType() == mysql::TypeSet
        {
            // Match MySQL's byte display width so old clients do not truncate data.
            // 按字符集 Maxlen 放大为字节显示宽度，避免老客户端截断。
            let max_length = charset::GetCharsetInfo(column.GetCharset())
                .map(|charset| charset.Maxlen as u32)
                .unwrap_or(4);
            info.ColumnLength = info.ColumnLength.wrapping_mul(max_length);
        }
    } else {
        // Flen 未指定时回退到类型默认长度。
        let (length, _) = mysql::GetDefaultFieldLengthAndDecimal(column.GetType());
        info.ColumnLength = length as u32;
    }

    // UnspecifiedLength：DURATION 用默认小数秒精度，其余标为 NotFixedDec。
    info.Decimal = if column.GetDecimal() == types::UnspecifiedLength as isize {
        if column.GetType() == mysql::TypeDuration {
            types::DefaultFsp as u8
        } else {
            mysql::NotFixedDec as u8
        }
    } else {
        column.GetDecimal() as u8
    };

    // Old clients expect VARCHAR metadata to be reported as VAR_STRING.
    // 老客户端期望 VARCHAR 在元数据中报告为 VAR_STRING。
    if info.Type == mysql::TypeVarchar {
        info.Type = mysql::TypeVarString;
    }
    info
}
