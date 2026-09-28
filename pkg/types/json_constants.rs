// Copyright 2026 AsterSQL.
// Copyright 2017 PingCAP, Inc.
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

// MySQL 二进制 JSON（Binary JSON）类型码、字面量与错误常量定义。
//
// 对齐 Go `types` 包中 JSON 相关常量：类型码描述 on-wire 布局，
// `JSONModify*` 描述 JSON_SET/INSERT/REPLACE 语义，`JsonError` 承载校验失败信息。

#![allow(non_upper_case_globals)]

use thiserror::Error;

/// JSONTypeCode indicates the on-wire MySQL binary JSON type.
/// 二进制 JSON 载荷的类型码（单字节），标识对象/数组/字面量/数值等。
pub type JSONTypeCode = u8;

/// 对象类型码。
pub const JSONTypeCodeObject: JSONTypeCode = 0x01;
/// 数组类型码。
pub const JSONTypeCodeArray: JSONTypeCode = 0x03;
/// 字面量（null/true/false）类型码。
pub const JSONTypeCodeLiteral: JSONTypeCode = 0x04;
/// 有符号 64 位整数类型码。
pub const JSONTypeCodeInt64: JSONTypeCode = 0x09;
/// 无符号 64 位整数类型码。
pub const JSONTypeCodeUint64: JSONTypeCode = 0x0a;
/// 双精度浮点类型码。
pub const JSONTypeCodeFloat64: JSONTypeCode = 0x0b;
/// 字符串类型码。
pub const JSONTypeCodeString: JSONTypeCode = 0x0c;
/// 不透明（opaque）载荷类型码，用于非标准 JSON 扩展类型。
pub const JSONTypeCodeOpaque: JSONTypeCode = 0x0d;
/// DATE 类型码。
pub const JSONTypeCodeDate: JSONTypeCode = 0x0e;
/// DATETIME 类型码。
pub const JSONTypeCodeDatetime: JSONTypeCode = 0x0f;
/// TIMESTAMP 类型码。
pub const JSONTypeCodeTimestamp: JSONTypeCode = 0x10;
/// DURATION/TIME 类型码。
pub const JSONTypeCodeDuration: JSONTypeCode = 0x11;

/// 字面量：JSON null。
pub const JSONLiteralNil: u8 = 0x00;
/// 字面量：JSON true。
pub const JSONLiteralTrue: u8 = 0x01;
/// 字面量：JSON false。
pub const JSONLiteralFalse: u8 = 0x02;

/// 未知类型码错误文案。
pub const unknownTypeCodeErrorMsg: &str = "unknown type code: %d";
/// 未知类型错误文案。
pub const unknownTypeErrorMsg: &str = "unknown type: %s";

/// 构造 ASCII 可直接输出的 JSON 字符安全表（排除 `"` 与 `\`）。
const fn make_json_safe_set() -> [bool; 128] {
    let mut set = [false; 128];
    let mut byte = 0x20;
    while byte < 128 {
        set[byte] = byte != b'"' as usize && byte != b'\\' as usize;
        byte += 1;
    }
    set
}

/// 可安全原样写入 JSON 字符串的 ASCII 字符表。
pub(crate) const jsonSafeSet: [bool; 128] = make_json_safe_set();
/// `\uXXXX` 转义用的十六进制字符表。
pub(crate) const jsonHexChars: &[u8; 16] = b"0123456789abcdef";

/// 数组/对象头部大小：元素个数(4) + 数据总长(4)。
pub const headerSize: usize = 8;
/// 头部中“数据总字节数”字段的偏移。
pub const dataSizeOff: usize = 4;
/// 对象 key entry 大小：key 偏移(4) + key 长度(2)。
pub const keyEntrySize: usize = 6;
/// key entry 内 key 长度字段的偏移。
pub const keyLenOff: usize = 4;
/// value entry 中类型码所占字节数。
pub const valTypeSize: usize = 1;
/// value entry 总大小：类型码(1) + 偏移或内联字面量(4)。
pub const valEntrySize: usize = 5;

/// The comparison precedence table from MySQL 5.7, kept as a function so no
/// mutable global map is required.
/// MySQL 5.7 JSON 比较优先级表：数值越小优先级越低；同名返回对应序值。
pub fn json_type_precedence(type_name: &str) -> Option<i32> {
    Some(match type_name {
        "BLOB" => -1,
        "BIT" => -2,
        "OPAQUE" => -3,
        "DATETIME" => -4,
        "TIME" => -5,
        "DATE" => -6,
        "BOOLEAN" => -7,
        "ARRAY" => -8,
        "OBJECT" => -9,
        "STRING" => -10,
        "INTEGER" | "UNSIGNED INTEGER" | "DOUBLE" => -11,
        "NULL" => -12,
        _ => return None,
    })
}

/// JSON 修改操作类型（对应 JSON_INSERT / REPLACE / SET）。
pub type JSONModifyType = u8;
/// 仅在路径不存在时插入。
pub const JSONModifyInsert: JSONModifyType = 0x01;
/// 仅在路径已存在时替换。
pub const JSONModifyReplace: JSONModifyType = 0x02;
/// 存在则替换，不存在则插入。
pub const JSONModifySet: JSONModifyType = 0x03;

/// JSON 相关错误种类，对齐 Go 侧错误码分类。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum JsonErrorKind {
    InvalidJsonText,
    InvalidJsonType,
    InvalidJsonTextInParam,
    InvalidJsonPath,
    InvalidJsonCharset,
    InvalidJsonData,
    InvalidJsonPathMultipleSelection,
    InvalidJsonContainsPathType,
    JsonDocumentNullKey,
    DocumentTooDeep,
    ObjectKeyTooLong,
    InvalidJsonPathArrayCell,
    UnsupportedSecondArgumentType,
    UnsupportedValue,
    UnknownType,
    InvalidBinaryData,
}

/// 带种类与消息的 JSON 错误，供上层函数返回。
#[derive(Clone, Debug, Eq, Error, PartialEq)]
#[error("{message}")]
pub struct JsonError {
    kind: JsonErrorKind,
    message: String,
}

impl JsonError {
    /// 构造指定种类与消息的错误。
    pub fn new(kind: impl Into<JsonErrorKind>, message: impl Into<String>) -> Self {
        Self {
            kind: kind.into(),
            message: message.into(),
        }
    }

    /// 返回错误种类。
    pub fn kind(&self) -> JsonErrorKind {
        self.kind
    }
}

/// Copyable standard-error descriptor. Dereferencing exposes the Go terror API;
/// conversion to a kind preserves the existing Rust error-construction API.
#[derive(Clone, Copy, Debug)]
pub struct JsonErrorDefinition {
    kind: JsonErrorKind,
    error: fn() -> &'static dbterror::terror::Error,
}

impl From<JsonErrorDefinition> for JsonErrorKind {
    fn from(error: JsonErrorDefinition) -> Self {
        error.kind
    }
}

impl std::ops::Deref for JsonErrorDefinition {
    type Target = dbterror::terror::Error;

    fn deref(&self) -> &Self::Target {
        (self.error)()
    }
}

macro_rules! standard_json_error {
    ($name:ident, $kind:ident, $class:ident) => {
        pub const $name: JsonErrorDefinition = JsonErrorDefinition {
            kind: JsonErrorKind::$kind,
            error: || {
                static ERROR: std::sync::LazyLock<Box<dbterror::terror::Error>> =
                    std::sync::LazyLock::new(|| dbterror::$class.NewStd(dbterror::errno::$name));
                &ERROR
            },
        };
    };
}

standard_json_error!(ErrInvalidJSONText, InvalidJsonText, ClassJSON);
standard_json_error!(ErrInvalidJSONType, InvalidJsonType, ClassJSON);
standard_json_error!(ErrInvalidJSONTextInParam, InvalidJsonTextInParam, ClassJSON);
standard_json_error!(ErrInvalidJSONPath, InvalidJsonPath, ClassJSON);
standard_json_error!(ErrInvalidJSONCharset, InvalidJsonCharset, ClassJSON);
standard_json_error!(ErrInvalidJSONData, InvalidJsonData, ClassJSON);
standard_json_error!(
    ErrInvalidJSONPathMultipleSelection,
    InvalidJsonPathMultipleSelection,
    ClassJSON
);
standard_json_error!(
    ErrInvalidJSONContainsPathType,
    InvalidJsonContainsPathType,
    ClassJSON
);
standard_json_error!(ErrJSONDocumentNULLKey, JsonDocumentNullKey, ClassJSON);
standard_json_error!(ErrJSONDocumentTooDeep, DocumentTooDeep, ClassJSON);
standard_json_error!(ErrJSONObjectKeyTooLong, ObjectKeyTooLong, ClassTypes);
standard_json_error!(
    ErrInvalidJSONPathArrayCell,
    InvalidJsonPathArrayCell,
    ClassJSON
);
standard_json_error!(
    ErrUnsupportedSecondArgumentType,
    UnsupportedSecondArgumentType,
    ClassJSON
);

// Match Go package initialization: register the standard codes before use.
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
static JSON_ERRORS_INIT: extern "C" fn() = {
    extern "C" fn initialize() {
        let _ = &*ErrInvalidJSONText;
        let _ = &*ErrInvalidJSONType;
        let _ = &*ErrInvalidJSONTextInParam;
        let _ = &*ErrInvalidJSONPath;
        let _ = &*ErrInvalidJSONCharset;
        let _ = &*ErrInvalidJSONData;
        let _ = &*ErrInvalidJSONPathMultipleSelection;
        let _ = &*ErrInvalidJSONContainsPathType;
        let _ = &*ErrJSONDocumentNULLKey;
        let _ = &*ErrJSONDocumentTooDeep;
        let _ = &*ErrJSONObjectKeyTooLong;
        let _ = &*ErrInvalidJSONPathArrayCell;
        let _ = &*ErrUnsupportedSecondArgumentType;
    }
    initialize
};

/// JSON_CONTAINS_PATH 要求全部路径命中。
pub const JSONContainsPathAll: &str = "all";
/// JSON_CONTAINS_PATH 任一路径命中即可。
pub const JSONContainsPathOne: &str = "one";
