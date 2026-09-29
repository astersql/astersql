// Copyright 2026 AsterSQL.
// Copyright 2018 PingCAP, Inc.
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

// 会话/求值上下文中的行为标志位（flags）。
//
// 用于控制截断、溢出、除零等告警策略，以及当前语句类型（INSERT/SELECT 等）。
// 下半部分 `FLAG_*` 为与 Go 风格对齐的别名常量。

/// 忽略截断错误（不报错也不告警）。
pub const FlagIgnoreTruncate: u64 = 1;
/// 截断时记为 warning 而非 error。
pub const FlagTruncateAsWarning: u64 = 1 << 1;
/// CHAR 按完整长度填充（pad to full length）。
pub const FlagPadCharToFullLength: u64 = 1 << 2;
/// 当前处于 INSERT 语句上下文。
pub const FlagInInsertStmt: u64 = 1 << 3;
/// 当前处于 UPDATE 或 DELETE 语句上下文。
pub const FlagInUpdateOrDeleteStmt: u64 = 1 << 4;
/// 当前处于 SELECT 语句上下文。
pub const FlagInSelectStmt: u64 = 1 << 5;
/// 数值溢出记为 warning。
pub const FlagOverflowAsWarning: u64 = 1 << 6;
/// 忽略日期中的零值校验。
pub const FlagIgnoreZeroInDate: u64 = 1 << 7;
/// 除以零记为 warning。
pub const FlagDividedByZeroAsWarning: u64 = 1 << 8;
/// 当前处于集合运算（UNION/INTERSECT 等）语句上下文。
pub const FlagInSetOprStmt: u64 = 1 << 9;
/// 当前处于 LOAD DATA 语句上下文。
pub const FlagInLoadDataStmt: u64 = 1 << 10;
/// 当前处于受限 SQL（restricted SQL）上下文。
pub const FlagInRestrictedSQL: u64 = 1 << 11;
/// 在 TiKV 中启用短路表达式求值。
pub const FlagEnableTiKVShortCircuitExpression: u64 = 1 << 12;

/// `FlagIgnoreTruncate` 的 SCREAMING_SNAKE 别名。
pub const FLAG_IGNORE_TRUNCATE: u64 = FlagIgnoreTruncate;
/// `FlagTruncateAsWarning` 的 SCREAMING_SNAKE 别名。
pub const FLAG_TRUNCATE_AS_WARNING: u64 = FlagTruncateAsWarning;
/// `FlagPadCharToFullLength` 的 SCREAMING_SNAKE 别名。
pub const FLAG_PAD_CHAR_TO_FULL_LENGTH: u64 = FlagPadCharToFullLength;
/// `FlagInInsertStmt` 的 SCREAMING_SNAKE 别名。
pub const FLAG_IN_INSERT_STMT: u64 = FlagInInsertStmt;
/// `FlagInUpdateOrDeleteStmt` 的 SCREAMING_SNAKE 别名。
pub const FLAG_IN_UPDATE_OR_DELETE_STMT: u64 = FlagInUpdateOrDeleteStmt;
/// `FlagInSelectStmt` 的 SCREAMING_SNAKE 别名。
pub const FLAG_IN_SELECT_STMT: u64 = FlagInSelectStmt;
/// `FlagOverflowAsWarning` 的 SCREAMING_SNAKE 别名。
pub const FLAG_OVERFLOW_AS_WARNING: u64 = FlagOverflowAsWarning;
/// `FlagIgnoreZeroInDate` 的 SCREAMING_SNAKE 别名。
pub const FLAG_IGNORE_ZERO_IN_DATE: u64 = FlagIgnoreZeroInDate;
/// `FlagDividedByZeroAsWarning` 的 SCREAMING_SNAKE 别名。
pub const FLAG_DIVIDED_BY_ZERO_AS_WARNING: u64 = FlagDividedByZeroAsWarning;
/// `FlagInSetOprStmt` 的 SCREAMING_SNAKE 别名。
pub const FLAG_IN_SET_OPR_STMT: u64 = FlagInSetOprStmt;
/// `FlagInLoadDataStmt` 的 SCREAMING_SNAKE 别名。
pub const FLAG_IN_LOAD_DATA_STMT: u64 = FlagInLoadDataStmt;
/// `FlagInRestrictedSQL` 的 SCREAMING_SNAKE 别名。
pub const FLAG_IN_RESTRICTED_SQL: u64 = FlagInRestrictedSQL;
pub const FLAG_ENABLE_TIKV_SHORT_CIRCUIT_EXPRESSION: u64 = FlagEnableTiKVShortCircuitExpression;
