// Copyright 2026 AsterSQL.
// Copyright 2019 PingCAP, Inc.
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     http://www.apache.org/licenses/LICENSE-2.0
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// 解析器测试驱动的 Datum（数据盒）与字面量解析。
//
// 对照 Go `test_driver_datum.go`：用判别值 kind + 紧凑字段模拟 Go Datum，
// 支持整数/浮点/字符串/十进制/二进制字面量存取，以及 BIT/HEX 解析与默认 FieldType 推断。
// Datum 是表达式求值中承载单个标量值的统一容器。

// 在内存中描述解析器测试驱动的数据盒。
// Go 的 any/type switch、errors、charset、mysql 与 types 依赖保留为跨文件接线点。
use std::any::Any;

use crate::*;

// Kind 常量与 Go byte 判别值严格对齐，供 ValueExpr 的 switch/match 共用。
/// NULL 判别值。
pub const KindNull: u8 = 0;
/// 有符号 64 位整数。
pub const KindInt64: u8 = 1;
/// 无符号 64 位整数。
pub const KindUint64: u8 = 2;
/// 32 位浮点。
pub const KindFloat32: u8 = 3;
/// 64 位浮点。
pub const KindFloat64: u8 = 4;
/// 字符串（字节按 UTF-8 解释）。
pub const KindString: u8 = 5;
/// 原始字节序列。
pub const KindBytes: u8 = 6;
/// 二进制字面量（BIT / HEX）。
pub const KindBinaryLiteral: u8 = 7; // 用于 BIT / HEX 字面量。
/// MySQL DECIMAL（定点数）。
pub const KindMysqlDecimal: u8 = 8;
/// MySQL TIME/DURATION。
pub const KindMysqlDuration: u8 = 9;
/// MySQL ENUM。
pub const KindMysqlEnum: u8 = 10;
/// MySQL BIT 列值。
pub const KindMysqlBit: u8 = 11; // 用于 BIT 表列值。
/// MySQL SET。
pub const KindMysqlSet: u8 = 12;
/// MySQL 日期时间。
pub const KindMysqlTime: u8 = 13;
/// 动态接口载荷（对应 Go any）。
pub const KindInterface: u8 = 14;
/// 排序用「最小非空」哨兵。
pub const KindMinNotNull: u8 = 15;
/// 排序用「最大值」哨兵。
pub const KindMaxValue: u8 = 16;
/// 原始未解释载荷。
pub const KindRaw: u8 = 17;
/// MySQL JSON。
pub const KindMysqlJSON: u8 = 18;

// Datum 对应 Go 的紧凑数据盒：i 同时保存整数和浮点位模式，b 保存字符串/字节，x 保存其它动态值。
/// 紧凑数据盒：kind + i/b/x 字段复用存储。
#[derive(Default)]
pub struct Datum {
    k: u8,
    i: i64,
    b: Vec<u8>,
    x: Option<Box<dyn Any>>,
}

/// Borrowing representation of Go's `any` return value. Primitive values are
/// copied exactly; dynamically stored values stay borrowed instead of being
/// replaced by a placeholder merely to satisfy Rust ownership rules.
/// Go `any` 返回值的借用表示：原语按值拷贝，动态载荷保持借用。
pub enum DatumValue<'a> {
    Null,
    Int64(i64),
    Uint64(u64),
    Float32(f32),
    Float64(f64),
    String(String),
    Bytes(&'a [u8]),
    Decimal(&'a MyDecimal),
    Binary(BinaryLiteral),
    Interface(&'a dyn Any),
}

impl Datum {
    // Kind 返回当前判别值。
    /// 返回当前 kind 判别值。
    pub fn Kind(&self) -> u8 {
        self.k
    }

    // 整数访问器保持 Go 的直接读写语义。
    /// 读取有符号整数。
    pub fn GetInt64(&self) -> i64 {
        self.i
    }
    /// 写入有符号整数并设置 kind。
    pub fn SetInt64(&mut self, value: i64) {
        self.k = KindInt64;
        self.i = value;
    }

    // uint64 与 Go 一样复用 i64 存储位模式，而不是另设字段。
    /// 读取无符号整数（复用 i 的位模式）。
    pub fn GetUint64(&self) -> u64 {
        self.i as u64
    }
    /// 写入无符号整数（位模式存入 i）。
    pub fn SetUint64(&mut self, value: u64) {
        self.k = KindUint64;
        self.i = value as i64;
    }

    // 浮点数通过 IEEE 位模式装入 i64，避免数值转换丢失。
    /// 按 IEEE 位模式读取 f64。
    pub fn GetFloat64(&self) -> f64 {
        f64::from_bits(self.i as u64)
    }
    /// 按 IEEE 位模式写入 f64。
    pub fn SetFloat64(&mut self, value: f64) {
        self.k = KindFloat64;
        self.i = value.to_bits() as i64;
    }
    /// 读取 f32（经 f64 位模式转换）。
    pub fn GetFloat32(&self) -> f32 {
        f64::from_bits(self.i as u64) as f32
    }
    /// 写入 f32（提升为 f64 位模式存储）。
    pub fn SetFloat32(&mut self, value: f32) {
        self.k = KindFloat32;
        self.i = (value as f64).to_bits() as i64;
    }

    // 字符串沿用 Go []byte 存储；这里按 UTF-8 损失替换生成只读文本。
    /// 将内部字节按 UTF-8（损失替换）转为 String。
    pub fn GetString(&self) -> String {
        String::from_utf8_lossy(&self.b).into_owned()
    }
    /// 写入字符串并设置 KindString。
    pub fn SetString(&mut self, value: &str) {
        self.k = KindString;
        self.b = value.as_bytes().to_vec();
    }

    /// 借用内部字节切片。
    pub fn GetBytes(&self) -> &[u8] {
        &self.b
    }
    /// 写入字节并设置 KindBytes。
    pub fn SetBytes(&mut self, value: Vec<u8>) {
        self.k = KindBytes;
        self.b = value;
    }
    /// 以字符串 kind 写入原始字节（不强制 UTF-8 合法）。
    pub fn SetBytesAsString(&mut self, value: Vec<u8>) {
        self.k = KindString;
        self.b = value;
    }

    // interface 值使用 Any 保留 Go 动态载荷形状。
    /// 借用动态 Interface 载荷。
    pub fn GetInterface(&self) -> Option<&dyn Any> {
        self.x.as_deref()
    }
    /// 写入动态 Interface 载荷。
    pub fn SetInterface(&mut self, value: Box<dyn Any>) {
        self.k = KindInterface;
        self.x = Some(value);
    }
    /// 置为 NULL 并清空动态载荷。
    pub fn SetNull(&mut self) {
        self.k = KindNull;
        self.x = None;
    }

    /// 将内部字节包装为 BinaryLiteral。
    pub fn GetBinaryLiteral(&self) -> BinaryLiteral {
        BinaryLiteral(self.b.clone())
    }
    /// 写入二进制字面量。
    pub fn SetBinaryLiteral(&mut self, value: BinaryLiteral) {
        self.k = KindBinaryLiteral;
        self.b = value.0;
    }

    // decimal 必须由 SetMysqlDecimal 写入；类型不匹配与 Go 类型断言一样属于调用错误。
    /// 借用 MyDecimal；类型不匹配时 panic。
    pub fn GetMysqlDecimal(&self) -> &MyDecimal {
        self.x
            .as_ref()
            .and_then(|v| v.downcast_ref::<MyDecimal>())
            .expect("datum is not MyDecimal")
    }
    /// 写入 MyDecimal。
    pub fn SetMysqlDecimal(&mut self, value: MyDecimal) {
        self.k = KindMysqlDecimal;
        self.x = Some(Box::new(value));
    }

    // GetValue 按 kind 还原公开值；动态返回类型对应 Go any。
    /// 按 kind 还原为 DatumValue（对应 Go any）。
    pub fn GetValue(&self) -> DatumValue<'_> {
        match self.k {
            KindNull => DatumValue::Null,
            KindInt64 => DatumValue::Int64(self.GetInt64()),
            KindUint64 => DatumValue::Uint64(self.GetUint64()),
            KindFloat32 => DatumValue::Float32(self.GetFloat32()),
            KindFloat64 => DatumValue::Float64(self.GetFloat64()),
            KindString => DatumValue::String(self.GetString()),
            KindBytes => DatumValue::Bytes(&self.b),
            KindMysqlDecimal => DatumValue::Decimal(self.GetMysqlDecimal()),
            KindBinaryLiteral | KindMysqlBit => DatumValue::Binary(self.GetBinaryLiteral()),
            _ => DatumValue::Interface(self.GetInterface().expect("interface datum has no value")),
        }
    }

    // SetValue 对应 Go type switch，按具体运行时类型选择专用 setter。
    /// 按运行时类型分发到专用 setter（对齐 Go type switch）。
    pub fn SetValue(&mut self, value: Box<dyn Any>) {
        if value.is::<()>() {
            self.SetNull();
        } else if value.is::<bool>() {
            self.SetInt64(if *value.downcast::<bool>().unwrap() {
                1
            } else {
                0
            });
        } else if value.is::<i32>() {
            self.SetInt64(*value.downcast::<i32>().unwrap() as i64);
        } else if value.is::<i64>() {
            self.SetInt64(*value.downcast::<i64>().unwrap());
        } else if value.is::<u64>() {
            self.SetUint64(*value.downcast::<u64>().unwrap());
        } else if value.is::<f32>() {
            self.SetFloat32(*value.downcast::<f32>().unwrap());
        } else if value.is::<f64>() {
            self.SetFloat64(*value.downcast::<f64>().unwrap());
        } else if value.is::<String>() {
            self.SetString(&value.downcast::<String>().unwrap());
        } else if value.is::<Vec<u8>>() {
            self.SetBytes(*value.downcast::<Vec<u8>>().unwrap());
        } else if value.is::<MyDecimal>() {
            self.SetMysqlDecimal(*value.downcast::<MyDecimal>().unwrap());
        } else if value.is::<BinaryLiteral>() {
            self.SetBinaryLiteral(*value.downcast::<BinaryLiteral>().unwrap());
        } else if value.is::<BitLiteral>() {
            self.SetBinaryLiteral(BinaryLiteral(value.downcast::<BitLiteral>().unwrap().0));
        } else if value.is::<HexLiteral>() {
            self.SetBinaryLiteral(BinaryLiteral(value.downcast::<HexLiteral>().unwrap().0));
        } else {
            self.SetInterface(value);
        }
    }
}

// NewDatum 从动态输入创建 Datum；Go 的 []any 特例会逐项递归转换。
/// 从动态输入创建 Datum；Vec 特例会逐项递归包装。
pub fn NewDatum(input: Box<dyn Any>) -> Datum {
    let mut datum = Datum::default();
    if input.is::<Vec<Box<dyn Any>>>() {
        datum.SetValue(Box::new(MakeDatums(
            *input.downcast::<Vec<Box<dyn Any>>>().unwrap(),
        )));
    } else {
        datum.SetValue(input);
    }
    datum
}

/// 构造 KindBytes 的 Datum。
pub fn NewBytesDatum(bytes: Vec<u8>) -> Datum {
    let mut d = Datum::default();
    d.SetBytes(bytes);
    d
}
/// 构造 KindString 的 Datum。
pub fn NewStringDatum(text: &str) -> Datum {
    let mut d = Datum::default();
    d.SetString(text);
    d
}

// MakeDatums 保持输入顺序逐项包装，不进行共享或 IO。
/// 按输入顺序将动态值列表包装为 Datum 向量。
pub fn MakeDatums(args: Vec<Box<dyn Any>>) -> Vec<Datum> {
    args.into_iter().map(NewDatum).collect()
}

// 三种字面量使用不同新类型，避免解析完成后丢失源码类别。
/// 通用二进制字面量字节包装。
#[derive(Clone, Default)]
pub struct BinaryLiteral(pub Vec<u8>);
/// BIT 字面量（源码类别标记）。
pub struct BitLiteral(pub Vec<u8>);
/// HEX 字面量（源码类别标记）。
pub struct HexLiteral(pub Vec<u8>);
/// 空二进制字面量的共享空切片。
pub static ZeroBinaryLiteral: &[u8] = &[];

impl BinaryLiteral {
    // String 对应 fmt.Stringer：非空值使用 0x 前缀和小写十六进制。
    /// 格式化为 `0x` 前缀小写十六进制；空值返回空串。
    pub fn String(&self) -> String {
        if self.0.is_empty() {
            String::new()
        } else {
            format!("0x{}", hex::encode(&self.0))
        }
    }
    /// 将字节按 UTF-8 损失替换转为字符串。
    pub fn ToString(&self) -> String {
        String::from_utf8_lossy(&self.0).into_owned()
    }

    // ToBitLiteralString 逐字节写八位二进制，可按 Go 规则裁掉前导零。
    /// 格式化为 `b'...'` 位串，可选裁剪前导零。
    pub fn ToBitLiteralString(&self, trim_leading_zero: bool) -> String {
        if self.0.is_empty() {
            return "b''".to_owned();
        }
        let mut bits = self
            .0
            .iter()
            .map(|v| format!("{v:08b}"))
            .collect::<String>();
        if trim_leading_zero {
            bits = bits.trim_start_matches('0').to_owned();
            if bits.is_empty() {
                bits.push('0');
            }
        }
        format!("b'{bits}'")
    }
}

// ParseBitStr 接受 b'val'、B'val' 或 0bval，并把位串左侧补齐到整字节。
/// 解析 BIT 字面量字符串为 BinaryLiteral。
pub fn ParseBitStr(input: &str) -> Result<BinaryLiteral, String> {
    if input.is_empty() {
        return Err("invalid empty string for parsing bit type".to_owned());
    }
    let bits = if input.starts_with('b') || input.starts_with('B') {
        input[1..].trim_matches('\'')
    } else if let Some(rest) = input.strip_prefix("0b") {
        rest
    } else {
        return Err(format!("invalid bit type format {input}"));
    };
    if bits.is_empty() {
        return Ok(BinaryLiteral::default());
    }

    let aligned_length = (bits.len() + 7) & !7;
    let padded = format!("{:0>width$}", bits, width = aligned_length);
    let mut output = Vec::with_capacity(aligned_length / 8);
    for chunk in padded.as_bytes().chunks_exact(8) {
        // 非 0/1 字符由基数解析返回错误，与 errors.Trace 路径一致。
        let text = std::str::from_utf8(chunk).map_err(|err| err.to_string())?;
        output.push(u8::from_str_radix(text, 2).map_err(|err| err.to_string())?);
    }
    Ok(BinaryLiteral(output))
}

/// 解析并包装为 BitLiteral。
pub fn NewBitLiteral(input: &str) -> Result<BitLiteral, String> {
    ParseBitStr(input).map(|value| BitLiteral(value.0))
}
impl BitLiteral {
    /// 委托 BinaryLiteral::ToString。
    pub fn ToString(&self) -> String {
        BinaryLiteral(self.0.clone()).ToString()
    }
}

// ParseHexStr 区分 x'val'（必须偶数位）和 0xval（允许补一个前导零）。
/// 解析 HEX 字面量字符串为 BinaryLiteral。
pub fn ParseHexStr(input: &str) -> Result<BinaryLiteral, String> {
    if input.is_empty() {
        return Err("invalid empty string for parsing hexadecimal literal".to_owned());
    }
    let (mut digits, quoted) = if input.starts_with('x') || input.starts_with('X') {
        (input[1..].trim_matches('\'').to_owned(), true)
    } else if let Some(rest) = input.strip_prefix("0x") {
        (rest.to_owned(), false)
    } else {
        return Err(format!("invalid hexadecimal format {input}"));
    };
    if quoted && digits.len() % 2 != 0 {
        return Err(format!(
            "invalid hexadecimal format, must even numbers, but {}",
            digits.len()
        ));
    }
    if digits.is_empty() {
        return Ok(BinaryLiteral::default());
    }
    if digits.len() % 2 != 0 {
        digits.insert(0, '0');
    }
    hex::decode(digits)
        .map(BinaryLiteral)
        .map_err(|err| err.to_string())
}

/// 解析并包装为 HexLiteral。
pub fn NewHexLiteral(input: &str) -> Result<HexLiteral, String> {
    ParseHexStr(input).map(|value| HexLiteral(value.0))
}
impl HexLiteral {
    /// 委托 BinaryLiteral::ToString。
    pub fn ToString(&self) -> String {
        BinaryLiteral(self.0.clone()).ToString()
    }
}

// SetBinChsClnFlag 把字段类型统一标成 binary 字符集、排序规则和标志。
/// 将 FieldType 标为 binary 字符集/排序规则并加上 BinaryFlag。
pub fn SetBinChsClnFlag(field_type: &mut types::FieldType) {
    field_type.SetCharset(charset::CharsetBin.to_owned());
    field_type.SetCollate(charset::CollationBin.to_owned());
    field_type.AddFlag(mysql::BinaryFlag);
}

// MySQL 小数秒默认精度为零。
/// 小数秒精度（FSP）默认值 0。
pub const DefaultFsp: i8 = 0;

// DefaultTypeForValue 对应 Go type switch，为字面值补齐解析器默认 FieldType。
/// 按字面值运行时类型填充默认 FieldType（对齐 Go type switch）。
pub fn DefaultTypeForValue(
    value: &dyn Any,
    field_type: &mut types::FieldType,
    charset_name: &str,
    collate: &str,
) {
    if value.is::<()>() {
        field_type.SetType(mysql::TypeNull);
        field_type.SetFlen(0);
        field_type.SetDecimal(0);
        SetBinChsClnFlag(field_type);
    } else if value.is::<bool>() {
        field_type.SetType(mysql::TypeLonglong);
        field_type.SetFlen(1);
        field_type.SetDecimal(0);
        field_type.AddFlag(mysql::IsBooleanFlag);
        SetBinChsClnFlag(field_type);
    } else if let Some(value) = value.downcast_ref::<i32>() {
        field_type.SetType(mysql::TypeLonglong);
        field_type.SetFlen(StrLenOfInt64Fast(*value as i64) as isize);
        field_type.SetDecimal(0);
        SetBinChsClnFlag(field_type);
    } else if let Some(value) = value.downcast_ref::<i64>() {
        field_type.SetType(mysql::TypeLonglong);
        field_type.SetFlen(StrLenOfInt64Fast(*value) as isize);
        field_type.SetDecimal(0);
        SetBinChsClnFlag(field_type);
    } else if let Some(value) = value.downcast_ref::<u64>() {
        field_type.SetType(mysql::TypeLonglong);
        field_type.AddFlag(mysql::UnsignedFlag);
        field_type.SetFlen(StrLenOfUint64Fast(*value) as isize);
        field_type.SetDecimal(0);
        SetBinChsClnFlag(field_type);
    } else if let Some(value) = value.downcast_ref::<String>() {
        field_type.SetType(mysql::TypeVarString);
        // Go 暂以字符数作为 flen；UTF-8 最大字节数的修正留给原后续工作。
        field_type.SetFlen(value.len() as isize);
        field_type.SetDecimal(types::UnspecifiedLength);
        field_type.SetCharset(charset_name.to_owned());
        field_type.SetCollate(collate.to_owned());
    } else if let Some(value) = value.downcast_ref::<f32>() {
        field_type.SetType(mysql::TypeFloat);
        field_type.SetFlen(format_float_fixed(*value as f64, 32).len() as isize);
        field_type.SetDecimal(types::UnspecifiedLength);
        SetBinChsClnFlag(field_type);
    } else if let Some(value) = value.downcast_ref::<f64>() {
        field_type.SetType(mysql::TypeDouble);
        field_type.SetFlen(format_float_fixed(*value, 64).len() as isize);
        field_type.SetDecimal(types::UnspecifiedLength);
        SetBinChsClnFlag(field_type);
    } else if let Some(value) = value.downcast_ref::<Vec<u8>>() {
        field_type.SetType(mysql::TypeBlob);
        field_type.SetFlen(value.len() as isize);
        field_type.SetDecimal(types::UnspecifiedLength);
        SetBinChsClnFlag(field_type);
    } else if let Some(value) = value.downcast_ref::<BitLiteral>() {
        field_type.SetType(mysql::TypeVarString);
        field_type.SetFlen(value.0.len() as isize);
        field_type.SetDecimal(0);
        SetBinChsClnFlag(field_type);
    } else if let Some(value) = value.downcast_ref::<HexLiteral>() {
        field_type.SetType(mysql::TypeVarString);
        field_type.SetFlen((value.0.len() * 3) as isize);
        field_type.SetDecimal(0);
        field_type.AddFlag(mysql::UnsignedFlag);
        SetBinChsClnFlag(field_type);
    } else if let Some(value) = value.downcast_ref::<BinaryLiteral>() {
        field_type.SetType(mysql::TypeBit);
        field_type.SetFlen((value.0.len() * 8) as isize);
        field_type.SetDecimal(0);
        SetBinChsClnFlag(field_type);
        // BIT 列保留 unsigned，但移除通用 binary flag，与 Go 顺序一致。
        field_type.DelFlag(mysql::BinaryFlag);
        field_type.AddFlag(mysql::UnsignedFlag);
    } else if let Some(value) = value.downcast_ref::<MyDecimal>() {
        field_type.SetType(mysql::TypeNewDecimal);
        field_type.SetFlen(value.String().len() as isize);
        field_type.SetDecimal(value.digitsFrac as isize);
        SetBinChsClnFlag(field_type);
    } else {
        field_type.SetType(mysql::TypeUnspecified);
        field_type.SetFlen(types::UnspecifiedLength);
        field_type.SetDecimal(types::UnspecifiedLength);
    }
}

/// 对齐 strconv.FormatFloat(value, 'f', -1, bits) 的特殊值和有限值文本。
fn format_float_fixed(value: f64, bits: u32) -> String {
    if bits == 32 {
        let value = value as f32;
        if value.is_nan() {
            return "NaN".to_owned();
        }
        if value == f32::INFINITY {
            return "+Inf".to_owned();
        }
        if value == f32::NEG_INFINITY {
            return "-Inf".to_owned();
        }
        return value.to_string();
    }
    if value.is_nan() {
        "NaN".to_owned()
    } else if value == f64::INFINITY {
        "+Inf".to_owned()
    } else if value == f64::NEG_INFINITY {
        "-Inf".to_owned()
    } else {
        value.to_string()
    }
}
