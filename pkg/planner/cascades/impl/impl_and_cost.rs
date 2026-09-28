// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
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

// Cascades 物理实现与代价枚举核心类型。
//
// 定义 Task（物理执行任务）、PhysicalProperty（期望的物理属性，如任务类型与
// 期望行数）、CostEngine 回调边界，以及 `implement_group_and_cost` /
// `implement_memo_and_cost`：在 Memo 组上按属性找最优物理计划并计算代价。

use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

/// 本模块统一错误类型，包装可读消息字符串。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Error(String);

impl Error {
    /// 由任意可转 String 的消息构造错误。
    pub fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for Error {}
/// 本模块 Result 别名。
pub type Result<T> = std::result::Result<T, Error>;

/// 物理任务执行位置/形态：Root（TiDB 节点）、Cop、BatchCop、Mpp。
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum TaskType {
    #[default]
    Root,
    Cop,
    BatchCop,
    Mpp,
}

/// 物理属性：任务类型 + 期望输出行数（expected count），用于代价枚举与缓存键。
#[derive(Clone, Debug, PartialEq)]
pub struct PhysicalProperty {
    pub task_type: TaskType,
    pub expected_count: f64,
}

impl PhysicalProperty {
    /// 将 expected_count 以位模式纳入缓存键，避免 f64 直接作 Hash 不稳定。
    fn cache_key(&self) -> (TaskType, u64) {
        (self.task_type, self.expected_count.to_bits())
    }
}

/// 物理计划：可解析列下标并按任务类型估代价。
pub trait PhysicalPlan: Send {
    fn resolve_indices(&mut self) -> Result<()>;
    fn cost(&self, task_type: TaskType) -> Result<f64>;
}

/// 物理执行任务：持有物理计划、可复制、可标记无效，并携带警告。
pub trait Task: Send {
    fn copy_task(&self) -> Box<dyn Task>;
    fn invalid(&self) -> bool;
    fn plan(&self) -> Option<&dyn PhysicalPlan>;
    fn plan_mut(&mut self) -> Option<&mut (dyn PhysicalPlan + '_)>;
    fn take_plan(&mut self) -> Option<Box<dyn PhysicalPlan>>;
    fn warnings(&self) -> &[String];
}

/// 基础 Task 实现：包装物理计划、估计代价、警告与无效标志。
pub struct BasicTask {
    pub plan: Option<Box<dyn PhysicalPlan>>,
    pub estimated_cost: f64,
    pub warnings: Vec<String>,
    pub invalid: bool,
    /// 复制计划时使用的克隆回调（跨线程共享）。
    clone_plan: Arc<dyn Fn(&dyn PhysicalPlan) -> Box<dyn PhysicalPlan> + Send + Sync>,
}

impl BasicTask {
    /// 构造有效任务。
    pub fn new(
        plan: Box<dyn PhysicalPlan>,
        estimated_cost: f64,
        clone_plan: impl Fn(&dyn PhysicalPlan) -> Box<dyn PhysicalPlan> + Send + Sync + 'static,
    ) -> Self {
        Self {
            plan: Some(plan),
            estimated_cost,
            warnings: Vec::new(),
            invalid: false,
            clone_plan: Arc::new(clone_plan),
        }
    }

    /// 构造无效任务（代价为 +∞，无计划）。
    pub fn invalid() -> Self {
        Self {
            plan: None,
            estimated_cost: f64::INFINITY,
            warnings: Vec::new(),
            invalid: true,
            clone_plan: Arc::new(|_| unreachable!("invalid task has no plan")),
        }
    }
}

impl Task for BasicTask {
    fn copy_task(&self) -> Box<dyn Task> {
        Box::new(Self {
            plan: self.plan.as_deref().map(|plan| (self.clone_plan)(plan)),
            estimated_cost: self.estimated_cost,
            warnings: self.warnings.clone(),
            invalid: self.invalid,
            clone_plan: Arc::clone(&self.clone_plan),
        })
    }

    fn invalid(&self) -> bool {
        self.invalid || self.plan.is_none()
    }

    fn plan(&self) -> Option<&dyn PhysicalPlan> {
        self.plan.as_deref()
    }

    fn plan_mut(&mut self) -> Option<&mut (dyn PhysicalPlan + '_)> {
        match self.plan.as_mut() {
            Some(plan) => Some(plan.as_mut()),
            None => None,
        }
    }

    fn take_plan(&mut self) -> Option<Box<dyn PhysicalPlan>> {
        self.plan.take()
    }

    fn warnings(&self) -> &[String] {
        &self.warnings
    }
}

/// 本文件内简化的逻辑组表达式标识（仅名称），供代价引擎回调使用。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GroupExpression {
    pub name: String,
}

/// 会话侧状态：任务映射备份时间戳、解耦 TiFlash、MPP 开关与警告列表。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SessionState {
    pub task_map_backup_timestamp: u64,
    pub disaggregated_tiflash: bool,
    pub mpp_allowed: bool,
    pub warnings: Vec<String>,
}

/// 简化 Group：逻辑表达式列表 + 会话状态 + 按物理属性缓存的最优任务。
pub struct Group {
    pub logical_expressions: Vec<GroupExpression>,
    pub session: SessionState,
    best_tasks: HashMap<(TaskType, u64), Box<dyn Task>>,
}

impl Group {
    /// 由逻辑表达式列表构造 Group。
    pub fn new(logical_expressions: Vec<GroupExpression>) -> Self {
        Self {
            logical_expressions,
            session: SessionState::default(),
            best_tasks: HashMap::new(),
        }
    }

    /// 查询缓存的最优任务。
    pub fn get_best_task(&self, property: &PhysicalProperty) -> Option<&dyn Task> {
        self.best_tasks.get(&property.cache_key()).map(Box::as_ref)
    }

    /// 写入给定物理属性下的最优任务缓存。
    pub fn set_best_task(&mut self, property: &PhysicalProperty, task: Box<dyn Task>) {
        self.best_tasks.insert(property.cache_key(), task);
    }
}

/// Import-cycle boundary matching Go's physicalop/utilfuncp callbacks.
/// 导入环边界：对齐 Go `physicalop/utilfuncp` 回调，由外部注入找最优任务与代价比较。
pub trait CostEngine {
    fn find_best_task(
        &mut self,
        expression: &GroupExpression,
        property: &PhysicalProperty,
    ) -> Result<Box<dyn Task>>;

    fn task_plan_cost(&self, task: &dyn Task) -> Result<(f64, bool)>;

    fn candidate_is_better(&self, candidate: &dyn Task, best: &dyn Task) -> Result<bool>;

    fn invalid_task(&self) -> Box<dyn Task>;
}

/// 基于闭包的 `CostEngine` 实现，便于测试与解耦。
pub struct CallbackCostEngine {
    pub find_best_task_fn:
        Box<dyn FnMut(&GroupExpression, &PhysicalProperty) -> Result<Box<dyn Task>>>,
    pub task_plan_cost_fn: Box<dyn Fn(&dyn Task) -> Result<(f64, bool)>>,
    pub compare_task_cost_fn: Box<dyn Fn(&dyn Task, &dyn Task) -> Result<bool>>,
    pub invalid_task_fn: Box<dyn Fn() -> Box<dyn Task>>,
}

impl CostEngine for CallbackCostEngine {
    fn find_best_task(
        &mut self,
        expression: &GroupExpression,
        property: &PhysicalProperty,
    ) -> Result<Box<dyn Task>> {
        (self.find_best_task_fn)(expression, property)
    }

    fn task_plan_cost(&self, task: &dyn Task) -> Result<(f64, bool)> {
        (self.task_plan_cost_fn)(task)
    }

    fn candidate_is_better(&self, candidate: &dyn Task, best: &dyn Task) -> Result<bool> {
        (self.compare_task_cost_fn)(candidate, best)
    }

    fn invalid_task(&self) -> Box<dyn Task> {
        (self.invalid_task_fn)()
    }
}

/// Implements and costs one memo group. `None` is Go's nil task when a cached
/// plan is valid but exceeds the caller's cost limit.
///
/// 对单个 Memo Group 做物理实现与代价比较；当缓存计划有效但超过代价上限时
/// 返回 `None`（对齐 Go 的 nil task）。
pub fn implement_group_and_cost(
    group: &mut Group,
    property: &PhysicalProperty,
    cost_limit: f64,
    engine: &mut dyn CostEngine,
) -> Result<Option<Box<dyn Task>>> {
    // 命中缓存：复制任务，校验无效标志与代价上限。
    if let Some(cached) = group.get_best_task(property) {
        let cached = cached.copy_task();
        let (cost, invalid) = engine.task_plan_cost(cached.as_ref())?;
        if invalid {
            return Ok(Some(engine.invalid_task()));
        }
        return Ok((cost <= cost_limit).then_some(cached));
    }

    // 枚举组内每个逻辑表达式，保留代价更优的候选。
    let mut best_task = engine.invalid_task();
    for expression in &group.logical_expressions {
        let candidate = engine.find_best_task(expression, property)?;
        if engine.candidate_is_better(candidate.as_ref(), best_task.as_ref())? {
            best_task = candidate;
        }
    }
    // InvalidTask is cached too, recording that this property has been fully searched.
    // 无效任务也会缓存，标记该物理属性已完整搜索过。
    group.set_best_task(property, best_task.copy_task());
    Ok(Some(best_task))
}

/// Physicalizes the root memo, transfers warnings, resolves indices, and only
/// then computes cost, preserving the Go portal's observable order.
///
/// 物理化根 Memo：传递警告、解析列下标，再计算代价，保持与 Go 入口可观测顺序一致。
pub fn implement_memo_and_cost(
    root: &mut Group,
    engine: &mut dyn CostEngine,
) -> Result<(Box<dyn PhysicalPlan>, f64)> {
    if root.logical_expressions.is_empty() {
        return Err(Error::new("root group must contain a logical expression"));
    }
    let property = PhysicalProperty {
        task_type: TaskType::Root,
        expected_count: f64::MAX,
    };
    let mut task = implement_group_and_cost(root, &property, f64::MAX, engine)?
        .unwrap_or_else(|| engine.invalid_task());

    root.session.task_map_backup_timestamp = 0;
    if task.invalid() {
        // 找不到合适物理计划；解耦 TiFlash 且未开 MPP 时附加提示。
        let mut message = "Can't find a proper physical plan for this query".to_owned();
        if root.session.disaggregated_tiflash && !root.session.mpp_allowed {
            message.push_str(
                ": cop and batchCop are not allowed in disaggregated tiflash mode, you should turn on tidb_allow_mpp switch",
            );
        }
        return Err(Error::new(message));
    }

    root.session
        .warnings
        .extend(task.warnings().iter().cloned());
    task.plan_mut()
        .ok_or_else(|| Error::new("valid task has no physical plan"))?
        .resolve_indices()?;
    let cost = task
        .plan()
        .ok_or_else(|| Error::new("valid task has no physical plan"))?
        .cost(TaskType::Root)?;
    let plan = task
        .take_plan()
        .ok_or_else(|| Error::new("valid task has no physical plan"))?;
    Ok((plan, cost))
}

/// Go 风格命名导出：实现并代价化单个 Group。
#[allow(non_snake_case)]
pub fn ImplementGroupAndCost(
    group: &mut Group,
    property: &PhysicalProperty,
    cost_limit: f64,
    engine: &mut dyn CostEngine,
) -> Result<Option<Box<dyn Task>>> {
    implement_group_and_cost(group, property, cost_limit, engine)
}

/// Go 风格命名导出：实现并代价化根 Memo。
#[allow(non_snake_case)]
pub fn ImplementMemoAndCost(
    root: &mut Group,
    engine: &mut dyn CostEngine,
) -> Result<(Box<dyn PhysicalPlan>, f64)> {
    implement_memo_and_cost(root, engine)
}
