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
// See the License for the specific language governing permissions and
// limitations under the License.

// MySQL 协议字段类型编号与列标志位定义。
//
// 类型常量对应 MySQL 二进制协议中的 `field_type` 字节；标志位描述列属性
//（如 NOT NULL、主键、自增）。本模块只提供常量与位检测函数，不访问存储。

// MySQL type information.
// 这些编号直接来自 MySQL 协议；使用 u8 保留 Go byte 的取值范围和按字节传输语义。
/// TypeUnspecified 表示未指定类型（协议值 0）。
pub const TypeUnspecified: u8 = 0;
/// TypeTiny 对应 TINYINT。
pub const TypeTiny: u8 = 1; // TINYINT
/// TypeShort 对应 SMALLINT。
pub const TypeShort: u8 = 2; // SMALLINT
/// TypeLong 对应 INT。
pub const TypeLong: u8 = 3; // INT
/// TypeFloat 对应 FLOAT。
pub const TypeFloat: u8 = 4;
/// TypeDouble 对应 DOUBLE。
pub const TypeDouble: u8 = 5;
/// TypeNull 表示 NULL 类型占位。
pub const TypeNull: u8 = 6;
/// TypeTimestamp 对应 TIMESTAMP。
pub const TypeTimestamp: u8 = 7;
/// TypeLonglong 对应 BIGINT。
pub const TypeLonglong: u8 = 8; // BIGINT
/// TypeInt24 对应 MEDIUMINT。
pub const TypeInt24: u8 = 9; // MEDIUMINT
/// TypeDate 对应 DATE。
pub const TypeDate: u8 = 10;
// Go 原名 TypeTime 会与 Go 的 Time 类型冲突，因此源码改名为 TypeDuration；Rust 沿用该公开名。
/// TypeDuration 对应 TIME（时长），非墙上时钟。
pub const TypeDuration: u8 = 11;
/// TypeDatetime 对应 DATETIME。
pub const TypeDatetime: u8 = 12;
/// TypeYear 对应 YEAR。
pub const TypeYear: u8 = 13;
/// TypeNewDate 为协议保留的新 DATE 编号（历史兼容）。
pub const TypeNewDate: u8 = 14;
/// TypeVarchar 对应 VARCHAR。
pub const TypeVarchar: u8 = 15;
/// TypeBit 对应 BIT。
pub const TypeBit: u8 = 16;

/// TypeJSON 对应 JSON 类型（协议 0xf5）。
pub const TypeJSON: u8 = 0xf5;
/// TypeNewDecimal 对应 DECIMAL/NUMERIC。
pub const TypeNewDecimal: u8 = 0xf6;
/// TypeEnum 对应 ENUM。
pub const TypeEnum: u8 = 0xf7;
/// TypeSet 对应 SET。
pub const TypeSet: u8 = 0xf8;
/// TypeTinyBlob 对应 TINYBLOB/TINYTEXT。
pub const TypeTinyBlob: u8 = 0xf9;
/// TypeMediumBlob 对应 MEDIUMBLOB/MEDIUMTEXT。
pub const TypeMediumBlob: u8 = 0xfa;
/// TypeLongBlob 对应 LONGBLOB/LONGTEXT。
pub const TypeLongBlob: u8 = 0xfb;
/// TypeBlob 对应 BLOB/TEXT。
pub const TypeBlob: u8 = 0xfc;
/// TypeVarString 对应协议中的 VAR_STRING。
pub const TypeVarString: u8 = 0xfd;
/// TypeString 对应 CHAR / 定长字符串。
pub const TypeString: u8 = 0xfe; // TypeString is char type.
/// TypeGeometry 对应空间几何类型。
pub const TypeGeometry: u8 = 0xff;

// TiDB 向量类型使用协议保留区中的独立编号。
/// TypeTiDBVectorFloat32 为 TiDB 向量 float32 扩展类型。
pub const TypeTiDBVectorFloat32: u8 = 0xe1;

// Flag information.
// Go 的 uint 与平台字宽相关；这里用 usize 保留同类位掩码运算，当前最高位只使用到第 24 位。
/// NotNullFlag 表示列不允许 NULL。
pub const NotNullFlag: usize = 1 << 0; // Field can't be NULL.
/// PriKeyFlag 表示列属于主键。
pub const PriKeyFlag: usize = 1 << 1; // Field is part of a primary key.
/// UniqueKeyFlag 表示列属于唯一键。
pub const UniqueKeyFlag: usize = 1 << 2; // Field is part of a unique key.
/// MultipleKeyFlag 表示列属于某个索引键。
pub const MultipleKeyFlag: usize = 1 << 3; // Field is part of a key.
/// BlobFlag 表示列为 BLOB/TEXT 类。
pub const BlobFlag: usize = 1 << 4; // Field is a blob.
/// UnsignedFlag 表示无符号数值列。
pub const UnsignedFlag: usize = 1 << 5; // Field is unsigned.
/// ZerofillFlag 表示 ZEROFILL 显示属性。
pub const ZerofillFlag: usize = 1 << 6; // Field is zerofill.
/// BinaryFlag 表示二进制比较/存储语义。
pub const BinaryFlag: usize = 1 << 7; // Field is binary.
/// EnumFlag 表示 ENUM 列。
pub const EnumFlag: usize = 1 << 8; // Field is an enum.
/// AutoIncrementFlag 表示自增列。
pub const AutoIncrementFlag: usize = 1 << 9; // Field is an auto increment field.
/// TimestampFlag 表示 TIMESTAMP 列。
pub const TimestampFlag: usize = 1 << 10; // Field is a timestamp.
/// SetFlag 表示 SET 列。
pub const SetFlag: usize = 1 << 11; // Field is a set.
/// NoDefaultValueFlag 表示列没有默认值。
pub const NoDefaultValueFlag: usize = 1 << 12; // Field doesn't have a default value.
/// OnUpdateNowFlag 表示 ON UPDATE CURRENT_TIMESTAMP。
pub const OnUpdateNowFlag: usize = 1 << 13; // Field is set to NOW on UPDATE.
/// PartKeyFlag 表示列参与某些键（内部标记）。
pub const PartKeyFlag: usize = 1 << 14; // Intern: Part of some keys.
/// NumFlag 表示数值列（供客户端识别）。
pub const NumFlag: usize = 1 << 15; // Field is a num (for clients).

// 下列标志为解析器和 TiDB 内部约定；GroupFlag 与 NumFlag 按 Go 源码有意复用同一位。
/// GroupFlag 表示 GROUP BY 相关内部标记（与 NumFlag 同位）。
pub const GroupFlag: usize = 1 << 15; // Internal: Group field.
/// UniqueFlag 为 yacc 解析器用的唯一性内部标记。
pub const UniqueFlag: usize = 1 << 16; // Internal: Used by sql_yacc.
/// BinCmpFlag 为 yacc 解析器用的二进制比较内部标记。
pub const BinCmpFlag: usize = 1 << 17; // Internal: Used by sql_yacc.
/// ParseToJSONFlag 表示 CAST 时将字符串解析为 JSON。
pub const ParseToJSONFlag: usize = 1 << 18; // Internal: Parse string to JSON in CAST.
/// IsBooleanFlag 区分布尔字面量与普通整数。
pub const IsBooleanFlag: usize = 1 << 19; // Distinguish a boolean literal from an integer.
/// PreventNullInsertFlag 阻止向该列插入 NULL。
pub const PreventNullInsertFlag: usize = 1 << 20; // Prevent inserting NULL values.
/// EnumSetAsIntFlag 表示将 ENUM/SET 按整数推断求值类型。
pub const EnumSetAsIntFlag: usize = 1 << 21; // Internal: Infer enum evaluation type.
/// DropColumnIndexFlag 表示正在随索引一起删除该列。
pub const DropColumnIndexFlag: usize = 1 << 22; // Column is being dropped with an index.
/// GeneratedColumnFlag 表示生成列（TiFlash 可为其加占位）。
pub const GeneratedColumnFlag: usize = 1 << 23; // TiFlash adds a placeholder for this column.
/// UnderScoreCharsetFlag 表示使用了 `_charset'literal'` 形式指定字符集。
pub const UnderScoreCharsetFlag: usize = 1 << 24; // Charset was specified as _latin1'abc'.

// TypeInt24 bounds.
// 明确标为 i32，避免 Rust 对无类型负移位表达式的推断与 Go 常量规则产生歧义。
/// MaxUint24 为 24 位无符号最大值。
pub const MaxUint24: i32 = (1 << 24) - 1;
/// MaxInt24 为 24 位有符号最大值。
pub const MaxInt24: i32 = (1 << 23) - 1;
/// MinInt24 为 24 位有符号最小值。
pub const MinInt24: i32 = -(1 << 23);

// 下列函数逐一对应 Go 的 Has*Flag；统一委托 HasFlag，语义仍是目标位非零即为已设置。
/// HasDropColumnWithIndexFlag 检测 DropColumnIndexFlag。
pub fn HasDropColumnWithIndexFlag(flag: usize) -> bool {
    HasFlag(flag, DropColumnIndexFlag)
}
/// HasNotNullFlag 检测 NotNullFlag。
pub fn HasNotNullFlag(flag: usize) -> bool {
    HasFlag(flag, NotNullFlag)
}
/// HasNoDefaultValueFlag 检测 NoDefaultValueFlag。
pub fn HasNoDefaultValueFlag(flag: usize) -> bool {
    HasFlag(flag, NoDefaultValueFlag)
}
/// HasAutoIncrementFlag 检测 AutoIncrementFlag。
pub fn HasAutoIncrementFlag(flag: usize) -> bool {
    HasFlag(flag, AutoIncrementFlag)
}
/// HasUnsignedFlag 检测 UnsignedFlag。
pub fn HasUnsignedFlag(flag: usize) -> bool {
    HasFlag(flag, UnsignedFlag)
}
/// HasZerofillFlag 检测 ZerofillFlag。
pub fn HasZerofillFlag(flag: usize) -> bool {
    HasFlag(flag, ZerofillFlag)
}
/// HasBinaryFlag 检测 BinaryFlag。
pub fn HasBinaryFlag(flag: usize) -> bool {
    HasFlag(flag, BinaryFlag)
}
/// HasPriKeyFlag 检测 PriKeyFlag。
pub fn HasPriKeyFlag(flag: usize) -> bool {
    HasFlag(flag, PriKeyFlag)
}
/// HasUniKeyFlag 检测 UniqueKeyFlag。
pub fn HasUniKeyFlag(flag: usize) -> bool {
    HasFlag(flag, UniqueKeyFlag)
}
/// HasMultipleKeyFlag 检测 MultipleKeyFlag。
pub fn HasMultipleKeyFlag(flag: usize) -> bool {
    HasFlag(flag, MultipleKeyFlag)
}
/// HasTimestampFlag 检测 TimestampFlag。
pub fn HasTimestampFlag(flag: usize) -> bool {
    HasFlag(flag, TimestampFlag)
}
/// HasOnUpdateNowFlag 检测 OnUpdateNowFlag。
pub fn HasOnUpdateNowFlag(flag: usize) -> bool {
    HasFlag(flag, OnUpdateNowFlag)
}
/// HasParseToJSONFlag 检测 ParseToJSONFlag。
pub fn HasParseToJSONFlag(flag: usize) -> bool {
    HasFlag(flag, ParseToJSONFlag)
}
/// HasIsBooleanFlag 检测 IsBooleanFlag。
pub fn HasIsBooleanFlag(flag: usize) -> bool {
    HasFlag(flag, IsBooleanFlag)
}
/// HasPreventNullInsertFlag 检测 PreventNullInsertFlag。
pub fn HasPreventNullInsertFlag(flag: usize) -> bool {
    HasFlag(flag, PreventNullInsertFlag)
}
/// HasEnumSetAsIntFlag 检测 EnumSetAsIntFlag。
pub fn HasEnumSetAsIntFlag(flag: usize) -> bool {
    HasFlag(flag, EnumSetAsIntFlag)
}

// HasFlag 对应 Go 的通用位检测函数，不修改输入，也没有 IO 或外部依赖。
/// HasFlag 用按位与判断 `flag` 中是否设置了 `flagItem`。
pub fn HasFlag(flag: usize, flagItem: usize) -> bool {
    (flag & flagItem) > 0
}
