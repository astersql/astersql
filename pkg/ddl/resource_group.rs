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
// 本文件前半部分保留与 Go 版对应的注释迁移草稿，后半部分是可独立运行的
// 目录（catalog）与选项解析实现。

/*
// DDL 创建、修改、删除 resource group 时的元信息更新和 infosync 调用形状。

pub const defaultInfosyncTimeout: time::Duration = time::Duration::from_secs(5);
pub const unlimitedRURate: u64 = i32::MAX as u64;

// FIXME: this is a workaround for the compatibility, format the error code.
// 对应 Go 常量，用于兼容 keyspace 模式下默认资源组已存在的返回文本。
pub const alreadyExists: &str = "already exists";

// onCreateResourceGroup 对应 Go 的创建 resource group DDL job 处理。
pub fn onCreateResourceGroup(jobCtx: &mut jobContext, job: &mut model::Job) -> Result<i64, errors::Error> {
    let mut ver: i64 = 0;
    let args = match model::GetResourceGroupArgs(job) {
        Ok(v) => v,
        Err(err) => {
            job.State = model::JobStateCancelled;
            return Err(errors::Trace(err));
        }
    };
    let mut groupInfo = args.RGInfo;
    groupInfo.State = model::StateNone;

    // check if resource group value is valid and convert to proto format.
    let protoGroup = match resourcegroup::NewGroupFromOptions(groupInfo.Name.L.clone(), groupInfo.ResourceGroupSettings.as_ref()) {
        Ok(v) => v,
        Err(err) => {
            logutil::DDLLogger().Warn("convert to resource group failed", zap::Error(err.clone()));
            job.State = model::JobStateCancelled;
            return Err(errors::Trace(err));
        }
    };

    match groupInfo.State {
        model::StateNone => {
            // none -> public
            groupInfo.State = model::StatePublic;
            if let Err(err) = jobCtx.metaMut.AddResourceGroup(&groupInfo) {
                return Err(errors::Trace(err));
            }

            // Go 使用 context.WithTimeout 并 defer cancel；不实际创建异步超时，只保留 infosync 语义。
            let ctx = context::WithTimeout(&jobCtx.stepCtx, defaultInfosyncTimeout);
            if let Err(err) = infosync::AddResourceGroup(ctx, &protoGroup) {
                logutil::DDLLogger().Warn(
                    "create resource group failed",
                    zap::String("group-name", groupInfo.Name.L.clone()),
                    zap::Error(err.clone()),
                );
                // TiDB will add the group to the resource manager when it bootstraps.
                // here order to compatible with keyspace mode TiDB to skip the exist error with default group.
                if !err.Error().contains(alreadyExists) || groupInfo.Name.L != rg::DefaultResourceGroupName {
                    return Err(errors::Trace(err));
                }
            }
            job.SchemaID = groupInfo.ID;
            ver = updateSchemaVersion(jobCtx, job).map_err(errors::Trace)?;
            // Finish this job.
            job.FinishDBJob(model::JobStateDone, model::StatePublic, ver, None);
            Ok(ver)
        }
        _ => Err(dbterror::ErrInvalidDDLState.GenWithStackByArgs("resource_group", groupInfo.State)),
    }
}

// onAlterResourceGroup 对应 Go 的修改 resource group DDL job 处理。
pub fn onAlterResourceGroup(jobCtx: &mut jobContext, job: &mut model::Job) -> Result<i64, errors::Error> {
    let mut ver: i64 = 0;
    let args = match model::GetResourceGroupArgs(job) {
        Ok(v) => v,
        Err(err) => {
            job.State = model::JobStateCancelled;
            return Err(errors::Trace(err));
        }
    };
    let alterGroupInfo = args.RGInfo;
    // check if resource group value is valid and convert to proto format.
    let protoGroup = match resourcegroup::NewGroupFromOptions(
        alterGroupInfo.Name.L.clone(),
        alterGroupInfo.ResourceGroupSettings.as_ref(),
    ) {
        Ok(v) => v,
        Err(err) => {
            logutil::DDLLogger().Warn("convert to resource group failed", zap::Error(err.clone()));
            job.State = model::JobStateCancelled;
            return Err(errors::Trace(err));
        }
    };

    let metaMut = &mut jobCtx.metaMut;
    let oldGroup = checkResourceGroupExist(metaMut, job, alterGroupInfo.ID)?;

    let mut newGroup = oldGroup.clone();
    newGroup.ResourceGroupSettings = alterGroupInfo.ResourceGroupSettings.clone();

    // TODO: check the group validation
    // 保留 Go 的 TODO：这里直接更新 meta，不额外做跨资源校验。
    metaMut.UpdateResourceGroup(&newGroup).map_err(errors::Trace)?;

    if let Err(err) = infosync::ModifyResourceGroup(context::TODO(), &protoGroup) {
        logutil::DDLLogger().Warn("update resource group failed", zap::Error(err.clone()));
        job.State = model::JobStateCancelled;
        return Err(errors::Trace(err));
    }

    ver = updateSchemaVersion(jobCtx, job).map_err(errors::Trace)?;
    // Finish this job.
    job.FinishDBJob(model::JobStateDone, model::StatePublic, ver, None);
    Ok(ver)
}

// checkResourceGroupExist 对应 Go 的存在性检查辅助函数。
// 当 infoschema 返回资源组不存在时，Go 会把 job 标记为 cancelled。
pub fn checkResourceGroupExist(
    t: &mut meta::Mutator,
    job: &mut model::Job,
    groupID: i64,
) -> Result<model::ResourceGroupInfo, errors::Error> {
    match t.GetResourceGroup(groupID) {
        Ok(groupInfo) => Ok(groupInfo),
        Err(err) => {
            if infoschema::ErrResourceGroupNotExists.Equal(&err) {
                job.State = model::JobStateCancelled;
            }
            Err(err)
        }
    }
}

// onDropResourceGroup 对应 Go 的删除 resource group DDL job 处理。
pub fn onDropResourceGroup(jobCtx: &mut jobContext, job: &mut model::Job) -> Result<i64, errors::Error> {
    let mut ver: i64 = 0;
    let metaMut = &mut jobCtx.metaMut;
    let mut groupInfo = checkResourceGroupExist(metaMut, job, job.SchemaID)?;
    // TODO: check the resource group not in use.
    // 保留 Go 的 TODO：没有实现“未被使用”校验。
    let mut err: Option<errors::Error> = None;
    match groupInfo.State {
        model::StatePublic => {
            // public -> none
            // resource group not influence the correctness of the data, so we can directly remove it.
            groupInfo.State = model::StateNone;
            metaMut.DropResourceGroup(groupInfo.ID).map_err(errors::Trace)?;
            infosync::DeleteResourceGroup(context::TODO(), groupInfo.Name.L.clone()).map_err(errors::Trace)?;
            ver = updateSchemaVersion(jobCtx, job).map_err(errors::Trace)?;
            // Finish this job.
            job.FinishDBJob(model::JobStateDone, model::StateNone, ver, None);
        }
        _ => {
            err = Some(dbterror::ErrInvalidDDLState.GenWithStackByArgs("resource_group", groupInfo.State));
        }
    }
    match err {
        Some(e) => Err(errors::Trace(e)),
        None => Ok(ver),
    }
}

// buildResourceGroup 对应 Go 的资源组信息构造函数。
// 它复制旧设置、逐个套用 AST option，最后调用 Adjust 归一化。
pub fn buildResourceGroup(
    oldGroup: &model::ResourceGroupInfo,
    options: &[ast::ResourceGroupOption],
) -> Result<model::ResourceGroupInfo, errors::Error> {
    let mut groupInfo = model::ResourceGroupInfo {
        Name: oldGroup.Name.clone(),
        ID: oldGroup.ID,
        ResourceGroupSettings: Some(model::NewResourceGroupSettings()),
        ..Default::default()
    };
    if let Some(old_settings) = oldGroup.ResourceGroupSettings.as_ref() {
        *groupInfo.ResourceGroupSettings.as_mut().unwrap() = old_settings.clone();
    }
    for opt in options {
        SetDirectResourceGroupSettings(&mut groupInfo, opt)?;
    }
    groupInfo.ResourceGroupSettings.as_mut().unwrap().Adjust();
    Ok(groupInfo)
}

// SetDirectResourceGroupSettings tries to set the ResourceGroupSettings.
pub fn SetDirectResourceGroupSettings(
    groupInfo: &mut model::ResourceGroupInfo,
    opt: &ast::ResourceGroupOption,
) -> Result<(), errors::Error> {
    let resourceGroupSettings = groupInfo.ResourceGroupSettings.as_mut().unwrap();
    match opt.Tp {
        ast::ResourceRURate => {
            if opt.Burstable == ast::BurstableUnlimited {
                resourceGroupSettings.RURate = unlimitedRURate;
            } else {
                resourceGroupSettings.RURate = opt.UintValue;
            }
        }
        ast::ResourcePriority => {
            resourceGroupSettings.Priority = opt.UintValue;
        }
        ast::ResourceUnitCPU => {
            resourceGroupSettings.CPULimiter = opt.StrValue.clone();
        }
        ast::ResourceUnitIOReadBandwidth => {
            resourceGroupSettings.IOReadBandwidth = opt.StrValue.clone();
        }
        ast::ResourceUnitIOWriteBandwidth => {
            resourceGroupSettings.IOWriteBandwidth = opt.StrValue.clone();
        }
        ast::ResourceBurstable => {
            // Some about BurstLimit(b):
            //   - If b > 0, that means the limiter is limited capacity. (current not used).
            //   - If b == 0, that means the limiter is unlimited capacity. default use in resource controller (burst with a rate within a unlimited capacity).
            //   - If b == -1, that means the limiter is unlimited capacity and fillrate(r) is ignored, can be seen as r == Inf (burst with a inf rate within a unlimited capacity).
            //   - If b == -2, that means the limiter is unlimited capacity and fillrate(r) is burstable, can be seen as r == n*fillrate (burst with a n times rate within a unlimited capacity).
            // Note: If RU_PER_SEC=unlimted, it means unlimited whatever BURSTABLE is.
            resourceGroupSettings.BurstLimit = match opt.Burstable {
                ast::BurstableUnlimited => -1,
                ast::BurstableModerated => -2,
                _ => 0, // ast.BurstableDisable
            };
        }
        ast::ResourceGroupRunaway => {
            if opt.RunawayOptionList.is_empty() {
                resourceGroupSettings.Runaway = None;
            } else {
                resourceGroupSettings.Runaway = Some(model::ResourceGroupRunawaySettings::default());
            }
            for runaway_opt in &opt.RunawayOptionList {
                SetDirectResourceGroupRunawayOption(resourceGroupSettings, runaway_opt)?;
            }
        }
        ast::ResourceGroupBackground => {
            if groupInfo.Name.L != rg::DefaultResourceGroupName {
                // FIXME: this is a temporary restriction, so we don't add a error-code for it.
                return Err(errors::New(
                    "unsupported operation. Currently, only the default resource group support change background settings",
                ));
            }
            if opt.BackgroundOptions.is_empty() {
                resourceGroupSettings.Background = None;
            }
            resourceGroupSettings.Background = Some(model::ResourceGroupBackgroundSettings::default());

            for background_opt in &opt.BackgroundOptions {
                SetDirectResourceGroupBackgroundOption(resourceGroupSettings, background_opt)?;
            }
        }
        _ => return Err(errors::Trace(errors::New("unknown resource unit type"))),
    }
    Ok(())
}

// SetDirectResourceGroupRunawayOption tries to set runaway part of the ResourceGroupSettings.
// Go 这里解析执行时长和 watch 时长；用 time::ParseDuration 作为占位映射。
pub fn SetDirectResourceGroupRunawayOption(
    resourceGroupSettings: &mut model::ResourceGroupSettings,
    opt: &ast::ResourceGroupRunawayOption,
) -> Result<(), errors::Error> {
    let settings = resourceGroupSettings.Runaway.as_mut().unwrap();
    match opt.Tp {
        ast::RunawayRule => match opt.RuleOption.Tp {
            ast::RunawayRuleExecElapsed => {
                // because execute time won't be too long, we use `time` pkg which does not support to parse unit 'd'.
                let dur = time::ParseDuration(&opt.RuleOption.ExecElapsed)?;
                settings.ExecElapsedTimeMs = dur.Milliseconds() as u64;
            }
            ast::RunawayRuleProcessedKeys => {
                settings.ProcessedKeys = opt.RuleOption.ProcessedKeys;
            }
            ast::RunawayRuleRequestUnit => {
                settings.RequestUnit = opt.RuleOption.RequestUnit;
            }
            _ => {}
        },
        ast::RunawayAction => {
            settings.Action = opt.ActionOption.Type;
            settings.SwitchGroupName = opt.ActionOption.SwitchGroupName.String();
        }
        ast::RunawayWatch => {
            settings.WatchType = opt.WatchOption.Type;
            let dur = opt.WatchOption.Duration.clone();
            if !dur.is_empty() {
                let parsed = time::ParseDuration(&dur)?;
                settings.WatchDurationMs = parsed.Milliseconds();
            } else {
                settings.WatchDurationMs = 0;
            }
        }
        _ => return Err(errors::Trace(errors::New("unknown runaway option type"))),
    }
    Ok(())
}

// SetDirectResourceGroupBackgroundOption set background configs of the ResourceGroupSettings.
pub fn SetDirectResourceGroupBackgroundOption(
    resourceGroupSettings: &mut model::ResourceGroupSettings,
    opt: &ast::ResourceGroupBackgroundOption,
) -> Result<(), errors::Error> {
    match opt.Type {
        ast::BackgroundOptionTaskNames => {
            let jobTypes = parseBackgroundJobTypes(&opt.StrValue)?;
            resourceGroupSettings.Background.as_mut().unwrap().JobTypes = jobTypes;
        }
        ast::BackgroundUtilizationLimit => {
            if opt.UintValue == 0 || opt.UintValue > 100 {
                return Err(errors::Trace(errors::New(
                    "invalid background resource utilization limit, the valid range is (0, 100]",
                )));
            }
            resourceGroupSettings.Background.as_mut().unwrap().ResourceUtilLimit = opt.UintValue;
        }
        _ => return Err(errors::Trace(errors::New("unknown background option type"))),
    }
    Ok(())
}

// parseBackgroundJobTypes 对应 Go 的逗号分隔任务类型解析。
// 它会 trim、转小写，并用 kvutil.ExplicitTypeList 做白名单校验。
pub fn parseBackgroundJobTypes(t: &str) -> Result<Vec<String>, errors::Error> {
    if t.is_empty() {
        return Ok(Vec::new());
    }

    let segs: Vec<&str> = t.split(',').collect();
    let mut res = Vec::with_capacity(segs.len());
    for s in segs {
        let ty = s.trim().to_lowercase();
        if !ty.is_empty() {
            if !kvutil::ExplicitTypeList.contains(&ty) {
                return Err(infoschema::ErrResourceGroupInvalidBackgroundTaskName.GenWithStackByArgs(ty));
            }
            res.push(ty);
        }
    }
    Ok(res)
}

// checkResourceGroupValidation 对应 Go 的校验封装函数。
pub fn checkResourceGroupValidation(groupInfo: &model::ResourceGroupInfo) -> Result<(), errors::Error> {
    resourcegroup::NewGroupFromOptions(groupInfo.Name.L.clone(), groupInfo.ResourceGroupSettings.as_ref())?;
    Ok(())
}
*/

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
