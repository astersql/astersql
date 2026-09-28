// Copyright 2026 AsterSQL.
// Copyright 2014 The ql Authors. All rights reserved.
// Use of this source code is governed by a BSD-style
// license that can be found in the LICENSES/QL-LICENSE file.

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
// See the License for the specific language governing permissions and
// limitations under the License.

// 字段类型码辅助工具：BLOB/CHAR/向量判定、类型名双向映射与类型错误常量。
//
// 对照 `pkg/parser/types/etc.go`：保留辅助函数、映射表与 terror 错误声明顺序。
// 类型码（type code）是 MySQL/TiDB 内部用一字节标识列类型的枚举值。

// 本文件对照 pkg/parser/types/etc.go，保留类型辅助函数、映射和错误声明的顺序。

use std::collections::HashMap;
use std::sync::LazyLock;

// IsTypeBlob 对应 Go 的同名函数，判断类型码是否属于四种 BLOB 容量规格。
/// 判断类型码是否属于 Tiny/Medium/Blob/LongBlob 四种 BLOB 规格。
pub fn IsTypeBlob(tp: u8) -> bool {
    matches!(
        tp,
        mysql::TypeTinyBlob | mysql::TypeMediumBlob | mysql::TypeBlob | mysql::TypeLongBlob
    )
}

// IsTypeChar 对应 Go 的字符类型判定，仅包含定长 CHAR 与 VARCHAR。
/// 判断是否为定长 CHAR（TypeString）或 VARCHAR。
pub fn IsTypeChar(tp: u8) -> bool {
    tp == mysql::TypeString || tp == mysql::TypeVarchar
}

// IsTypeVector 对应 Go 的向量类型判定；当前只有 TiDBVectorFloat32 一个类型码。
/// 判断是否为 TiDB 向量类型（当前仅 float32 向量码）。
pub fn IsTypeVector(tp: u8) -> bool {
    tp == mysql::TypeTiDBVectorFloat32
}

// type2Str 保留 Go 的 byte -> string 全局映射；LazyLock 只替代 Go 包初始化，不产生外部 IO。
/// 类型码到规范类型名的懒加载映射表。
static type2Str: LazyLock<HashMap<u8, &'static str>> = LazyLock::new(|| {
    HashMap::from([
        (mysql::TypeBit, "bit"),
        (mysql::TypeBlob, "text"),
        (mysql::TypeDate, "date"),
        (mysql::TypeDatetime, "datetime"),
        (mysql::TypeUnspecified, "unspecified"),
        (mysql::TypeNewDecimal, "decimal"),
        (mysql::TypeDouble, "double"),
        (mysql::TypeEnum, "enum"),
        (mysql::TypeFloat, "float"),
        (mysql::TypeGeometry, "geometry"),
        (mysql::TypeTiDBVectorFloat32, "vector"),
        (mysql::TypeInt24, "mediumint"),
        (mysql::TypeJSON, "json"),
        (mysql::TypeLong, "int"),
        (mysql::TypeLonglong, "bigint"),
        (mysql::TypeLongBlob, "longtext"),
        (mysql::TypeMediumBlob, "mediumtext"),
        (mysql::TypeNull, "null"),
        (mysql::TypeSet, "set"),
        (mysql::TypeShort, "smallint"),
        (mysql::TypeString, "char"),
        (mysql::TypeDuration, "time"),
        (mysql::TypeTimestamp, "timestamp"),
        (mysql::TypeTiny, "tinyint"),
        (mysql::TypeTinyBlob, "tinytext"),
        (mysql::TypeVarchar, "varchar"),
        (mysql::TypeVarString, "var_string"),
        (mysql::TypeYear, "year"),
    ])
});

// str2Type 是 type2Str 的反向表，保持 Go 中可接受的规范化类型名集合。
/// 规范类型名到类型码的反向懒加载映射表。
static str2Type: LazyLock<HashMap<&'static str, u8>> = LazyLock::new(|| {
    type2Str.iter().map(|(tp, name)| (*name, *tp)).collect()
});

// TypeStr 将类型码转换为规范名称；未知码与 Go map 的零值语义对应为空字符串。
/// 将类型码转为规范名称；未知码返回空串。
pub fn TypeStr(tp: u8) -> &'static str {
    type2Str.get(&tp).copied().unwrap_or("")
}

// TypeToStr 对应 Go 的字段类型显示转换：binary 字符集会把 text/char 名称改成 blob/binary。
/// 按字符集调整显示名：binary 下 text→blob、char→binary。
pub fn TypeToStr(tp: u8, cs: &str) -> String {
    let mut ts = TypeStr(tp).to_owned();
    if cs != "binary" {
        return ts;
    }

    // replace_once 对应 strings.Replace(..., 1)，只改首个匹配片段。
    if IsTypeBlob(tp) {
        ts = ts.replacen("text", "blob", 1);
    } else if IsTypeChar(tp) {
        ts = ts.replacen("char", "binary", 1);
    } else if tp == mysql::TypeNull {
        ts = "binary".to_owned();
    }
    ts
}

// StrToType 对应 Go 的名称解析：先把 BLOB/BINARY 别名归一化，再查询反向表。
/// 将类型名解析为类型码；先归一化 blob/binary 别名，未知名返回 Unspecified。
pub fn StrToType(ts: &str) -> u8 {
    let ts = ts.replacen("blob", "text", 1);
    let ts = ts.replacen("binary", "char", 1);
    str2Type
        .get(ts.as_str())
        .copied()
        .unwrap_or(mysql::TypeUnspecified)
}

// dig2bytes、digitsPerWord 与 wordSize 共同描述十进制数按 9 位一组的存储长度计算规则。
/// 余下 0..=9 个十进制数字各自需要的存储字节数查表。
pub static dig2bytes: [isize; 10] = [0, 1, 1, 2, 2, 3, 3, 4, 4, 4];
/// 一个 word 容纳的十进制数字个数（9）。
pub const digitsPerWord: isize = 9; // 一个 word 容纳 9 个十进制数字。
/// 一个 word 占用的字节数（4，对应 int32）。
pub const wordSize: isize = 4; // 一个 word 使用 4 字节 int32。

// 以下错误逐一对应 Go 的 terror.ClassTypes.NewStd；LazyLock 保留包级初始化时机。
/// 无效默认值错误（对应 mysql.ErrInvalidDefault）。
pub static ErrInvalidDefault: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| terror::ClassTypes.NewStd(terror::ErrCode(mysql::ErrInvalidDefault as isize)));

// ErrDataOutOfRange 表示值超出目标类型可表示范围。
/// 数据超出类型可表示范围的错误。
pub static ErrDataOutOfRange: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| terror::ClassTypes.NewStd(terror::ErrCode(mysql::ErrDataOutOfRange as isize)));

// ErrTruncatedWrongValue 表示解析值超过 Go 源文件记录的最大十进制范围。
/// 截断/错误数值错误（对应 mysql.ErrTruncatedWrongValue）。
pub static ErrTruncatedWrongValue: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| terror::ClassTypes.NewStd(terror::ErrCode(mysql::ErrTruncatedWrongValue as isize)));

// ErrIllegalValueForType 对应 strconv.ParseFloat 遇到 ErrRange 时返回的类型错误。
/// 类型非法取值错误（对应 mysql.ErrIllegalValueForType）。
pub static ErrIllegalValueForType: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| terror::ClassTypes.NewStd(terror::ErrCode(mysql::ErrIllegalValueForType as isize)));

/// 在 crate 启动钩子中按 Go 包级变量顺序注册全部标准错误。
pub(crate) fn initialize_standard_errors() {
    LazyLock::force(&ErrInvalidDefault);
    LazyLock::force(&ErrDataOutOfRange);
    LazyLock::force(&ErrTruncatedWrongValue);
    LazyLock::force(&ErrIllegalValueForType);
}
