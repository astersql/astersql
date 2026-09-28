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

// Resource Group（资源组）DDL 逻辑：创建、修改、删除资源组及其配置校验。
//
// 资源组用于在集群内按 RU（Request Unit，统一资源计量单位）等维度做
// 工作负载隔离与限流；配置会同步到 Resource Manager（资源管理器）。

use std::collections::BTreeMap;

/// 默认资源组名称；仅该组允许修改 Background（后台任务）相关设置。
pub const DEFAULT_RESOURCE_GROUP_NAME: &str = "default";
/// 无限制 RU 速率的占位值（对应 `i32::MAX`），用于 `BURSTABLE UNLIMITED`。
pub const UNLIMITED_RU_RATE: u64 = i32::MAX as u64;

/// 资源组在元信息中的可见状态。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GroupState {
    /// 未生效 / 已删除中间态。
    None,
    /// 对外可见可用。
    Public,
}

/// Burst（突发）限流模式，决定令牌桶（token bucket）的容量与填充行为。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Burstable {
    /// 关闭突发：burst_limit = 0。
    Disabled,
    /// 无限制突发：burst_limit = -1，忽略 fill rate。
    Unlimited,
    /// 适度突发：burst_limit = -2，允许按 fill rate 的倍数突发。
    Moderated,
}

/// Runaway（失控查询）规则：超阈值时采取的动作与匹配方式。
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct RunawaySettings {
    pub exec_elapsed_ms: u64,
    pub processed_keys: u64,
    pub request_unit: u64,
    pub action: String,
    pub switch_group_name: String,
    pub watch_type: String,
    pub watch_duration_ms: i64,
}

/// 后台任务资源占用限制（仅 default 组可用）。
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct BackgroundSettings {
    pub job_types: Vec<String>,
    pub resource_util_limit: u64,
}

/// 资源组完整设置：RU 速率、优先级、IO/CPU 限流、突发、Runaway、Background。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResourceGroupSettings {
    pub ru_rate: u64,
    pub priority: u64,
    pub cpu_limiter: String,
    pub io_read_bandwidth: String,
    pub io_write_bandwidth: String,
    pub burst_limit: i64,
    pub runaway: Option<RunawaySettings>,
    pub background: Option<BackgroundSettings>,
}

impl Default for ResourceGroupSettings {
    fn default() -> Self {
        Self {
            ru_rate: 0,
            // 默认优先级 8，与 TiDB 默认资源组一致。
            priority: 8,
            cpu_limiter: String::new(),
            io_read_bandwidth: String::new(),
            io_write_bandwidth: String::new(),
            burst_limit: 0,
            runaway: None,
            background: None,
        }
    }
}

/// 资源组元信息：ID、名称、状态与设置。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResourceGroupInfo {
    pub id: i64,
    pub name: String,
    pub state: GroupState,
    pub settings: ResourceGroupSettings,
}

/// DDL/AST 层传入的单条资源组选项。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResourceGroupOption {
    RuRate { rate: u64, burstable: Burstable },
    Priority(u64),
    Cpu(String),
    IoRead(String),
    IoWrite(String),
    Burstable(Burstable),
    Runaway(Option<Vec<RunawayOption>>),
    Background(Option<Vec<BackgroundOption>>),
}

/// Runaway 子选项：规则阈值、动作或 Watch（相似查询标记）配置。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RunawayOption {
    ExecElapsed(String),
    ProcessedKeys(u64),
    RequestUnit(u64),
    Action {
        action: String,
        switch_group: String,
    },
    Watch {
        watch_type: String,
        duration: Option<String>,
    },
}

/// Background 子选项：任务类型列表或利用率上限。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BackgroundOption {
    TaskNames(String),
    UtilizationLimit(u64),
}

/// 资源组操作与校验错误。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResourceGroupError {
    AlreadyExists,
    NotFound,
    InvalidState,
    InvalidOption,
    InvalidDuration,
    UnsupportedBackground,
    InvalidTaskName(String),
    InvalidUtilization,
    Backend(String),
}

impl std::fmt::Display for ResourceGroupError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for ResourceGroupError {}

/// 与外部 Resource Manager 同步的抽象接口（infosync 侧）。
pub trait ResourceGroupManager {
    fn add(&mut self, group: &ResourceGroupInfo) -> Result<(), String>;
    fn modify(&mut self, group: &ResourceGroupInfo) -> Result<(), String>;
    fn delete(&mut self, name: &str) -> Result<(), String>;
}

/// 内存中的资源组目录，模拟 meta 存储与 schema version 推进。
#[derive(Default)]
pub struct ResourceGroupCatalog {
    groups: BTreeMap<i64, ResourceGroupInfo>,
    pub schema_version: u64,
}

impl ResourceGroupCatalog {
    /// Returns the persisted metadata for a resource group.
    pub fn get(&self, group_id: i64) -> Option<&ResourceGroupInfo> {
        self.groups.get(&group_id)
    }

    /// 创建资源组：校验 → 写入目录 → 同步 Manager → 递增 schema version。
    pub fn create(
        &mut self,
        mut group: ResourceGroupInfo,
        manager: &mut dyn ResourceGroupManager,
    ) -> Result<u64, ResourceGroupError> {
        // ID 或名称（大小写不敏感）冲突则拒绝。
        if self.groups.contains_key(&group.id)
            || self
                .groups
                .values()
                .any(|old| old.name.eq_ignore_ascii_case(&group.name))
        {
            return Err(ResourceGroupError::AlreadyExists);
        }
        check_resource_group_validation(&group)?;
        group.state = GroupState::Public;
        self.groups.insert(group.id, group.clone());
        // 兼容 keyspace 模式：default 组在 Manager 侧已存在时跳过 already exists。
        if let Err(error) = manager.add(&group) {
            if !(group.name.eq_ignore_ascii_case(DEFAULT_RESOURCE_GROUP_NAME)
                && error.contains("already exists"))
            {
                return Err(ResourceGroupError::Backend(error));
            }
        }
        self.schema_version = self.schema_version.saturating_add(1);
        Ok(self.schema_version)
    }

    /// 修改已有资源组的设置并同步到 Manager。
    pub fn alter(
        &mut self,
        group_id: i64,
        settings: ResourceGroupSettings,
        manager: &mut dyn ResourceGroupManager,
    ) -> Result<u64, ResourceGroupError> {
        let old = self
            .groups
            .get(&group_id)
            .ok_or(ResourceGroupError::NotFound)?;
        let mut new_group = old.clone();
        new_group.settings = settings;
        check_resource_group_validation(&new_group)?;
        // Go updates meta before infosync and does not roll it back if infosync fails.
        self.groups.insert(group_id, new_group.clone());
        manager
            .modify(&new_group)
            .map_err(ResourceGroupError::Backend)?;
        self.schema_version = self.schema_version.saturating_add(1);
        Ok(self.schema_version)
    }

    /// 删除 Public 状态的资源组；先通知 Manager，再从目录移除。
    pub fn drop_group(
        &mut self,
        group_id: i64,
        manager: &mut dyn ResourceGroupManager,
    ) -> Result<u64, ResourceGroupError> {
        let group = self
            .groups
            .get(&group_id)
            .ok_or(ResourceGroupError::NotFound)?;
        if group.state != GroupState::Public {
            return Err(ResourceGroupError::InvalidState);
        }
        let name = group.name.clone();
        // Go drops meta before infosync and does not restore it if infosync fails.
        self.groups.remove(&group_id);
        manager.delete(&name).map_err(ResourceGroupError::Backend)?;
        self.schema_version = self.schema_version.saturating_add(1);
        Ok(self.schema_version)
    }
}

/// 在旧组基础上套用 AST 选项列表，生成新的资源组信息。
pub fn build_resource_group(
    old: &ResourceGroupInfo,
    options: &[ResourceGroupOption],
) -> Result<ResourceGroupInfo, ResourceGroupError> {
    // Go constructs fresh metadata from name, ID, and a copied settings value;
    // the schema state is intentionally left at its zero value (None).
    let mut group = ResourceGroupInfo {
        id: old.id,
        name: old.name.clone(),
        state: GroupState::None,
        settings: old.settings.clone(),
    };
    for option in options {
        set_direct_resource_group_settings(&mut group, option)?;
    }
    // ResourceGroupSettings.Adjust in Go maps a finite RU rate to bucket capacity.
    if group.settings.ru_rate != UNLIMITED_RU_RATE && group.settings.burst_limit >= 0 {
        group.settings.burst_limit = group.settings.ru_rate as i64;
    }
    Ok(group)
}

/// 将单条 `ResourceGroupOption` 直接写入组设置。
pub fn set_direct_resource_group_settings(
    group: &mut ResourceGroupInfo,
    option: &ResourceGroupOption,
) -> Result<(), ResourceGroupError> {
    match option {
        ResourceGroupOption::RuRate { rate, burstable } => {
            // Unlimited burst 时 RU 速率记为占位最大值。
            group.settings.ru_rate = if *burstable == Burstable::Unlimited {
                UNLIMITED_RU_RATE
            } else {
                *rate
            }
        }
        ResourceGroupOption::Priority(value) => group.settings.priority = *value,
        ResourceGroupOption::Cpu(value) => group.settings.cpu_limiter = value.clone(),
        ResourceGroupOption::IoRead(value) => group.settings.io_read_bandwidth = value.clone(),
        ResourceGroupOption::IoWrite(value) => group.settings.io_write_bandwidth = value.clone(),
        ResourceGroupOption::Burstable(value) => {
            // BurstLimit 语义：0 关闭 / -1 无限 / -2 适度突发。
            group.settings.burst_limit = match value {
                Burstable::Disabled => 0,
                Burstable::Unlimited => -1,
                Burstable::Moderated => -2,
            }
        }
        ResourceGroupOption::Runaway(options) => {
            group.settings.runaway = options.as_ref().map(|_| RunawaySettings::default());
            for option in options.as_deref().unwrap_or_default() {
                set_direct_runaway_option(&mut group.settings, option)?;
            }
        }
        ResourceGroupOption::Background(options) => {
            // 临时限制：仅 default 组可改后台设置。
            if !group.name.eq_ignore_ascii_case(DEFAULT_RESOURCE_GROUP_NAME) {
                return Err(ResourceGroupError::UnsupportedBackground);
            }
            // The Go implementation assigns a fresh settings object even when
            // the option list is empty (after a redundant nil assignment).
            group.settings.background = Some(BackgroundSettings::default());
            for option in options.as_deref().unwrap_or_default() {
                set_direct_background_option(&mut group.settings, option)?;
            }
        }
    }
    Ok(())
}

/// 解析并写入 Runaway 子选项。
pub fn set_direct_runaway_option(
    settings: &mut ResourceGroupSettings,
    option: &RunawayOption,
) -> Result<(), ResourceGroupError> {
    let runaway = settings
        .runaway
        .get_or_insert_with(RunawaySettings::default);
    match option {
        RunawayOption::ExecElapsed(value) => {
            // Go converts the signed Milliseconds result directly to uint64.
            runaway.exec_elapsed_ms = parse_duration_ms(value)? as u64
        }
        RunawayOption::ProcessedKeys(value) => runaway.processed_keys = *value,
        RunawayOption::RequestUnit(value) => runaway.request_unit = *value,
        RunawayOption::Action {
            action,
            switch_group,
        } => {
            runaway.action = action.clone();
            runaway.switch_group_name = switch_group.clone();
        }
        RunawayOption::Watch {
            watch_type,
            duration,
        } => {
            runaway.watch_type = watch_type.clone();
            runaway.watch_duration_ms = duration
                .as_deref()
                .map(parse_duration_ms)
                .transpose()?
                .unwrap_or(0);
        }
    }
    Ok(())
}

/// 解析并写入 Background 子选项。
pub fn set_direct_background_option(
    settings: &mut ResourceGroupSettings,
    option: &BackgroundOption,
) -> Result<(), ResourceGroupError> {
    let background = settings
        .background
        .get_or_insert_with(BackgroundSettings::default);
    match option {
        BackgroundOption::TaskNames(value) => {
            background.job_types = parse_background_job_types(value)?
        }
        BackgroundOption::UtilizationLimit(value) if (1..=100).contains(value) => {
            background.resource_util_limit = *value
        }
        BackgroundOption::UtilizationLimit(_) => {
            return Err(ResourceGroupError::InvalidUtilization);
        }
    }
    Ok(())
}

/// 解析逗号分隔的后台任务类型，并做白名单校验。
pub fn parse_background_job_types(value: &str) -> Result<Vec<String>, ResourceGroupError> {
    const ALLOWED: &[&str] = &[
        "lightning",
        "br",
        "dumpling",
        "background",
        "ddl",
        "stats",
        "import",
    ];
    value
        .split(',')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(|part| {
            let name = part.to_ascii_lowercase();
            if ALLOWED.contains(&name.as_str()) {
                Ok(name)
            } else {
                Err(ResourceGroupError::InvalidTaskName(name))
            }
        })
        .collect()
}

/// Parses Go `time.ParseDuration`-style hour/minute/second units to milliseconds.
fn parse_duration_ms(value: &str) -> Result<i64, ResourceGroupError> {
    let normalized = value.replace(['µ', 'μ'], "u");
    let (negative, text) = if let Some(rest) = normalized.strip_prefix('-') {
        (true, rest)
    } else {
        (false, normalized.strip_prefix('+').unwrap_or(&normalized))
    };
    if text.is_empty() {
        return Err(ResourceGroupError::InvalidDuration);
    }
    if text == "0" {
        return Ok(0);
    }

    let mut rest = text;
    let mut total_ms = 0.0_f64;
    while !rest.is_empty() {
        let number_end = rest
            .find(|c: char| !c.is_ascii_digit() && c != '.')
            .ok_or(ResourceGroupError::InvalidDuration)?;
        if number_end == 0 {
            return Err(ResourceGroupError::InvalidDuration);
        }
        let amount: f64 = rest[..number_end]
            .parse()
            .map_err(|_| ResourceGroupError::InvalidDuration)?;
        rest = &rest[number_end..];
        let unit_end = rest
            .find(|c: char| c.is_ascii_digit() || c == '.')
            .unwrap_or(rest.len());
        let factor_ms = match &rest[..unit_end] {
            "ns" => 0.000_001,
            "us" => 0.001,
            "ms" => 1.0,
            "s" => 1_000.0,
            "m" => 60_000.0,
            "h" => 3_600_000.0,
            _ => return Err(ResourceGroupError::InvalidDuration),
        };
        total_ms += amount * factor_ms;
        rest = &rest[unit_end..];
    }
    if negative {
        total_ms = -total_ms;
    }
    if !total_ms.is_finite() || total_ms < i64::MIN as f64 || total_ms > i64::MAX as f64 {
        return Err(ResourceGroupError::InvalidDuration);
    }
    Ok(total_ms.trunc() as i64)
}

/// 校验资源组名称、优先级、RU/突发组合及后台利用率上限。
pub fn check_resource_group_validation(
    group: &ResourceGroupInfo,
) -> Result<(), ResourceGroupError> {
    // ru_rate==0 且非无限突发（burst_limit!=-1）视为无效配置。
    if group.name.trim().is_empty()
        || group.settings.priority > 16
        || group.settings.ru_rate == 0 && group.settings.burst_limit != -1
    {
        return Err(ResourceGroupError::InvalidOption);
    }
    if let Some(background) = &group.settings.background {
        if background.resource_util_limit > 100 {
            return Err(ResourceGroupError::InvalidUtilization);
        }
    }
    Ok(())
}
