// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

// 单条查询的 runaway 检查器（Checker）。
//
// 在执行前后检查：
// 1. quarantine watch 列表是否命中（`BeforeExecutor`）；
// 2. 执行耗时 / RU / processed keys 是否超限（`BeforeCopRequest`、`CheckThresholds`）。
//
// 首次超限时通过 CAS 标记，写入 runaway 日志，并按配置加入 quarantine。
// Coprocessor 为下推到存储层的计算任务；RU（Request Unit）衡量资源消耗。
// watch 命中与规则超限共享记录链路，但最终动作始终以 watch 结果优先。

use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};

use crate::manager::Manager;
use crate::{
    CopRequest, Error, RUDetails, Result, RunawayAction, RunawaySettings, RunawayWatchType,
    Timestamp, nowMicros,
};

/// 中等读超时参考值（毫秒）；剩余截止时间小于此值时才收紧 Cop 超时。
const READ_TIMEOUT_MEDIUM_MILLIS: i64 = 60_000;

/// 绑定到一次查询的 runaway 检查状态。
pub struct Checker {
    /// 所属管理器（共享监视列表与记录队列）。
    pub manager: Manager,
    /// 当前资源组名。
    pub resource_group_name: String,
    /// 原始 SQL 文本（Exact watch 用）。
    pub original_sql: String,
    /// SQL digest（Similar watch 用）。
    pub sql_digest: String,
    /// 执行计划 digest（Plan watch 用）。
    pub plan_digest: String,
    /// 执行截止时间（微秒时间戳）；0 表示不启用。
    deadline: Timestamp,
    /// RU 阈值；0 表示不检查。
    ru_threshold: i64,
    /// processed keys 阈值；0 表示不检查。
    processed_keys_threshold: i64,
    /// 资源组 runaway 设置快照。
    settings: Option<RunawaySettings>,
    /// 由 watch 命中确定的动作。
    watch_action: RunawayAction,
    /// 累计已处理键数（并发安全）。
    total_processed_keys: AtomicI64,
    /// 是否已因规则超限被标记（CAS 保证只标记一次）。
    marked_by_identify: AtomicBool,
    /// 是否已因 watch 列表命中被标记。
    marked_by_query_watch_rule: bool,
}

impl Checker {
    /// 根据设置与起始时间构造检查器；耗时阈值会换算为绝对 deadline。
    pub fn NewChecker(
        manager: Manager,
        resource_group_name: String,
        settings: Option<RunawaySettings>,
        original_sql: String,
        sql_digest: String,
        plan_digest: String,
        start_time: Timestamp,
    ) -> Self {
        // 将毫秒耗时阈值转为微秒绝对截止时间；0 表示禁用。
        let (deadline, ru_threshold, processed_keys_threshold) =
            settings.as_ref().map_or((0, 0, 0), |settings| {
                let deadline = if settings.rule.exec_elapsed_time_ms == 0 {
                    0
                } else {
                    start_time
                        .saturating_add(settings.rule.exec_elapsed_time_ms.saturating_mul(1000))
                };
                (
                    deadline,
                    settings.rule.request_unit,
                    settings.rule.processed_keys,
                )
            });
        Self {
            manager,
            resource_group_name,
            original_sql,
            sql_digest,
            plan_digest,
            deadline,
            ru_threshold,
            processed_keys_threshold,
            settings,
            watch_action: RunawayAction::NoneAction,
            total_processed_keys: AtomicI64::new(0),
            marked_by_identify: AtomicBool::new(false),
            marked_by_query_watch_rule: false,
        }
    }

    /// 是否已因规则识别（identify）被标记为 runaway。
    pub fn isMarkedByIdentifyInRunawaySettings(&self) -> bool {
        self.marked_by_identify.load(Ordering::Acquire)
    }

    /// 执行前检查 watch 列表；Kill 返回 `Quarantined`，SwitchGroup 返回目标组名。
    pub fn BeforeExecutor(&mut self) -> Result<String> {
        // 依次用原始 SQL、SQL digest、plan digest 作为 convict 标识探测监视列表。
        for convict in [
            self.original_sql.clone(),
            self.sql_digest.clone(),
            self.plan_digest.clone(),
        ] {
            let (watched, mut action, mut switch_group, cause) = self
                .manager
                .examineWatchList(&self.resource_group_name, &convict);
            if !watched {
                continue;
            }
            // watch 记录未指定动作时，回退到资源组设置中的动作。
            if action == RunawayAction::NoneAction {
                if let Some(settings) = &self.settings {
                    action = settings.action;
                    switch_group.clone_from(&settings.switch_group_name);
                }
            }
            self.markRunawayByQueryWatchRule(action, switch_group.clone(), cause);
            return match action {
                RunawayAction::Kill => Err(Error::Quarantined),
                RunawayAction::SwitchGroup => Ok(self.checkSwitchGroupName(&switch_group)),
                RunawayAction::CoolDown | RunawayAction::DryRun => Ok(String::new()),
                RunawayAction::NoneAction => continue,
            };
        }
        Ok(String::new())
    }

    /// 校验目标资源组是否存在；不存在则返回空串，避免切到无效组。
    fn checkSwitchGroupName(&self, name: &str) -> String {
        if name.is_empty() {
            return String::new();
        }
        match self.manager.resourceGroup(name) {
            Ok(Some(_)) => name.to_owned(),
            _ => String::new(),
        }
    }

    /// Coprocessor 请求发出前：可能收紧超时、降优先级、改资源组，或直接 Kill。
    pub fn BeforeCopRequest(&self, request: &mut CopRequest) -> Result<()> {
        // 已由 watch 命中且动作为 CoolDown：压低优先级。
        if self.marked_by_query_watch_rule && self.watch_action == RunawayAction::CoolDown {
            request.override_priority = Some(1);
        }
        let Some(settings) = self.settings.clone() else {
            return Ok(());
        };
        let now = nowMicros();
        let cause = self.exceedsThresholds(now, None, 0);
        if !self.isMarkedByIdentifyInRunawaySettings() && cause.is_empty() {
            // 未超限但动作为 Kill：把剩余截止时间写入 Cop 超时，尽快在存储侧中止。
            if settings.action == RunawayAction::Kill && self.deadline != 0 {
                let until_ms = (self.deadline - now) / 1000;
                if until_ms > 0 && until_ms < READ_TIMEOUT_MEDIUM_MILLIS {
                    request.max_execution_duration_ms = until_ms as u64;
                }
            }
            return Ok(());
        }
        self.markRunawayByIdentifyInRunawaySettings(now, cause.clone());
        match settings.action {
            RunawayAction::Kill => Err(Error::QueryInterrupted(cause)),
            RunawayAction::CoolDown => {
                request.override_priority = Some(1);
                Ok(())
            }
            RunawayAction::SwitchGroup => {
                let group = self.checkSwitchGroupName(&settings.switch_group_name);
                if !group.is_empty() {
                    request.resource_group_name = group;
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }

    /// 返回当前生效动作：watch 优先于规则识别。
    pub fn CheckAction(&self) -> RunawayAction {
        if self.marked_by_query_watch_rule {
            self.watch_action
        } else if self.isMarkedByIdentifyInRunawaySettings() {
            self.settings
                .as_ref()
                .map_or(RunawayAction::NoneAction, |s| s.action)
        } else {
            RunawayAction::NoneAction
        }
    }

    /// 检查规则 Kill：若刚超限则标记并返回 (cause, should_kill)。
    pub fn CheckRuleKillAction(&self) -> (String, bool) {
        let Some(settings) = &self.settings else {
            return (String::new(), false);
        };
        if self.isMarkedByIdentifyInRunawaySettings() {
            return (String::new(), false);
        }
        let cause = self.exceedsThresholds(nowMicros(), None, 0);
        if cause.is_empty() {
            return (String::new(), false);
        }
        self.markRunawayByIdentifyInRunawaySettings(nowMicros(), cause.clone());
        (cause, settings.action == RunawayAction::Kill)
    }

    /// 在设置仍匹配当前资源组时，将本查询标识加入 quarantine。
    fn markQuarantine(&self, now: Timestamp, cause: String) {
        let Some(settings) = &self.settings else {
            return;
        };
        let Some(watch) = &settings.watch else { return };
        // 仅当目录中的 runaway 设置与构造时快照一致时才写入，避免配置变更后误隔离。
        let current_matches = self
            .manager
            .resourceGroup(&self.resource_group_name)
            .ok()
            .flatten()
            .and_then(|group| group.runaway_settings)
            .is_some_and(|current| current == *settings);
        if !current_matches {
            return;
        }
        self.manager.markQuarantine(
            self.resource_group_name.clone(),
            self.getSettingConvictIdentifier(),
            watch.kind,
            settings.action,
            settings.switch_group_name.clone(),
            watch.lasting_duration_ms.saturating_mul(1000),
            now,
            cause,
        );
    }

    /// CAS 标记规则超限；成功则写 runaway 日志，并在未命中 watch 时加入 quarantine。
    fn markRunawayByIdentifyInRunawaySettings(&self, now: Timestamp, cause: String) {
        if self
            .marked_by_identify
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            let Some(settings) = &self.settings else {
                return;
            };
            self.markRunaway(
                "identify",
                settings.action,
                &settings.switch_group_name,
                now,
                cause.clone(),
            );
            if !self.marked_by_query_watch_rule {
                self.markQuarantine(now, cause);
            }
        }
    }

    /// 标记由 watch 命中触发的 runaway。
    fn markRunawayByQueryWatchRule(
        &mut self,
        action: RunawayAction,
        switch_group: String,
        cause: String,
    ) {
        self.marked_by_query_watch_rule = true;
        self.watch_action = action;
        self.markRunaway("watch", action, &switch_group, nowMicros(), cause);
    }
    /// 向管理器入队一条 runaway 查询记录。
    fn markRunaway(
        &self,
        match_type: &str,
        action: RunawayAction,
        switch_group: &str,
        now: Timestamp,
        cause: String,
    ) {
        let action = if action == RunawayAction::SwitchGroup {
            format!("{action}({switch_group})")
        } else {
            action.to_string()
        }
        .to_lowercase();
        self.manager
            .markRunaway(self, action, match_type.into(), now, cause);
    }

    /// 按 watch 类型取出用于 quarantine 的 convict 标识。
    pub fn getSettingConvictIdentifier(&self) -> String {
        match self
            .settings
            .as_ref()
            .and_then(|s| s.watch.as_ref())
            .map(|w| w.kind)
        {
            Some(RunawayWatchType::Plan) => self.plan_digest.clone(),
            Some(RunawayWatchType::Similar) => self.sql_digest.clone(),
            Some(RunawayWatchType::Exact) => self.original_sql.clone(),
            _ => String::new(),
        }
    }

    /// 累加 processed keys 并检查阈值；Kill 时返回 `QueryInterrupted`。
    pub fn CheckThresholds(
        &self,
        ru: Option<&RUDetails>,
        process_keys: i64,
        original_error: Option<Error>,
    ) -> Option<Error> {
        let Some(settings) = &self.settings else {
            return original_error;
        };
        let now = nowMicros();
        // Coprocessor 因 deadline 被终止时，用当前时间参与耗时判定。
        let check_time = if original_error.as_ref().is_some_and(|error| {
            error
                .to_string()
                .starts_with("Coprocessor task terminated due to exceeding the deadline")
        }) {
            now
        } else {
            0
        };
        let processed = self
            .total_processed_keys
            .fetch_add(process_keys, Ordering::AcqRel)
            .saturating_add(process_keys);
        let cause = self.exceedsThresholds(check_time, ru, processed);
        if cause.is_empty() {
            return original_error;
        }
        self.markRunawayByIdentifyInRunawaySettings(now, cause.clone());
        if settings.action == RunawayAction::Kill {
            Some(Error::QueryInterrupted(cause))
        } else {
            original_error
        }
    }

    /// 按优先级检查耗时、RU、processed keys；返回超限原因字符串，未超限为空。
    pub fn exceedsThresholds(
        &self,
        now: Timestamp,
        ru: Option<&RUDetails>,
        processed_keys: i64,
    ) -> String {
        // 耗时优先于 RU 与 processed keys。
        if self.deadline != 0 && now != 0 && now >= self.deadline {
            return format!("ElapsedTime = {now}({})", self.deadline);
        }
        if let Some(ru) = ru {
            if self.ru_threshold != 0 && (ru.write_ru + ru.read_ru) as i64 >= self.ru_threshold {
                return format!(
                    "RequestUnit = {}({})",
                    ru.write_ru + ru.read_ru,
                    self.ru_threshold
                );
            }
        }
        if processed_keys != 0
            && self.processed_keys_threshold != 0
            && processed_keys >= self.processed_keys_threshold
        {
            return format!(
                "ProcessedKeys = {processed_keys}({})",
                self.processed_keys_threshold
            );
        }
        String::new()
    }
    /// 将累计 processed keys 清零（例如新一轮扫描开始）。
    pub fn ResetTotalProcessedKeys(&self) {
        self.total_processed_keys.store(0, Ordering::Release);
    }
}

impl Manager {
    /// 若资源组启用了 runaway 或已有活跃 watch，则为查询派生 Checker。
    pub fn DeriveChecker(
        &self,
        group: &str,
        original_sql: String,
        sql_digest: String,
        plan_digest: String,
        start_time: Timestamp,
    ) -> Option<Checker> {
        let resource_group = self.resourceGroup(group).ok().flatten()?;
        // 无 plan digest，或既无设置也无活跃 watch 时跳过。
        if plan_digest.is_empty()
            || (resource_group.runaway_settings.is_none() && self.getActiveWatchCount(group) == 0)
        {
            return None;
        }
        Some(Checker::NewChecker(
            self.clone(),
            group.into(),
            resource_group.runaway_settings,
            original_sql,
            sql_digest,
            plan_digest,
            start_time,
        ))
    }
}
