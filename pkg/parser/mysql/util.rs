// Copyright 2026 AsterSQL.
// Copyright 2015 PingCAP, Inc.
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     http://www.apache.org/licenses/LICENSE-2.0
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// MySQL 类型与认证相关的进程内工具函数。
//
// 提供字段默认显示长度/小数位查询、整数类型判断，以及是否为明文认证插件的判定。
// 只读取静态常量表，不访问网络或密码存储。

// 查询进程内常量表。
use std::collections::HashMap;
use std::sync::LazyLock;

use super::r#const::{AuthCachingSha2Password, AuthNativePassword, AuthTiDBSM3Password};
// 同目录 type.rs 提供 MySQL 协议类型编号；raw identifier 用于避开 Rust 的 type 关键字。
use super::r#type::*;

// LengthAndDecimal 对应 Go 的私有结构体 lengthAndDecimal，length 保留 int64，decimal 对应 Go int。
/// LengthAndDecimal 保存某协议类型的默认显示长度与小数位数。
#[derive(Clone, Copy)]
struct LengthAndDecimal {
    length: i64,
    decimal: isize,
}

// defaultLengthAndDecimal 对应 Go 的全局 map：字段在 CREATE TABLE 中未显式指定时返回这些默认值。
// LazyLock 只负责首次访问时构造只读表，不涉及异步、IO 或外部资源收尾。
/// CREATE TABLE 未指定长度时使用的默认 flen/decimal 表。
static defaultLengthAndDecimal: LazyLock<HashMap<u8, LengthAndDecimal>> = LazyLock::new(|| {
    HashMap::from([
        (
            TypeBit,
            LengthAndDecimal {
                length: 1,
                decimal: 0,
            },
        ),
        (
            TypeTiny,
            LengthAndDecimal {
                length: 4,
                decimal: 0,
            },
        ),
        (
            TypeShort,
            LengthAndDecimal {
                length: 6,
                decimal: 0,
            },
        ),
        (
            TypeInt24,
            LengthAndDecimal {
                length: 9,
                decimal: 0,
            },
        ),
        (
            TypeLong,
            LengthAndDecimal {
                length: 11,
                decimal: 0,
            },
        ),
        (
            TypeLonglong,
            LengthAndDecimal {
                length: 20,
                decimal: 0,
            },
        ),
        (
            TypeDouble,
            LengthAndDecimal {
                length: 22,
                decimal: -1,
            },
        ),
        (
            TypeFloat,
            LengthAndDecimal {
                length: 12,
                decimal: -1,
            },
        ),
        (
            TypeNewDecimal,
            LengthAndDecimal {
                length: 10,
                decimal: 0,
            },
        ),
        (
            TypeDuration,
            LengthAndDecimal {
                length: 10,
                decimal: 0,
            },
        ),
        (
            TypeDate,
            LengthAndDecimal {
                length: 10,
                decimal: 0,
            },
        ),
        (
            TypeTimestamp,
            LengthAndDecimal {
                length: 19,
                decimal: 0,
            },
        ),
        (
            TypeDatetime,
            LengthAndDecimal {
                length: 19,
                decimal: 0,
            },
        ),
        (
            TypeYear,
            LengthAndDecimal {
                length: 4,
                decimal: 0,
            },
        ),
        (
            TypeString,
            LengthAndDecimal {
                length: 1,
                decimal: 0,
            },
        ),
        (
            TypeVarchar,
            LengthAndDecimal {
                length: 5,
                decimal: 0,
            },
        ),
        (
            TypeVarString,
            LengthAndDecimal {
                length: 5,
                decimal: 0,
            },
        ),
        (
            TypeTinyBlob,
            LengthAndDecimal {
                length: 255,
                decimal: 0,
            },
        ),
        (
            TypeBlob,
            LengthAndDecimal {
                length: 65_535,
                decimal: 0,
            },
        ),
        (
            TypeMediumBlob,
            LengthAndDecimal {
                length: 16_777_215,
                decimal: 0,
            },
        ),
        (
            TypeLongBlob,
            LengthAndDecimal {
                length: 4_294_967_295,
                decimal: 0,
            },
        ),
        (
            TypeJSON,
            LengthAndDecimal {
                length: 4_294_967_295,
                decimal: 0,
            },
        ),
        (
            TypeNull,
            LengthAndDecimal {
                length: 0,
                decimal: 0,
            },
        ),
        (
            TypeSet,
            LengthAndDecimal {
                length: -1,
                decimal: 0,
            },
        ),
        (
            TypeEnum,
            LengthAndDecimal {
                length: -1,
                decimal: 0,
            },
        ),
    ])
});

// IsIntegerType 对应 Go switch，仅将五种整数协议类型判为 true。
/// IsIntegerType 判断协议类型是否为 TINYINT/SMALLINT/MEDIUMINT/INT/BIGINT 之一。
pub fn IsIntegerType(tp: u8) -> bool {
    matches!(
        tp,
        TypeTiny | TypeShort | TypeInt24 | TypeLong | TypeLonglong
    )
}

// GetDefaultFieldLengthAndDecimal 返回 DDL 或表达式推导使用的默认显示长度和小数位。
// 与 Go 一致：未登记的类型返回 (-1, -1)；i64 到 isize 的转换保留 Go int 的平台相关形状。
/// GetDefaultFieldLengthAndDecimal 查询 CREATE TABLE 默认 flen/decimal；未知类型返回 (-1, -1)。
pub fn GetDefaultFieldLengthAndDecimal(tp: u8) -> (isize, isize) {
    match defaultLengthAndDecimal.get(&tp) {
        Some(value) => (value.length as isize, value.decimal),
        None => (-1, -1),
    }
}

// defaultLengthAndDecimalForCast 对应 CAST 未指定长度时的另一张默认值表。
// String 与 JSON 的 flen 特意不同于 CREATE TABLE 表，不能合并这两组配置。
/// CAST 未指定长度时使用的默认 flen/decimal 表（与 DDL 表刻意不同）。
static defaultLengthAndDecimalForCast: LazyLock<HashMap<u8, LengthAndDecimal>> =
    LazyLock::new(|| {
        HashMap::from([
            (
                TypeString,
                LengthAndDecimal {
                    length: 0,
                    decimal: -1,
                },
            ),
            (
                TypeDate,
                LengthAndDecimal {
                    length: 10,
                    decimal: 0,
                },
            ),
            (
                TypeDatetime,
                LengthAndDecimal {
                    length: 19,
                    decimal: 0,
                },
            ),
            (
                TypeNewDecimal,
                LengthAndDecimal {
                    length: 10,
                    decimal: 0,
                },
            ),
            (
                TypeDuration,
                LengthAndDecimal {
                    length: 10,
                    decimal: 0,
                },
            ),
            (
                TypeLonglong,
                LengthAndDecimal {
                    length: 22,
                    decimal: 0,
                },
            ),
            (
                TypeDouble,
                LengthAndDecimal {
                    length: 22,
                    decimal: -1,
                },
            ),
            (
                TypeFloat,
                LengthAndDecimal {
                    length: 12,
                    decimal: -1,
                },
            ),
            (
                TypeJSON,
                LengthAndDecimal {
                    length: 4_194_304,
                    decimal: 0,
                },
            ),
        ])
    });

// GetDefaultFieldLengthAndDecimalForCast 查询 CAST 默认值；缺失类型同样返回 (-1, -1)。
/// GetDefaultFieldLengthAndDecimalForCast 查询 CAST 默认 flen/decimal；未知类型返回 (-1, -1)。
pub fn GetDefaultFieldLengthAndDecimalForCast(tp: u8) -> (isize, isize) {
    match defaultLengthAndDecimalForCast.get(&tp) {
        Some(value) => (value.length as isize, value.decimal),
        None => (-1, -1),
    }
}

// 认证插件名由同包其它迁移文件提供；这里保留 Go 的三项相等比较，不读取或处理真实密码。
/// IsAuthPluginClearText 判断插件是否在握手阶段使用明文口令交换路径。
pub fn IsAuthPluginClearText(authPlugin: &str) -> bool {
    authPlugin == AuthNativePassword
        || authPlugin == AuthTiDBSM3Password
        || authPlugin == AuthCachingSha2Password
}
