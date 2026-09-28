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

// MySQL 列定义与结果行的线协议编码。
//
// 将内部列元数据（`Info`）序列化为 ColumnDefinition41 包，并把 chunk 行
// （列式执行结果中的一行）按文本协议或二进制协议写出，供客户端消费。

use crate::{charset, chunk, err, mysql, textrow};

pub use crate::model::DefaultValue;

/// 列名在协议包中的最大字节长度（超出部分截断）。
const maxColumnNameSize: usize = 256;

/// Information sent in a MySQL column-definition packet.
/// 发送给客户端的 MySQL 列定义（ColumnDefinition41）元数据。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Info {
    /// 列默认值；COM_FIELD_LIST 响应可能附带写出。
    pub DefaultValue: Option<DefaultValue>,
    /// 库名（schema）。
    pub Schema: String,
    /// 结果集中的表别名。
    pub Table: String,
    /// 原始表名。
    pub OrgTable: String,
    /// 结果集中的列别名。
    pub Name: String,
    /// 原始列名。
    pub OrgName: String,
    /// 显示宽度 / 字段长度。
    pub ColumnLength: u32,
    /// 字符集 / 校对集 ID。
    pub Charset: u16,
    /// 列标志位（如 UNSIGNED、NOT NULL）。
    pub Flag: u16,
    /// 小数位数（DECIMAL / 时间类型的精度）。
    pub Decimal: u8,
    /// MySQL 字段类型字节。
    pub Type: u8,
}

impl Info {
    /// Encodes a ColumnDefinition41 packet without a default value.
    /// 编码 ColumnDefinition41 包，不含默认值后缀。
    pub fn Dump(&self, buffer: Vec<u8>, encoder: Option<&mut textrow::ResultEncoder>) -> Vec<u8> {
        self.dump(buffer, encoder, false)
    }

    /// Encodes a ColumnDefinition41 packet including its ComFieldList default.
    /// 编码 ColumnDefinition41，并附带 COM_FIELD_LIST 所需的默认值字段。
    pub fn DumpWithDefault(
        &self,
        buffer: Vec<u8>,
        encoder: Option<&mut textrow::ResultEncoder>,
    ) -> Vec<u8> {
        self.dump(buffer, encoder, true)
    }

    /// 内部实现：按 ColumnDefinition41 布局写出 catalog/schema/table/name，
    /// 再写定长类型元数据；`with_default` 为真时追加默认值。
    fn dump(
        &self,
        mut buffer: Vec<u8>,
        encoder: Option<&mut textrow::ResultEncoder>,
        with_default: bool,
    ) -> Vec<u8> {
        // 未传入编码器时回退到 utf8mb4，保证元数据可独立编码。
        let mut fallback = None;
        let encoder = match encoder {
            Some(encoder) => encoder,
            None => fallback.insert(textrow::NewResultEncoder(charset::CharsetUTF8MB4)),
        };

        // 协议限制列名长度，避免超长别名撑破包。
        let name = &self.Name.as_bytes()[..self.Name.len().min(maxColumnNameSize)];
        let org_name = &self.OrgName.as_bytes()[..self.OrgName.len().min(maxColumnNameSize)];
        // catalog 固定为 "def"（MySQL 协议约定）。
        buffer = dump::LengthEncodedString(buffer, b"def");
        for value in [
            self.Schema.as_bytes(),
            self.Table.as_bytes(),
            self.OrgTable.as_bytes(),
            name,
            org_name,
        ] {
            let encoded = encoder.EncodeMeta(value);
            buffer = dump::LengthEncodedString(buffer, &encoded);
        }

        // 0x0c 表示后续定长字段长度为 12 字节。
        buffer.push(0x0c);
        buffer = dump::Uint16(
            buffer,
            encoder.ColumnCharsetID(self.dumpCharset(), textrow::IsStringColumnType(self.Type)),
        );
        buffer = dump::Uint32(buffer, self.dumpLength());
        buffer.push(dumpType(self.Type));
        buffer = dump::Uint16(buffer, DumpFlag(self.Type, self.Flag));
        buffer.extend_from_slice(&[self.Decimal, 0, 0]);

        // COM_FIELD_LIST：NULL / 时间函数默认值写成 0xfb，其余按长度编码字符串。
        if with_default {
            match &self.DefaultValue {
                None => buffer.push(0xfb),
                Some(DefaultValue::String(value))
                    if value == b"CURRENT_TIMESTAMP" || value == b"CURRENT_DATE" =>
                {
                    buffer.push(0xfb)
                }
                Some(value) => {
                    let rendered = render_default_value(value);
                    buffer = dump::LengthEncodedString(buffer, &rendered);
                }
            }
        }
        buffer
    }

    /// 写出协议用字符集：向量类型强制默认校对集，其余沿用列 Charset。
    pub fn dumpCharset(&self) -> u16 {
        if self.Type == mysql::TypeTiDBVectorFloat32 {
            mysql::DefaultCollationID
        } else {
            self.Charset
        }
    }

    /// 写出协议用列长度：向量类型映射为 MaxLongBlobWidth。
    pub fn dumpLength(&self) -> u32 {
        if self.Type == mysql::TypeTiDBVectorFloat32 {
            mysql::MaxLongBlobWidth as u32
        } else {
            self.ColumnLength
        }
    }

    /// 转为文本行格式化所需的精简列信息。
    fn toTextRow(&self) -> textrow::ColumnInfo {
        textrow::ColumnInfo {
            Type: self.Type,
            Charset: self.Charset,
            Flag: self.Flag,
            Decimal: self.Decimal,
            Table: self.Table.clone(),
        }
    }
}

/// 将内部 DefaultValue 渲染为 Go `fmt.Sprintf("%v", value)` 的协议字节。
fn render_default_value(value: &DefaultValue) -> Vec<u8> {
    match value {
        DefaultValue::Bool(value) => value.to_string().into_bytes(),
        DefaultValue::Int(value) => value.to_string().into_bytes(),
        DefaultValue::Uint(value) => value.to_string().into_bytes(),
        DefaultValue::Float(value) => format_go_float(*value).into_bytes(),
        // Go strings may contain arbitrary bytes; never replace invalid UTF-8.
        DefaultValue::String(value) => value.clone(),
    }
}

/// Go `%v` uses the shortest `%g` representation and a two-digit exponent.
fn format_go_float(value: f64) -> String {
    if value.is_nan() {
        return "NaN".to_owned();
    }
    if value == f64::INFINITY {
        return "+Inf".to_owned();
    }
    if value == f64::NEG_INFINITY {
        return "-Inf".to_owned();
    }

    let scientific = format!("{value:e}");
    let (mantissa, exponent) = scientific
        .split_once('e')
        .expect("Rust scientific float format has an exponent");
    let exponent: i32 = exponent
        .parse()
        .expect("Rust scientific float exponent is numeric");
    if (-4..6).contains(&exponent) {
        return value.to_string();
    }
    format!("{mantissa}e{exponent:+03}")
}

/// Applies the protocol-only flags used for set, enum, and vector columns.
/// 按类型补齐/清除协议侧标志：SET/ENUM 置位，向量类型去掉 BinaryFlag。
pub fn DumpFlag(tp: u8, flag: u16) -> u16 {
    match tp {
        mysql::TypeSet => flag | mysql::SetFlag as u16,
        mysql::TypeEnum => flag | mysql::EnumFlag as u16,
        mysql::TypeTiDBVectorFloat32 => flag & !(mysql::BinaryFlag as u16),
        _ => flag,
    }
}

/// 将内部类型映射为客户端可识别的 MySQL 协议类型字节。
pub fn dumpType(tp: u8) -> u8 {
    match tp {
        mysql::TypeSet | mysql::TypeEnum => mysql::TypeString,
        mysql::TypeTiDBVectorFloat32 => mysql::TypeLongBlob,
        mysql::TypeTinyBlob | mysql::TypeMediumBlob | mysql::TypeLongBlob => mysql::TypeBlob,
        _ => tp,
    }
}

/// Encodes every value with the shared text-row formatter and length framing.
/// 按文本协议编码一行：NULL 写 0xfb，非空值经 textrow 格式化后长度编码写出。
pub fn DumpTextRow(
    mut buffer: Vec<u8>,
    columns: &[Info],
    row: chunk::Row,
    encoder: Option<&mut textrow::ResultEncoder>,
) -> Result<Vec<u8>, err::Error> {
    let mut fallback = None;
    let encoder = match encoder {
        Some(encoder) => encoder,
        None => fallback.insert(textrow::NewResultEncoder(charset::CharsetUTF8MB4)),
    };

    for (index, column) in columns.iter().enumerate() {
        // MySQL 文本协议用 0xfb 表示 NULL。
        if row.IsNull(index) {
            buffer.push(0xfb);
            continue;
        }
        let value =
            textrow::FormatValueText(&row, index, &column.toTextRow(), encoder).map_err(|_| {
                err::ErrInvalidType.GenWithStack(&format!("invalid type {}", column.Type), &[])
            })?;
        buffer = dump::LengthEncodedString(buffer, &value);
    }
    Ok(buffer)
}

/// Encodes a row using the MySQL binary-row header, bitmap, and native values.
/// 按二进制结果集协议编码一行：OK 头 + NULL 位图 + 各类型原生化载荷。
pub fn DumpBinaryRow(
    mut buffer: Vec<u8>,
    columns: &[Info],
    row: chunk::Row,
    encoder: Option<&mut textrow::ResultEncoder>,
) -> Result<Vec<u8>, err::Error> {
    let mut fallback = None;
    let encoder = match encoder {
        Some(encoder) => encoder,
        None => fallback.insert(textrow::NewResultEncoder(charset::CharsetUTF8MB4)),
    };

    // 二进制行以 OKHeader 开头，随后是 (列数+7+2)/8 字节的 NULL 位图。
    buffer.push(mysql::OKHeader);
    let null_bitmap_offset = buffer.len();
    buffer.resize(null_bitmap_offset + (columns.len() + 9) / 8, 0);

    for (index, column) in columns.iter().enumerate() {
        // 位图从第 2 bit 起对应第 0 列（前两 bit 保留）。
        if row.IsNull(index) {
            buffer[null_bitmap_offset + (index + 2) / 8] |= 1 << ((index + 2) % 8);
            continue;
        }

        // 按字段类型选择定长整数、浮点位型或长度编码字符串写出。
        match column.Type {
            mysql::TypeTiny => buffer.push(row.GetInt64(index) as u8),
            mysql::TypeShort | mysql::TypeYear => {
                buffer = dump::Uint16(buffer, row.GetInt64(index) as u16)
            }
            mysql::TypeInt24 | mysql::TypeLong => {
                buffer = dump::Uint32(buffer, row.GetInt64(index) as u32)
            }
            mysql::TypeLonglong => buffer = dump::Uint64(buffer, row.GetUint64(index)),
            mysql::TypeFloat => buffer = dump::Uint32(buffer, row.GetFloat32(index).to_bits()),
            mysql::TypeDouble => buffer = dump::Uint64(buffer, row.GetFloat64(index).to_bits()),
            mysql::TypeNewDecimal => {
                buffer =
                    dump::LengthEncodedString(buffer, row.GetMyDecimal(index).String().as_bytes())
            }
            mysql::TypeString
            | mysql::TypeVarString
            | mysql::TypeVarchar
            | mysql::TypeBit
            | mysql::TypeTinyBlob
            | mysql::TypeMediumBlob
            | mysql::TypeLongBlob
            | mysql::TypeBlob => {
                encoder.UpdateDataEncoding(column.Charset);
                let encoded = encoder.EncodeData(&row.GetBytes(index));
                buffer = dump::LengthEncodedString(buffer, &encoded);
            }
            mysql::TypeDate | mysql::TypeDatetime | mysql::TypeTimestamp => {
                buffer = dump::BinaryDateTime(buffer, row.GetTime(index))
            }
            mysql::TypeDuration => buffer.extend_from_slice(&dump::BinaryTime(
                time::Duration::nanoseconds(row.GetDuration(index, 0).Duration),
            )),
            mysql::TypeEnum => {
                encoder.UpdateDataEncoding(column.Charset);
                let value = row.GetEnum(index).String();
                let encoded = encoder.EncodeData(value.as_bytes());
                buffer = dump::LengthEncodedString(buffer, &encoded);
            }
            mysql::TypeSet => {
                encoder.UpdateDataEncoding(column.Charset);
                let value = row.GetSet(index).String();
                let encoded = encoder.EncodeData(value.as_bytes());
                buffer = dump::LengthEncodedString(buffer, &encoded);
            }
            mysql::TypeJSON => {
                encoder.UpdateDataEncoding(mysql::DefaultCollationID);
                let value = row.GetJSON(index).String();
                let encoded = encoder.EncodeData(value.as_bytes());
                buffer = dump::LengthEncodedString(buffer, &encoded);
            }
            mysql::TypeTiDBVectorFloat32 => {
                encoder.UpdateDataEncoding(mysql::DefaultCollationID);
                let value = row.GetVectorFloat32(index).String();
                let encoded = encoder.EncodeData(value.as_bytes());
                buffer = dump::LengthEncodedString(buffer, &encoded);
            }
            _ => {
                return Err(
                    err::ErrInvalidType.GenWithStack(&format!("invalid type {}", column.Type), &[])
                );
            }
        }
    }
    Ok(buffer)
}
