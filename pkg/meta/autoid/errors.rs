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

// AutoID 错误类型与 AUTO_RANDOM 相关文案常量。
//
// 集中定义分配器、存储事务、RPC 与远程服务路径上的错误枚举，以及 DDL/校验
// 阶段使用的 `AUTO_RANDOM` 提示字符串（与 Go 版文案保持一致，含 `%s`/`%d` 占位）。

use thiserror::Error;

/// 本模块统一的 `Result` 别名，错误固定为 [`AutoIdError`]。
pub type Result<T> = std::result::Result<T, AutoIdError>;

/// AutoID 分配与服务调用过程中可能出现的错误。
#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum AutoIdError {
    /// 表 ID 非法（例如未绑定有效 table_id）。
    #[error("invalid table ID: {0}")]
    InvalidTableId(String),
    /// `auto_increment` 的 increment/offset 超出合法范围。
    #[error("invalid auto-increment increment and offset: {increment}, {offset}")]
    InvalidIncrementAndOffset { increment: i64, offset: i64 },
    /// 当前分配器类型不支持该操作。
    #[error("not implemented: {0}")]
    NotImplemented(String),
    /// 自增/序列 ID 空间耗尽或读全局水位失败。
    #[error("auto-increment read failed: {0}")]
    AutoIncrementReadFailed(String),
    /// 元数据中的 AutoID 键类型与预期不符。
    #[error("wrong auto key: {0}")]
    WrongAutoKey(String),
    /// 未知的分配器类型名。
    #[error("unknown allocator type: {0}")]
    InvalidAllocatorType(String),
    /// `AUTO_RANDOM` 读取失败。
    #[error("auto-random read failed: {0}")]
    AutoRandomReadFailed(String),
    /// `AUTO_RANDOM` 参数或约束不合法。
    #[error("invalid auto_random: {0}")]
    InvalidAutoRandom(String),
    /// 底层存储事务失败。
    #[error("storage error: {0}")]
    Storage(String),
    /// 上下文已取消（对应 Go context.Canceled）。
    #[error("context canceled")]
    Canceled,
    /// 远程 RPC 调用失败（可触发重连退避）。
    #[error("{0}")]
    Rpc(String),
    /// AutoID 服务端返回的业务错误消息。
    #[error("autoid service error: {0}")]
    Service(String),
}

/// 构造 increment/offset 非法错误。
pub fn invalid_increment_and_offset(increment: i64, offset: i64) -> AutoIdError {
    AutoIdError::InvalidIncrementAndOffset { increment, offset }
}

/// 构造自增/序列读失败错误。
pub fn autoinc_read_failed(message: impl Into<String>) -> AutoIdError {
    AutoIdError::AutoIncrementReadFailed(message.into())
}

/// AUTO_RANDOM 列必须是主键首列。
pub const AUTO_RANDOM_MUST_FIRST_COLUMN_IN_PK: &str =
    "column '%s' must be the first column in primary key";
/// AUTO_RANDOM 仅支持聚簇主键（clustered primary key）表。
pub const AUTO_RANDOM_NO_CLUSTERED_PK_ERR_MSG: &str =
    "auto_random is only supported on the tables with clustered primary key";
/// AUTO_RANDOM 与 AUTO_INCREMENT 互斥。
pub const AUTO_RANDOM_INCOMPATIBLE_WITH_AUTO_INC_ERR_MSG: &str =
    "auto_random is incompatible with auto_increment";
/// AUTO_RANDOM 列不能带 DEFAULT。
pub const AUTO_RANDOM_INCOMPATIBLE_WITH_DEFAULT_VALUE_ERR_MSG: &str =
    "auto_random is incompatible with default";
/// 分片位数（shard bits）超出上限。
pub const AUTO_RANDOM_OVERFLOW_ERR_MSG: &str =
    "max allowed auto_random shard bits is %d, but got %d on column `%s`";
/// 不允许修改 AUTO_RANDOM 列类型。
pub const AUTO_RANDOM_MODIFY_COL_TYPE_ERR_MSG: &str =
    "modifying the auto_random column type is not supported";
/// 不允许增删改 AUTO_RANDOM 属性本身。
pub const AUTO_RANDOM_ALTER_ERR_MSG: &str =
    "adding/dropping/modifying auto_random is not supported";
/// 不允许减小分片位数。
pub const AUTO_RANDOM_DECREASE_BIT_ERR_MSG: &str =
    "decreasing auto_random shard bits is not supported";
/// AUTO_RANDOM 取值必须为正。
pub const AUTO_RANDOM_NON_POSITIVE: &str = "the value of auto_random should be positive";
/// 隐式可分配次数提示。
pub const AUTO_RANDOM_AVAILABLE_ALLOC_TIMES_NOTE: &str = "Available implicit allocation times: %d";
/// 显式写入 AUTO_RANDOM 列被禁用时的提示。
pub const AUTO_RANDOM_EXPLICIT_INSERT_DISABLED_ERR_MSG: &str = "Explicit insertion on auto_random column is disabled. Try to set @@allow_auto_random_explicit_insert = true.";
/// AUTO_RANDOM 只能定义在 bigint 列上。
pub const AUTO_RANDOM_ON_NON_BIG_INT_COLUMN: &str =
    "auto_random option must be defined on `bigint` column, but not on `%s` column";
/// 对非 AUTO_RANDOM 表改 auto_random_base。
pub const AUTO_RANDOM_REBASE_NOT_APPLICABLE: &str =
    "alter auto_random_base of a non auto_random table";
/// rebase 超出增量位数容量。
pub const AUTO_RANDOM_REBASE_OVERFLOW: &str =
    "alter auto_random_base to %d overflows the incremental bits, max allowed base is %d";
/// ALTER 时不允许新增带 AUTO_RANDOM 的列。
pub const AUTO_RANDOM_ALTER_ADD_COLUMN: &str =
    "unsupported add column '%s' constraint AUTO_RANDOM when altering '%s.%s'";
/// 仅允许从聚簇主键的 AUTO_INCREMENT 转为 AUTO_RANDOM。
pub const AUTO_RANDOM_ALTER_CHANGE_FROM_AUTO_INC: &str =
    "auto_random can only be converted from auto_increment clustered primary key";
/// 表上找不到 AUTO_RANDOM 分配器。
pub const AUTO_RANDOM_ALLOCATOR_NOT_FOUND: &str =
    "auto_random ID allocator not found in table '%s.%s'";
/// range bits 不在合法区间。
pub const AUTO_RANDOM_INVALID_RANGE_BITS: &str =
    "auto_random range bits must be between %d and %d, but got %d";
/// 增量位数过小，可用 ID 空间不足。
pub const AUTO_RANDOM_INCREMENTAL_BITS_TOO_SMALL: &str =
    "auto_random ID space is too small, please decrease the shard bits or increase the range bits";
/// 不允许 ALTER range bits。
pub const AUTO_RANDOM_UNSUPPORTED_ALTER_RANGE_BITS: &str =
    "alter the range bits of auto_random column is not supported";
