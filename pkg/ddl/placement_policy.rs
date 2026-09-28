// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
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

// Placement Policy（放置策略）目录与 DDL 辅助逻辑。
//
// Placement Policy 描述表/库/分区数据在集群中的副本分布策略（主区域、副本数、label 约束等）。

use std::collections::{BTreeMap, BTreeSet};

/// 放置策略在 DDL 状态机中的可见性状态。
///
/// 删除路径通常为 Public → WriteOnly → DeleteOnly → None（与 schema 对象软删除类似）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PolicyState {
    /// 未公开（新建初始态）。
    None,
    /// 对外可见且可被引用。
    Public,
    /// 删除中间态：禁止新写入依赖。
    WriteOnly,
    /// 删除中间态：仅保留删除路径可见性。
    DeleteOnly,
}

/// 放置策略的具体配置项（区域、副本角色计数、约束字符串等）。
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct PlacementSettings {
    /// 主区域（Primary Region）名称。
    pub primary_region: String,
    /// 参与调度的区域列表（逗号分隔）。
    pub regions: String,
    /// Follower 副本数。
    pub followers: u64,
    /// Voter 副本数。
    pub voters: u64,
    /// Learner 副本数（学习副本，不参与投票）。
    pub learners: u64,
    /// 调度策略名，如 EVEN / MAJORITY_IN_PRIMARY。
    pub schedule: String,
    /// 通用 label 约束字符串。
    pub constraints: String,
    /// Leader 约束。
    pub leader_constraints: String,
    /// Learner 约束。
    pub learner_constraints: String,
    /// Follower 约束。
    pub follower_constraints: String,
    /// Voter 约束。
    pub voter_constraints: String,
    /// 存活偏好（跨故障域容忍配置）。
    pub survival_preferences: String,
}

/// 一条已登记的放置策略元信息。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PolicyInfo {
    pub id: i64,
    pub name: String,
    pub state: PolicyState,
    pub settings: PlacementSettings,
}

/// 对某条放置策略的引用（按 ID + 名称）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PolicyRef {
    pub id: i64,
    pub name: String,
}

/// 可绑定放置策略的对象（库/表/分区的简化模型）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlacementObject {
    pub id: i64,
    pub policy_ref: Option<PolicyRef>,
    /// 分区子对象；表级对象可嵌套分区放置引用。
    pub partitions: Vec<PlacementObject>,
}

/// DDL/AST 侧放置选项类型，用于写入 `PlacementSettings` 对应字段。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlacementOptionType {
    PrimaryRegion,
    Regions,
    FollowerCount,
    VoterCount,
    LearnerCount,
    Schedule,
    Constraints,
    LeaderConstraints,
    LearnerConstraints,
    FollowerConstraints,
    VoterConstraints,
    SurvivalPreferences,
}

/// 放置策略操作错误。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PolicyError {
    AlreadyExists,
    NotFound,
    InvalidState,
    InvalidOption,
    InvalidSettings,
    InUse,
    RangeBackend(String),
}

impl std::fmt::Display for PolicyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for PolicyError {}

/// 内存中的放置策略目录：按 ID/名称索引，并记录库表引用与 range 规则占用。
#[derive(Default)]
pub struct PlacementPolicyCatalog {
    policies: BTreeMap<i64, PolicyInfo>,
    by_name: BTreeMap<String, i64>,
    /// Schema 版本号；每次成功变更递增。
    pub schema_version: u64,
    /// 数据库级放置对象列表。
    pub databases: Vec<PlacementObject>,
    /// 表级放置对象列表（可含分区）。
    pub tables: Vec<PlacementObject>,
    /// range 规则 ID → 策略名；用于检测系统 range 是否占用策略。
    pub range_policy_names: BTreeMap<String, String>,
}

impl PlacementPolicyCatalog {
    /// 创建策略；若同名已存在且 `replace_on_exist` 为真则仅替换 settings。
    pub fn create(
        &mut self,
        mut policy: PolicyInfo,
        replace_on_exist: bool,
    ) -> Result<u64, PolicyError> {
        // Go onCreatePlacementPolicy discards the caller-provided state before validation.
        policy.state = PolicyState::None;
        check_policy_validation(&policy.settings)?;
        let key = policy.name.to_ascii_lowercase();
        // OR REPLACE：复用原 ID，只更新 settings。
        if let Some(existing_id) = self.by_name.get(&key).copied() {
            if !replace_on_exist {
                return Err(PolicyError::AlreadyExists);
            }
            let old = self
                .policies
                .get_mut(&existing_id)
                .ok_or(PolicyError::NotFound)?;
            old.settings = policy.settings;
            self.schema_version = self.schema_version.saturating_add(1);
            return Ok(self.schema_version);
        }
        // 新建：None → Public 后写入目录。
        policy.state = PolicyState::Public;
        self.by_name.insert(key, policy.id);
        self.policies.insert(policy.id, policy);
        self.schema_version = self.schema_version.saturating_add(1);
        Ok(self.schema_version)
    }

    /// 修改已存在且处于 Public 状态的策略配置。
    pub fn alter(
        &mut self,
        policy_id: i64,
        settings: PlacementSettings,
    ) -> Result<u64, PolicyError> {
        // Go resolves the old policy before validating the replacement settings.
        if !self.policies.contains_key(&policy_id) {
            return Err(PolicyError::NotFound);
        }
        check_policy_validation(&settings)?;
        let policy = self
            .policies
            .get_mut(&policy_id)
            .ok_or(PolicyError::NotFound)?;
        if policy.state != PolicyState::Public {
            return Err(PolicyError::InvalidState);
        }
        policy.settings = settings;
        self.schema_version = self.schema_version.saturating_add(1);
        Ok(self.schema_version)
    }

    /// 推进删除状态机一步；到达 None 时从目录移除策略。
    pub fn drop_step(&mut self, policy_id: i64) -> Result<PolicyState, PolicyError> {
        self.check_not_in_use(policy_id)?;
        let policy = self
            .policies
            .get_mut(&policy_id)
            .ok_or(PolicyError::NotFound)?;
        let next = match policy.state {
            PolicyState::Public => PolicyState::WriteOnly,
            PolicyState::WriteOnly => PolicyState::DeleteOnly,
            PolicyState::DeleteOnly => PolicyState::None,
            PolicyState::None => return Err(PolicyError::InvalidState),
        };
        policy.state = next;
        // 最终态：清理 ID 与名称索引。
        if next == PolicyState::None {
            let policy = self.policies.remove(&policy_id).unwrap();
            self.by_name.remove(&policy.name.to_ascii_lowercase());
        }
        self.schema_version = self.schema_version.saturating_add(1);
        Ok(next)
    }

    /// 按名称（大小写不敏感）查找策略。
    pub fn by_name(&self, name: &str) -> Option<&PolicyInfo> {
        self.by_name
            .get(&name.to_ascii_lowercase())
            .and_then(|id| self.policies.get(id))
    }

    /// 将引用归一化为真实策略 ID；`default` 视为清除引用。
    pub fn normalize_ref(
        &self,
        reference: Option<PolicyRef>,
    ) -> Result<Option<PolicyRef>, PolicyError> {
        let Some(mut reference) = reference else {
            return Ok(None);
        };
        if reference.name.eq_ignore_ascii_case("default") {
            return Ok(None);
        }
        let policy = self.by_name(&reference.name).ok_or(PolicyError::NotFound)?;
        reference.id = policy.id;
        Ok(Some(reference))
    }

    /// 检查策略是否仍被库/表/分区或 range 规则引用。
    pub fn check_not_in_use(&self, policy_id: i64) -> Result<(), PolicyError> {
        let policy = self.policies.get(&policy_id).ok_or(PolicyError::NotFound)?;
        if self
            .databases
            .iter()
            .chain(&self.tables)
            .any(|object| object_uses_policy(object, policy_id))
        {
            return Err(PolicyError::InUse);
        }
        // 系统 key range 上的放置规则也可能引用该策略名。
        if self
            .range_policy_names
            .values()
            .any(|name| name.eq_ignore_ascii_case(&policy.name))
        {
            return Err(PolicyError::InUse);
        }
        Ok(())
    }

    /// 收集依赖该策略的库、分区、表 ID 列表（返回顺序：db、partition、table）。
    pub fn depended_object_ids(
        &self,
        policy_id: i64,
    ) -> Result<(Vec<i64>, Vec<i64>, Vec<i64>), PolicyError> {
        if !self.policies.contains_key(&policy_id) {
            return Err(PolicyError::NotFound);
        }
        let db_ids = self
            .databases
            .iter()
            .filter(|db| ref_matches(&db.policy_ref, policy_id))
            .map(|db| db.id)
            .collect();
        let table_ids = self
            .tables
            .iter()
            .filter(|table| ref_matches(&table.policy_ref, policy_id))
            .map(|table| table.id)
            .collect();
        let partition_ids = self
            .tables
            .iter()
            .flat_map(|table| &table.partitions)
            .filter(|part| ref_matches(&part.policy_ref, policy_id))
            .map(|part| part.id)
            .collect();
        Ok((db_ids, partition_ids, table_ids))
    }
}

/// 判断可选引用是否指向指定策略 ID。
fn ref_matches(reference: &Option<PolicyRef>, policy_id: i64) -> bool {
    reference
        .as_ref()
        .is_some_and(|reference| reference.id == policy_id)
}

/// 对象自身或其分区是否使用指定策略。
fn object_uses_policy(object: &PlacementObject, policy_id: i64) -> bool {
    ref_matches(&object.policy_ref, policy_id)
        || object
            .partitions
            .iter()
            .any(|part| ref_matches(&part.policy_ref, policy_id))
}

/// 校验放置配置合法性：followers/voters 互斥、主区域必须落在 regions、schedule 取值受限。
pub fn check_policy_validation(settings: &PlacementSettings) -> Result<(), PolicyError> {
    if settings.followers > 0 && settings.voters > 0 {
        return Err(PolicyError::InvalidSettings);
    }
    // primary_region 非空时，必须出现在 regions 列表中。
    if !settings.primary_region.is_empty()
        && settings
            .regions
            .split(',')
            .all(|region| region.trim() != settings.primary_region)
    {
        return Err(PolicyError::InvalidSettings);
    }
    if !settings.schedule.is_empty()
        && !matches!(settings.schedule.as_str(), "EVEN" | "MAJORITY_IN_PRIMARY")
    {
        return Err(PolicyError::InvalidSettings);
    }
    Ok(())
}

/// 根据选项列表构造初始状态为 None 的策略信息。
pub fn build_policy_info(
    id: i64,
    name: &str,
    options: &[(PlacementOptionType, String, u64)],
) -> Result<PolicyInfo, PolicyError> {
    let mut settings = PlacementSettings::default();
    for (kind, text, number) in options {
        set_direct_placement_opt(&mut settings, *kind, text, *number)?;
    }
    Ok(PolicyInfo {
        id,
        name: name.to_string(),
        state: PolicyState::None,
        settings,
    })
}

/// 按选项类型写入对应 settings 字段；schedule 统一转大写。
pub fn set_direct_placement_opt(
    settings: &mut PlacementSettings,
    kind: PlacementOptionType,
    text: &str,
    number: u64,
) -> Result<(), PolicyError> {
    match kind {
        PlacementOptionType::PrimaryRegion => settings.primary_region = text.to_string(),
        PlacementOptionType::Regions => settings.regions = text.to_string(),
        PlacementOptionType::FollowerCount => settings.followers = number,
        PlacementOptionType::VoterCount => settings.voters = number,
        PlacementOptionType::LearnerCount => settings.learners = number,
        PlacementOptionType::Schedule => settings.schedule = text.to_string(),
        PlacementOptionType::Constraints => settings.constraints = text.to_string(),
        PlacementOptionType::LeaderConstraints => settings.leader_constraints = text.to_string(),
        PlacementOptionType::LearnerConstraints => settings.learner_constraints = text.to_string(),
        PlacementOptionType::FollowerConstraints => {
            settings.follower_constraints = text.to_string()
        }
        PlacementOptionType::VoterConstraints => settings.voter_constraints = text.to_string(),
        PlacementOptionType::SurvivalPreferences => {
            settings.survival_preferences = text.to_string()
        }
    }
    Ok(())
}

/// 清除表及其分区上的策略引用，返回是否发生变更。
pub fn remove_table_placement(table: &mut PlacementObject) -> bool {
    let mut changed = table.policy_ref.take().is_some();
    for partition in &mut table.partitions {
        changed |= partition.policy_ref.take().is_some();
    }
    changed
}

/// 处理表级放置：ignore 时清除引用，否则归一化表与分区引用。
pub fn handle_table_placement(
    table: &mut PlacementObject,
    catalog: &PlacementPolicyCatalog,
    ignore: bool,
) -> Result<bool, PolicyError> {
    if ignore {
        return Ok(remove_table_placement(table));
    }
    table.policy_ref = catalog.normalize_ref(table.policy_ref.take())?;
    for partition in &mut table.partitions {
        partition.policy_ref = catalog.normalize_ref(partition.policy_ref.take())?;
    }
    Ok(false)
}

/// 从 PD range ID（形如 `policyName_rule_xxx`）提取策略名；无法解析则返回空串。
pub fn get_range_placement_policy_name(rule_id: Option<&str>) -> String {
    rule_id
        .and_then(|id| {
            id.rfind("_rule_")
                .filter(|position| *position > 0)
                .map(|position| id[..position].to_string())
        })
        .unwrap_or_default()
}

/// 收集对象及其分区上出现过的策略 ID 集合。
pub fn collect_policy_ids(objects: &[PlacementObject]) -> BTreeSet<i64> {
    objects
        .iter()
        .flat_map(|object| {
            object.policy_ref.iter().chain(
                object
                    .partitions
                    .iter()
                    .filter_map(|part| part.policy_ref.as_ref()),
            )
        })
        .map(|reference| reference.id)
        .collect()
}
