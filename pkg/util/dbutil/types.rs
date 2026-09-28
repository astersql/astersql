// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

// MySQL 字段类型字节常量与类型归类判断。
//
// 对应 Go/MySQL `FieldType` 中的 type 字节（如 `TypeLonglong`=8、`TypeNewDecimal`=246）。
// 供扫描、比较等路径按整数/浮点/需解码时间类型分流。

/// DECIMAL（旧/占位，值为 0）。
pub const TypeDecimal: u8 = 0;
/// TINYINT。
pub const TypeTiny: u8 = 1;
/// SMALLINT。
pub const TypeShort: u8 = 2;
/// INT。
pub const TypeLong: u8 = 3;
/// FLOAT。
pub const TypeFloat: u8 = 4;
/// DOUBLE。
pub const TypeDouble: u8 = 5;
/// TIMESTAMP。
pub const TypeTimestamp: u8 = 7;
/// BIGINT。
pub const TypeLonglong: u8 = 8;
/// MEDIUMINT。
pub const TypeInt24: u8 = 9;
/// DATE。
pub const TypeDate: u8 = 10;
/// DATETIME。
pub const TypeDatetime: u8 = 12;
/// YEAR。
pub const TypeYear: u8 = 13;
/// 新 DECIMAL（MySQL NEWDECIMAL，值为 246）。
pub const TypeNewDecimal: u8 = 246;

/// 是否为整数类类型（含 YEAR）。
pub fn IsNumberType(tp: u8) -> bool {
    matches!(
        tp,
        TypeTiny | TypeShort | TypeLong | TypeLonglong | TypeInt24 | TypeYear
    )
}
/// 是否为浮点/定点数类型（FLOAT、DOUBLE、NEWDECIMAL）。
pub fn IsFloatType(tp: u8) -> bool {
    matches!(tp, TypeFloat | TypeDouble | TypeNewDecimal)
}
/// 是否为需从二进制解码的时间类型（DATETIME、TIMESTAMP、DATE）。
pub fn IsTimeTypeAndNeedDecode(tp: u8) -> bool {
    matches!(tp, TypeDatetime | TypeTimestamp | TypeDate)
}
