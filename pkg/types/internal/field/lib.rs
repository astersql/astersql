// Copyright 2026 AsterSQL.

// 字段类型相关内部 crate：错误、字面量桩与 FieldType 挂接。
//
// 汇总截断/溢出等标准错误、Binary/Time/Decimal 等桩类型，
// 以及 `field_type` / `fsp` / `helper` 的 include 导出。

#![allow(dead_code, non_snake_case, non_upper_case_globals)]

use std::any::Any;
use std::sync::LazyLock;

pub use ::dbterror::{dbterror, errno, errors};
pub use collate;
pub use mathutil;
pub use parser_types::types as ast;
pub use parser_types::{charset, mysql};

pub use ast::{
    ETDatetime, ETDecimal, ETDuration, ETInt, ETJson, ETReal, ETString, ETTimestamp,
    ETVectorFloat32, EvalType,
};

/// 数据截断警告（WarnDataTruncated）。
pub static ErrTruncated: LazyLock<Box<errors::Error>> =
    LazyLock::new(|| dbterror::ClassTypes.NewStd(errno::WarnDataTruncated));
/// 数值越界（ErrDataOutOfRange）。
pub static ErrOverflow: LazyLock<Box<errors::Error>> =
    LazyLock::new(|| dbterror::ClassTypes.NewStd(errno::ErrDataOutOfRange));
/// 非法数字。
pub static ErrBadNumber: LazyLock<Box<errors::Error>> =
    LazyLock::new(|| dbterror::ClassTypes.NewStd(errno::ErrBadNumber));
/// 字段长度过大。
pub static ErrTooBigFieldLength: LazyLock<Box<errors::Error>> =
    LazyLock::new(|| dbterror::ClassTypes.NewStd(errno::ErrTooBigFieldlength));

fn initialize_standard_errors() {
    LazyLock::force(&ErrTruncated);
    LazyLock::force(&ErrOverflow);
    LazyLock::force(&ErrBadNumber);
    LazyLock::force(&ErrTooBigFieldLength);
}

// Go 在导入包时初始化包级错误变量。把构造函数放入平台启动段，保证
// `RegisterFinish` 冻结错误注册表前按声明顺序注册本 crate 的标准错误。
#[cfg(any(target_family = "unix", target_os = "windows"))]
#[used]
#[cfg_attr(
    all(target_family = "unix", not(target_vendor = "apple")),
    unsafe(link_section = ".init_array")
)]
#[cfg_attr(
    target_vendor = "apple",
    unsafe(link_section = "__DATA,__mod_init_func")
)]
#[cfg_attr(target_os = "windows", unsafe(link_section = ".CRT$XCU"))]
static TYPES_FIELD_PACKAGE_INIT: extern "C" fn() = {
    extern "C" fn initialize() {
        initialize_standard_errors();
    }
    initialize
};

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 二进制字面量字节序列。
pub struct BinaryLiteral(pub Vec<u8>);
#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// BIT 字面量。
pub struct BitLiteral(pub BinaryLiteral);
#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// HEX 字面量。
pub struct HexLiteral(pub BinaryLiteral);

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 时间类型桩：保存 MySQL 类型码与 FSP。
pub struct Time {
    tp: u8,
    fsp: i32,
}

impl Time {
    /// 构造 Time 桩。
    pub fn new(tp: u8, fsp: i32) -> Self {
        Self { tp, fsp }
    }
    /// MySQL 类型码。
    pub fn Type(&self) -> u8 {
        self.tp
    }
    /// 小数秒精度。
    pub fn Fsp(&self) -> i32 {
        self.fsp
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// TIME 时长桩：FSP + 显示串。
pub struct Duration {
    pub Fsp: i32,
    pub display: String,
}

impl Duration {
    /// 显示串；空则返回 00:00:00。
    pub fn String(&self) -> String {
        if self.display.is_empty() {
            "00:00:00".to_owned()
        } else {
            self.display.clone()
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// DECIMAL 桩：小数位数 + 显示串。
pub struct MyDecimal {
    pub digitsFrac: u8,
    pub display: String,
}

impl MyDecimal {
    /// 显示串；空则返回 "0"。
    pub fn ToString(&self) -> String {
        if self.display.is_empty() {
            "0".to_owned()
        } else {
            self.display.clone()
        }
    }
    /// 小数位数。
    pub fn GetDigitsFrac(&mut self) -> i8 {
        self.digitsFrac as i8
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// ENUM 值：名称与数值。
pub struct Enum {
    pub Name: String,
    pub Value: u64,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// SET 值：名称与位掩码数值。
pub struct Set {
    pub Name: String,
    pub Value: u64,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// JSON 二进制表示桩。
pub struct BinaryJSON;

#[derive(Clone, Debug, Default, PartialEq)]
/// 向量类型桩。
pub struct VectorFloat32(pub Vec<f32>);

#[derive(Default)]
/// 简化 Datum：动态值 + Kind + 排序规则。
pub struct Datum {
    value: Option<Box<dyn Any>>,
    pub k: u8,
    pub collation: String,
}

impl Datum {
    /// 取内部动态值。
    pub fn GetValue(&self) -> Option<&dyn Any> {
        self.value.as_deref()
    }
}

/// Datum Kind：字符串。
pub const KindString: u8 = 5;
/// Datum Kind：字节。
pub const KindBytes: u8 = 6;

/// 是否为 BLOB 族类型。
pub fn IsTypeBlob(tp: u8) -> bool {
    ast::IsTypeBlob(tp)
}
/// 是否为定长字符类型。
pub fn IsTypeChar(tp: u8) -> bool {
    ast::IsTypeChar(tp)
}
/// 是否为 VARCHAR / VARSTRING。
pub fn IsTypeVarchar(tp: u8) -> bool {
    tp == mysql::TypeVarString || tp == mysql::TypeVarchar
}
/// 是否为整数类型。
pub fn IsTypeInteger(tp: u8) -> bool {
    matches!(
        tp,
        mysql::TypeTiny
            | mysql::TypeShort
            | mysql::TypeInt24
            | mysql::TypeLong
            | mysql::TypeLonglong
            | mysql::TypeYear
    )
}
/// 是否为日期时间类型（含 DATE/DATETIME/TIMESTAMP）。
pub fn IsTypeTime(tp: u8) -> bool {
    matches!(
        tp,
        mysql::TypeDatetime | mysql::TypeDate | mysql::TypeTimestamp
    )
}
/// 是否为字符串类类型（含未指定）。
pub fn IsString(tp: u8) -> bool {
    IsTypeChar(tp) || IsTypeBlob(tp) || IsTypeVarchar(tp) || tp == mysql::TypeUnspecified
}
/// 是否为 binary 校对的字符串列。
pub fn IsBinaryStr(ft: &FieldType) -> bool {
    ft.GetCollate() == charset::CollationBin && IsString(ft.GetType())
}
/// Datum Kind 是否为字符串或字节。
pub fn IsStringKind(kind: u8) -> bool {
    kind == KindString || kind == KindBytes
}

/// 挂接 FieldType 生产实现。
pub mod field_type {
    use crate::*;
    include!("../../field_type.rs");
}
pub use field_type::*;

/// 挂接 FSP 生产实现。
pub mod fsp {
    use crate::*;
    include!("../../fsp.rs");
}
pub use fsp::*;

/// 挂接 helper 生产实现。
pub mod helper {
    use crate::*;
    include!("../../helper.rs");
}
pub use helper::*;

#[cfg(test)]
mod migration_aster_unit_test;
