// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
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

// Copyright 2013 The ql Authors. All rights reserved.
// Use of this source code is governed by a BSD-style
// license that can be found in the LICENSES/QL-LICENSE file.

// 运行中 DDL 作业的冲突管理模块。
//
// DDL（Data Definition Language，数据定义语言）指 CREATE/ALTER/DROP 等修改
// 库表结构的语句。数据库内核会把每条 DDL 语句封装为一个"作业"（job）异步执行，
// 多个 DDL 作业可以并发运行，但若它们涉及（involve）相同的对象
// （库表、放置策略、资源组），就可能互相冲突。
//
// 本模块负责：
// - 记录当前正在运行的 DDL 作业各自涉及了哪些对象；
// - 在调度新作业前判断它是否与运行中/挂起中的作业冲突（`check_runnable`）；
// - 维护"独占/共享"两种占用模式的引用计数，作业结束后正确释放。
//
// 这类似于一个简化的表级读写锁管理器：Shared（共享）相当于读锁，
// Exclusive（独占）相当于写锁。

use std::collections::{BTreeMap, BTreeSet};

/// 通配符 `*`，表示涉及某一类别下的"全部"对象（如所有库或某库下所有表）。
pub const INVOLVING_ALL: &str = "*";
/// 空字符串，表示"不涉及"该类别的任何对象。
pub const INVOLVING_NONE: &str = "";

/// 作业对涉及对象的占用模式，语义上等价于读写锁的读/写模式。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InvolvingMode {
    /// 共享模式：多个共享占用可以共存，但与独占模式冲突（类似读锁）。
    Shared,
    /// 独占模式：与任何其他占用都冲突（类似写锁）。
    Exclusive,
}

impl Default for InvolvingMode {
    /// 默认采用独占模式，保证在未显式声明时按最严格的冲突规则处理。
    fn default() -> Self {
        Self::Exclusive
    }
}

/// 描述一个 DDL 作业所涉及的单个对象。
///
/// 一个实例只能属于以下三种类别之一（由 `assert_valid_info` 校验）：
/// - 库表：`database` 与 `table` 同时非空；
/// - 放置策略（placement policy，控制数据副本物理放置位置的规则）：`policy` 非空；
/// - 资源组（resource group，用于限制/隔离资源使用的分组）：`resource_group` 非空。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct InvolvingSchemaInfo {
    /// 涉及的数据库名；可为 `INVOLVING_ALL`（全部）或 `INVOLVING_NONE`（不涉及）。
    pub database: String,
    /// 涉及的表名；与 `database` 必须同时为空或同时非空。
    pub table: String,
    /// 涉及的放置策略名。
    pub policy: String,
    /// 涉及的资源组名。
    pub resource_group: String,
    /// 占用模式（共享/独占）。
    pub mode: InvolvingMode,
}

impl InvolvingSchemaInfo {
    /// 构造一个"库表"类别的涉及信息，默认独占模式。
    pub fn schema(database: impl Into<String>, table: impl Into<String>) -> Self {
        Self {
            database: database.into(),
            table: table.into(),
            ..Self::default()
        }
    }

    /// 构造一个"放置策略"类别的涉及信息，默认独占模式。
    pub fn policy(policy: impl Into<String>) -> Self {
        Self {
            policy: policy.into(),
            ..Self::default()
        }
    }

    /// 构造一个"资源组"类别的涉及信息，默认独占模式。
    pub fn resource_group(resource_group: impl Into<String>) -> Self {
        Self {
            resource_group: resource_group.into(),
            ..Self::default()
        }
    }

    /// 将占用模式改为共享（链式调用风格）。
    pub fn shared(mut self) -> Self {
        self.mode = InvolvingMode::Shared;
        self
    }
}

/// 按类别聚合的"被占用对象"集合，值为引用计数。
///
/// 因为多个作业可能涉及同一对象（尤其是共享模式），需要用计数记录
/// 有多少个作业正占用该对象，计数归零时才将其从集合中移除。
#[derive(Clone, Debug, Default)]
struct Objects {
    /// 库名 -> (表名 -> 引用计数) 的两级映射。
    schemas: BTreeMap<String, BTreeMap<String, usize>>,
    /// 放置策略名 -> 引用计数。
    placement_policies: BTreeMap<String, usize>,
    /// 资源组名 -> 引用计数。
    resource_groups: BTreeMap<String, usize>,
}

impl Objects {
    /// 三个类别的集合是否全部为空（即当前没有任何对象被占用）。
    fn is_empty(&self) -> bool {
        self.schemas.is_empty()
            && self.placement_policies.is_empty()
            && self.resource_groups.is_empty()
    }

    /// 登记一个涉及对象：按其所属类别把对应的引用计数加一。
    fn add(&mut self, info: &InvolvingSchemaInfo) {
        if info.database != INVOLVING_NONE {
            *self
                .schemas
                .entry(info.database.clone())
                .or_default()
                .entry(info.table.clone())
                .or_default() += 1;
        }
        if info.policy != INVOLVING_NONE {
            *self
                .placement_policies
                .entry(info.policy.clone())
                .or_default() += 1;
        }
        if info.resource_group != INVOLVING_NONE {
            *self
                .resource_groups
                .entry(info.resource_group.clone())
                .or_default() += 1;
        }
    }

    /// 注销一个涉及对象：按其所属类别把对应的引用计数减一，计数归零则删除条目。
    fn remove(&mut self, info: &InvolvingSchemaInfo) {
        if info.database != INVOLVING_NONE {
            if let Some(tables) = self.schemas.get_mut(&info.database) {
                decrement(tables, &info.table);
                // 库下已无任何被占用的表时，把库条目也移除，保持映射精简。
                if tables.is_empty() {
                    self.schemas.remove(&info.database);
                }
            }
        }
        if info.policy != INVOLVING_NONE {
            decrement(&mut self.placement_policies, &info.policy);
        }
        if info.resource_group != INVOLVING_NONE {
            decrement(&mut self.resource_groups, &info.resource_group);
        }
    }

    /// 判断给定涉及对象是否与本集合中已占用的对象冲突。
    /// 库表类别需要考虑 `*` 通配符匹配；策略与资源组按名字精确匹配。
    fn conflicts(&self, info: &InvolvingSchemaInfo) -> bool {
        if info.database != INVOLVING_NONE {
            return has_schema_conflict(&info.database, &info.table, &self.schemas);
        }
        if info.policy != INVOLVING_NONE {
            return self.placement_policies.contains_key(&info.policy);
        }
        self.resource_groups.contains_key(&info.resource_group)
    }

    /// 不变式检查：所有仍存在的条目的引用计数必须为正数（用于内部一致性校验）。
    fn counts_are_positive(&self) -> bool {
        self.schemas
            .values()
            .all(|tables| !tables.is_empty() && tables.values().all(|count| *count > 0))
            && self.placement_policies.values().all(|count| *count > 0)
            && self.resource_groups.values().all(|count| *count > 0)
    }
}

/// 将映射中指定键的引用计数减一；计数归零时删除该键。
fn decrement(map: &mut BTreeMap<String, usize>, key: &str) {
    if let Some(count) = map.get_mut(key) {
        *count -= 1;
        if *count == 0 {
            map.remove(key);
        }
    }
}

/// 运行中 DDL 作业的总登记表，是本模块的核心状态机。
///
/// 内部维护三份 `Objects` 集合：
/// - `exclusive`：运行中作业以独占模式占用的对象；
/// - `shared`：运行中作业以共享模式占用的对象；
/// - `pending`：曾因冲突被挂起的作业所涉及的对象，用于避免后来的作业
///   持续"插队"导致挂起作业饿死（starvation）。
#[derive(Default)]
pub struct RunningJobs {
    /// 作业 ID -> 该作业的涉及对象列表，记录所有正在运行的作业。
    running: BTreeMap<i64, Vec<InvolvingSchemaInfo>>,
    /// 独占模式占用的对象集合。
    exclusive: Objects,
    /// 共享模式占用的对象集合。
    shared: Objects,
    /// 挂起作业占用的对象集合（新作业也需避让这些对象）。
    pending: Objects,
}

impl RunningJobs {
    /// 判断给定作业当前是否可以开始运行（即与所有运行中/挂起中的作业均不冲突）。
    pub fn check_runnable(&self, job_id: i64, involves: &[InvolvingSchemaInfo]) -> bool {
        // 同一作业不能重复运行。
        if self.running.contains_key(&job_id) {
            return false;
        }
        // 已有作业独占了全部库（如 FLASHBACK CLUSTER 这类全局 DDL），任何新作业都不可运行。
        if self.exclusive.schemas.contains_key(INVOLVING_ALL) {
            return false;
        }
        // 快速路径：没有任何占用记录时必然无冲突。
        if self.exclusive.is_empty() && self.shared.is_empty() && self.pending.is_empty() {
            return true;
        }

        // 逐个检查涉及对象，全部无冲突才可运行。
        involves.iter().all(|info| {
            assert_valid_info(info);
            // 请求独占全部库表的作业必须等到系统完全空闲，此处直接判为不可运行。
            if info.database == INVOLVING_ALL
                && info.table == INVOLVING_ALL
                && info.mode == InvolvingMode::Exclusive
            {
                return false;
            }
            match info.mode {
                // 独占请求与独占、共享、挂起三类占用都互斥（类似写锁）。
                InvolvingMode::Exclusive => {
                    !self.exclusive.conflicts(info)
                        && !self.shared.conflicts(info)
                        && !self.pending.conflicts(info)
                }
                // 共享请求只与独占占用及挂起对象互斥，共享之间可以共存（类似读锁）。
                InvolvingMode::Shared => {
                    !self.exclusive.conflicts(info) && !self.pending.conflicts(info)
                }
            }
        })
    }

    /// 登记一个开始运行的作业：按各涉及对象的模式记入独占/共享集合。
    pub fn add_running(&mut self, job_id: i64, involves: Vec<InvolvingSchemaInfo>) {
        for info in &involves {
            assert_valid_info(info);
            match info.mode {
                InvolvingMode::Exclusive => self.exclusive.add(info),
                InvolvingMode::Shared => self.shared.add(info),
            }
        }
        self.running.insert(job_id, involves);
    }

    /// 结束或挂起一个作业：先从运行集合中释放其占用；
    /// 若 `move_to_pending` 为真，则把涉及对象转入挂起集合，
    /// 防止后续作业抢占从而饿死该作业。
    pub fn finish_or_pend_job(
        &mut self,
        job_id: i64,
        involves: Vec<InvolvingSchemaInfo>,
        move_to_pending: bool,
    ) {
        self.remove_running_involves(job_id, &involves);
        if move_to_pending {
            self.add_pending(involves);
        }
    }

    /// 按作业 ID 移除一个运行中的作业，并释放其全部占用。
    pub fn remove_running(&mut self, job_id: i64) {
        if let Some(involves) = self.running.get(&job_id).cloned() {
            self.remove_running_involves(job_id, &involves);
        }
    }

    /// 将一组涉及对象记入挂起集合（作业因冲突暂时无法运行时调用）。
    pub fn add_pending(&mut self, involves: Vec<InvolvingSchemaInfo>) {
        for info in &involves {
            assert_valid_info(info);
            self.pending.add(info);
        }
    }

    /// 清空全部挂起记录（通常在新一轮调度开始前重置）。
    pub fn reset_all_pending(&mut self) {
        self.pending = Objects::default();
    }

    /// 返回所有运行中作业 ID 的逗号分隔字符串，便于日志输出。
    pub fn all_ids(&self) -> String {
        self.running
            .keys()
            .map(i64::to_string)
            .collect::<Vec<_>>()
            .join(",")
    }
    /// 返回所有运行中作业 ID 的有序集合。
    pub fn running_ids(&self) -> BTreeSet<i64> {
        self.running.keys().copied().collect()
    }

    /// 内部不变式检查：三个占用集合中的引用计数均应为正数。
    pub fn invariants_hold(&self) -> bool {
        self.exclusive.counts_are_positive()
            && self.shared.counts_are_positive()
            && self.pending.counts_are_positive()
    }

    /// 从运行表中删除作业，并按各涉及对象的模式从独占/共享集合中释放占用。
    fn remove_running_involves(&mut self, job_id: i64, involves: &[InvolvingSchemaInfo]) {
        self.running.remove(&job_id);
        for info in involves {
            match info.mode {
                InvolvingMode::Exclusive => self.exclusive.remove(info),
                InvolvingMode::Shared => self.shared.remove(info),
            }
        }
    }
}

/// 判断两组涉及对象是否存在冲突：
/// 只要有一对对象重叠，且其中至少一方是独占模式，即视为冲突
/// （两个共享占用即使重叠也不冲突，符合读写锁语义）。
pub fn has_conflict(left: &[InvolvingSchemaInfo], right: &[InvolvingSchemaInfo]) -> bool {
    left.iter().any(|a| {
        right.iter().any(|b| {
            object_overlap(a, b)
                && (a.mode == InvolvingMode::Exclusive || b.mode == InvolvingMode::Exclusive)
        })
    })
}

/// 校验涉及信息的合法性：
/// 1. `database` 与 `table` 必须同时为空或同时非空；
/// 2. 库表、放置策略、资源组三种类别必须恰好指定其中一种。
fn assert_valid_info(info: &InvolvingSchemaInfo) {
    assert_eq!(
        info.database == INVOLVING_NONE,
        info.table == INVOLVING_NONE,
        "database and table must be specified together"
    );
    let categories = usize::from(info.database != INVOLVING_NONE)
        + usize::from(info.policy != INVOLVING_NONE)
        + usize::from(info.resource_group != INVOLVING_NONE);
    assert_eq!(
        categories, 1,
        "an involving object must identify exactly one category"
    );
}

/// 判断请求的库表与已占用的库表映射是否冲突。
/// 表级匹配需处理 `*` 通配符：请求全表、或已占用方登记了全表、
/// 或双方为同一张表时均视为冲突。
fn has_schema_conflict(
    request_database: &str,
    request_table: &str,
    schemas: &BTreeMap<String, BTreeMap<String, usize>>,
) -> bool {
    // 库名不存在于占用映射中则必然无冲突。
    let Some(tables) = schemas.get(request_database) else {
        return false;
    };
    request_table == INVOLVING_ALL
        || tables.contains_key(INVOLVING_ALL)
        || tables.contains_key(request_table)
}

/// 判断两个涉及对象是否指向重叠的实体。
/// 只有同类别的对象才可能重叠：库表按名字（支持 `*` 通配符）比较，
/// 放置策略与资源组按名字精确比较。
fn object_overlap(left: &InvolvingSchemaInfo, right: &InvolvingSchemaInfo) -> bool {
    if left.database != INVOLVING_NONE && right.database != INVOLVING_NONE {
        let database = left.database == INVOLVING_ALL
            || right.database == INVOLVING_ALL
            || left.database == right.database;
        let table = left.table == INVOLVING_ALL
            || right.table == INVOLVING_ALL
            || left.table == right.table;
        return database && table;
    }
    if left.policy != INVOLVING_NONE && right.policy != INVOLVING_NONE {
        return left.policy == right.policy;
    }
    if left.resource_group != INVOLVING_NONE && right.resource_group != INVOLVING_NONE {
        return left.resource_group == right.resource_group;
    }
    false
}
