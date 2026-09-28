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

// 将 TiDB 模型层资源组设置转换为 Resource Manager protobuf 结构。
//
// 负责名称长度校验、Runaway（失控查询）/Background 配置填充，
// 以及 RU（Request Unit）模式下令牌桶（token bucket）参数组装。
// 当前与 Go 实现一致：仅支持 RU 模式，不允许同时配置 Raw 限流选项。

use crate::errors::{
    ErrInvalidGroupSettings, ErrInvalidResourceGroupDuplicatedMode,
    ErrResourceGroupRunawayRuleIsEmpty, ErrTooLongResourceGroupName, ErrUnknownResourceGroupMode,
    ErrUnknownResourceGroupRunawayAction, ErrUnknownResourceGroupRunawaySwitchGroupName, Error,
};
use crate::{ast, model, rmpb};

/// Maximum byte length of a resource-group name, matching Go's `len(string)`.
/// 资源组名称最大字节长度，与 Go `len(string)` 语义一致。
pub const MAX_GROUP_NAME_LENGTH: usize = 32;
/// Go 风格别名，等同于 [`MAX_GROUP_NAME_LENGTH`]。
pub const MaxGroupNameLength: usize = MAX_GROUP_NAME_LENGTH;

/// Converts TiDB model settings into the resource-manager protobuf shape.
/// 将模型层 `ResourceGroupSettings` 转为 `rmpb::ResourceGroup`。
pub fn NewGroupFromOptions(
    groupName: String,
    options: Option<&model::ResourceGroupSettings>,
) -> Result<rmpb::ResourceGroup, Error> {
    let options = options.ok_or(ErrInvalidGroupSettings)?;
    if groupName.len() > MAX_GROUP_NAME_LENGTH {
        return Err(ErrTooLongResourceGroupName);
    }

    let mut group = rmpb::ResourceGroup::new();
    group.set_name(groupName);
    group.set_priority(options.Priority as u32);

    // 填充 Runaway 规则：至少需要一个阈值字段，且动作类型有效。
    if let Some(runaway_options) = options.Runaway.as_ref() {
        if runaway_options.ExecElapsedTimeMs == 0
            && runaway_options.ProcessedKeys == 0
            && runaway_options.RequestUnit == 0
        {
            return Err(ErrResourceGroupRunawayRuleIsEmpty);
        }

        let mut rule = rmpb::RunawayRule::new();
        rule.set_exec_elapsed_time_ms(runaway_options.ExecElapsedTimeMs);
        rule.set_processed_keys(runaway_options.ProcessedKeys);
        rule.set_request_unit(runaway_options.RequestUnit);

        if runaway_options.Action == ast::RunawayActionNone {
            return Err(ErrUnknownResourceGroupRunawayAction);
        }
        if runaway_options.Action == ast::RunawayActionSwitchGroup
            && runaway_options.SwitchGroupName.is_empty()
        {
            return Err(ErrUnknownResourceGroupRunawaySwitchGroupName);
        }

        let mut runaway = rmpb::RunawaySettings::new();
        runaway.set_rule(rule);
        runaway.set_action(runaway_action(runaway_options.Action));
        runaway.set_switch_group_name(runaway_options.SwitchGroupName.clone());

        // Watch 用于标记相似查询，在 lasting_duration 内复用同一动作。
        if runaway_options.WatchType != ast::WatchNone {
            let mut watch = rmpb::RunawayWatch::new();
            watch.set_type(runaway_watch_type(runaway_options.WatchType));
            watch.set_lasting_duration_ms(runaway_options.WatchDurationMs);
            runaway.set_watch(watch);
        }
        group.set_runaway_settings(runaway);
    }

    if let Some(background_options) = options.Background.as_ref() {
        let mut background = rmpb::BackgroundSettings::new();
        background.set_job_types(protobuf::RepeatedField::from_vec(
            background_options.JobTypes.clone(),
        ));
        background.set_utilization_limit(background_options.ResourceUtilLimit);
        group.set_background_settings(background);
    }

    // RU 模式：按 fill_rate / burst_limit 组装令牌桶。
    if options.RURate > 0 {
        group.set_mode(rmpb::GroupMode::RuMode);

        let mut limit = rmpb::TokenLimitSettings::new();
        limit.set_fill_rate(options.RURate);
        limit.set_burst_limit(options.BurstLimit);

        let mut bucket = rmpb::TokenBucket::new();
        bucket.set_settings(limit);

        let mut ru_settings = rmpb::GroupRequestUnitSettings::new();
        ru_settings.set_r_u(bucket);
        group.set_r_u_settings(ru_settings);

        // 已选 RU 模式时禁止再带 Raw 模式的 CPU/IO 限流选项。
        if !options.CPULimiter.is_empty()
            || !options.IOReadBandwidth.is_empty()
            || !options.IOWriteBandwidth.is_empty()
        {
            return Err(ErrInvalidResourceGroupDuplicatedMode);
        }
        return Ok(group);
    }

    // Only RU mode is supported, matching the current Go implementation.
    // 当前仅支持 RU 模式，与 Go 实现保持一致。
    Err(ErrUnknownResourceGroupMode)
}

/// 将 AST Runaway 动作枚举映射为 protobuf 动作。
fn runaway_action(action: ast::RunawayActionType) -> rmpb::RunawayAction {
    match action {
        ast::RunawayActionNone => rmpb::RunawayAction::NoneAction,
        ast::RunawayActionDryRun => rmpb::RunawayAction::DryRun,
        ast::RunawayActionCooldown => rmpb::RunawayAction::CoolDown,
        ast::RunawayActionKill => rmpb::RunawayAction::Kill,
        ast::RunawayActionSwitchGroup => rmpb::RunawayAction::SwitchGroup,
        _ => rmpb::RunawayAction::NoneAction,
    }
}

/// 将 AST Watch 类型枚举映射为 protobuf Watch 类型。
fn runaway_watch_type(watch_type: ast::RunawayWatchType) -> rmpb::RunawayWatchType {
    match watch_type {
        ast::WatchNone => rmpb::RunawayWatchType::NoneWatch,
        ast::WatchExact => rmpb::RunawayWatchType::Exact,
        ast::WatchSimilar => rmpb::RunawayWatchType::Similar,
        ast::WatchPlan => rmpb::RunawayWatchType::Plan,
        _ => rmpb::RunawayWatchType::NoneWatch,
    }
}
