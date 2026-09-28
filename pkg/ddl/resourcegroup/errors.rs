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

// 资源组（Resource Group）校验与转换相关错误定义。
//
// 各变体与 Go 包中的哨兵错误（sentinel error）语义一一对应，
// 便于迁移期保持相同错误文本与分支判断。

use thiserror::Error;

/// Errors returned while validating and converting resource-group settings.
///
/// The variants preserve the identity semantics of the Go package's sentinel
/// errors while exposing normal Rust `Error`, equality, and copy behavior.
///
/// 校验/转换资源组设置时返回的错误集合。
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum ResourceGroupError {
    /// 组设置为空或无效。
    #[error("invalid group settings")]
    InvalidGroupSettings,
    /// 资源组名称超过最大长度限制。
    #[error("resource group name too long")]
    TooLongResourceGroupName,
    /// 组设置格式非法。
    #[error("group settings with invalid format")]
    InvalidResourceGroupFormat,
    /// 同时指定了 RU 模式与 Raw 模式选项。
    #[error("cannot set RU mode and Raw mode options at the same time")]
    InvalidResourceGroupDuplicatedMode,
    /// 未知的资源组模式（当前仅支持 RU 模式）。
    #[error("unknown resource group mode")]
    UnknownResourceGroupMode,
    /// 尝试删除系统保留的内部资源组。
    #[error("can't drop reserved resource group")]
    DroppingInternalResourceGroup,
    /// Runaway 规则未设置任何触发阈值字段。
    #[error("please set at least one field(exec_elapsed_time_ms, processed_keys, ru)")]
    ResourceGroupRunawayRuleIsEmpty,
    /// 未知的 Runaway 动作类型。
    #[error("unknown resource group runaway action")]
    UnknownResourceGroupRunawayAction,
    /// SwitchGroup 动作未指定目标组名。
    #[error("unknown resource group runaway switch group name")]
    UnknownResourceGroupRunawaySwitchGroupName,
}

/// 本模块错误类型别名，对齐 Go 侧 `Error` 命名。
pub type Error = ResourceGroupError;

/// 无效组设置（Go: `ErrInvalidGroupSettings`）。
pub const ErrInvalidGroupSettings: Error = Error::InvalidGroupSettings;
/// 名称过长（Go: `ErrTooLongResourceGroupName`）。
pub const ErrTooLongResourceGroupName: Error = Error::TooLongResourceGroupName;
/// 格式非法（Go: `ErrInvalidResourceGroupFormat`）。
pub const ErrInvalidResourceGroupFormat: Error = Error::InvalidResourceGroupFormat;
/// RU/Raw 模式冲突（Go: `ErrInvalidResourceGroupDuplicatedMode`）。
pub const ErrInvalidResourceGroupDuplicatedMode: Error = Error::InvalidResourceGroupDuplicatedMode;
/// 未知模式（Go: `ErrUnknownResourceGroupMode`）。
pub const ErrUnknownResourceGroupMode: Error = Error::UnknownResourceGroupMode;
/// 删除保留组（Go: `ErrDroppingInternalResourceGroup`）。
pub const ErrDroppingInternalResourceGroup: Error = Error::DroppingInternalResourceGroup;
/// Runaway 规则为空（Go: `ErrResourceGroupRunawayRuleIsEmpty`）。
pub const ErrResourceGroupRunawayRuleIsEmpty: Error = Error::ResourceGroupRunawayRuleIsEmpty;
/// 未知 Runaway 动作（Go: `ErrUnknownResourceGroupRunawayAction`）。
pub const ErrUnknownResourceGroupRunawayAction: Error = Error::UnknownResourceGroupRunawayAction;
/// 未知 SwitchGroup 名称（Go: `ErrUnknownResourceGroupRunawaySwitchGroupName`）。
pub const ErrUnknownResourceGroupRunawaySwitchGroupName: Error =
    Error::UnknownResourceGroupRunawaySwitchGroupName;
