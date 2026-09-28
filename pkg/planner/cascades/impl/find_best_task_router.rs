// Copyright 2026 AsterSQL.

// Cascades「找最优物理任务」路由与默认枚举实现。
//
// 为逻辑组表达式按物理属性穷举物理计划、递归求解子 Group 最优任务并比较代价；
// 通过 `OnceLock` 安装可替换的 find-best-task 处理器，打通物理算子依赖反转边界。
// 文件前半大段块注释保留历史完整实现草案。

use crate::impl_and_cost::{Error, PhysicalPlan, PhysicalProperty, Result, Task, TaskType};
use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, Mutex, OnceLock};

/// 路由用逻辑计划抽象：可查询类型名并穷举物理实现。
pub trait LogicalPlan: Any + Send + Sync {
    fn as_any(&self) -> &dyn Any;
    fn plan_type(&self) -> &str;
    fn exhaust_physical_plans(
        &self,
        property: &PhysicalProperty,
    ) -> Result<Vec<Box<dyn RoutePhysicalPlan>>>;
}

/// 带「子需求属性」与「挂接到 Task」能力的物理计划，供路由枚举使用。
pub trait RoutePhysicalPlan: PhysicalPlan {
    fn child_required_properties(&self) -> &[PhysicalProperty];
    fn attach_to_task(self: Box<Self>, children: Vec<Box<dyn Task>>) -> Result<Box<dyn Task>>;
}

/// 路由场景下的 Group 共享句柄（Arc + Mutex）。
pub type GroupRef = Arc<Mutex<RouteGroup>>;

/// 已路由的组表达式：逻辑计划 + 子 Group 输入。
pub struct RoutedGroupExpression {
    pub logical_plan: Box<dyn LogicalPlan>,
    pub inputs: Vec<GroupRef>,
}

impl RoutedGroupExpression {
    /// 由逻辑计划与子 Group 列表构造。
    pub fn new(logical_plan: Box<dyn LogicalPlan>, inputs: Vec<GroupRef>) -> Self {
        Self {
            logical_plan,
            inputs,
        }
    }
}

/// 路由用 Group：逻辑表达式列表 + 按物理属性缓存的最优任务。
pub struct RouteGroup {
    pub logical_expressions: Vec<Arc<RoutedGroupExpression>>,
    best_tasks: HashMap<(TaskType, u64), Box<dyn Task>>,
}

impl RouteGroup {
    /// 由表达式列表构造空缓存 Group。
    pub fn new(logical_expressions: Vec<Arc<RoutedGroupExpression>>) -> Self {
        Self {
            logical_expressions,
            best_tasks: HashMap::new(),
        }
    }
    /// 若已缓存则复制返回最优任务。
    fn cached_task(&self, property: &PhysicalProperty) -> Option<Box<dyn Task>> {
        self.best_tasks
            .get(&property_key(property))
            .map(|task| task.copy_task())
    }
    /// 写入最优任务缓存。
    fn set_best_task(&mut self, property: &PhysicalProperty, task: &dyn Task) {
        self.best_tasks
            .insert(property_key(property), task.copy_task());
    }
}

/// 逻辑计划路由身份：是否组表达式、包装类型 ID 与算子类型名。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LogicalPlanRoute {
    pub is_group_expression: bool,
    pub wrapped_type_id: TypeId,
    pub wrapped_plan_type: String,
}

/// 从逻辑计划提取路由身份（当前简化路径始终视为非组表达式包装）。
pub fn inspect_logical_plan_route(plan: &dyn LogicalPlan) -> LogicalPlanRoute {
    LogicalPlanRoute {
        is_group_expression: false,
        wrapped_type_id: plan.as_any().type_id(),
        wrapped_plan_type: plan.plan_type().to_owned(),
    }
}

/// find-best-task 处理器函数类型。
pub type FindBestTaskHandler =
    fn(&RoutedGroupExpression, &PhysicalProperty, &LogicalPlanRoute) -> Result<Box<dyn Task>>;

/// 进程级单次安装的 find-best-task 处理器。
static FIND_BEST_TASK_HANDLER: OnceLock<FindBestTaskHandler> = OnceLock::new();

/// 安装路由失败：处理器已被安装。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InstallFindBestTaskRouterError {
    HandlerAlreadyInstalled,
}

impl fmt::Display for InstallFindBestTaskRouterError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("cascades find-best-task handler has already been installed")
    }
}

impl std::error::Error for InstallFindBestTaskRouterError {}

/// 带代价的候选物理任务。
struct PlannedTask {
    task: Box<dyn Task>,
    cost: f64,
}

/// 读取 Group 缓存任务并按其物理计划计算代价。
fn cached_group_task(group: &GroupRef, property: &PhysicalProperty) -> Result<Option<PlannedTask>> {
    let task = group
        .lock()
        .map_err(|_| Error::new("memo group lock is poisoned"))?
        .cached_task(property);
    let Some(task) = task else { return Ok(None) };
    let cost = task
        .plan()
        .ok_or_else(|| Error::new("cached task has no plan"))?
        .cost(property.task_type)?;
    Ok(Some(PlannedTask { task, cost }))
}

/// 对 Group 在给定物理属性下找代价最低的物理任务，并写回缓存。
fn find_best_task_for_group(group: &GroupRef, property: &PhysicalProperty) -> Result<PlannedTask> {
    if let Some(cached) = cached_group_task(group, property)? {
        return Ok(cached);
    }
    let expressions = group
        .lock()
        .map_err(|_| Error::new("memo group lock is poisoned"))?
        .logical_expressions
        .clone();
    let mut best: Option<PlannedTask> = None;
    // 枚举组内表达式；与 Go ImplementGroupAndCost 一致，任一候选报错即停止。
    for expression in expressions {
        let candidate = find_best_task_for_expression(&expression, property)?;
        if best
            .as_ref()
            .is_none_or(|current| candidate.cost < current.cost)
        {
            best = Some(candidate);
        }
    }
    let best =
        best.ok_or_else(|| Error::new("no supported physical task exists for memo group"))?;
    group
        .lock()
        .map_err(|_| Error::new("memo group lock is poisoned"))?
        .set_best_task(property, best.task.as_ref());
    Ok(best)
}

/// 优先走已安装 handler；否则走默认穷举实现。
fn find_best_task_for_expression(
    expression: &RoutedGroupExpression,
    property: &PhysicalProperty,
) -> Result<PlannedTask> {
    let route = LogicalPlanRoute {
        is_group_expression: true,
        wrapped_type_id: expression.logical_plan.as_any().type_id(),
        wrapped_plan_type: expression.logical_plan.plan_type().to_owned(),
    };
    if let Some(handler) = FIND_BEST_TASK_HANDLER.get() {
        let task = handler(expression, property, &route)?;
        let cost = task
            .plan()
            .ok_or_else(|| Error::new("routed task has no plan"))?
            .cost(property.task_type)?;
        return Ok(PlannedTask { task, cost });
    }
    find_best_task_for_expression_default(expression, property)
}

/// 默认路径：穷举物理计划，递归求子任务代价，取总和最低者。
fn find_best_task_for_expression_default(
    expression: &RoutedGroupExpression,
    property: &PhysicalProperty,
) -> Result<PlannedTask> {
    let mut best: Option<PlannedTask> = None;
    for physical in expression.logical_plan.exhaust_physical_plans(property)? {
        let required = physical.child_required_properties();
        // 子需求属性个数须与输入 Group 个数一致。
        if required.len() != expression.inputs.len() {
            continue;
        }
        let local_cost = physical.cost(property.task_type)?;
        let mut child_cost = 0.0;
        let mut child_tasks = Vec::with_capacity(expression.inputs.len());
        for (child, child_property) in expression.inputs.iter().zip(required) {
            // Go iteratePhysicalPlan4GroupExpression preserves child errors.
            let child = find_best_task_for_group(child, child_property)?;
            child_cost += child.cost;
            child_tasks.push(child.task);
        }
        let task = physical.attach_to_task(child_tasks)?;
        let candidate = PlannedTask {
            task,
            cost: child_cost + local_cost,
        };
        if best
            .as_ref()
            .is_none_or(|current| candidate.cost < current.cost)
        {
            best = Some(candidate);
        }
    }
    best.ok_or_else(|| Error::new("no physical task satisfies the property"))
}

/// 委托逻辑计划穷举物理实现。
pub fn exhaust_physical_plans_for_group_expression(
    expression: &RoutedGroupExpression,
    property: &PhysicalProperty,
) -> Result<Vec<Box<dyn RoutePhysicalPlan>>> {
    expression.logical_plan.exhaust_physical_plans(property)
}

/// 默认 handler：用默认穷举路径返回最优 Task。
pub fn find_best_task_for_group_expression(
    expression: &RoutedGroupExpression,
    property: &PhysicalProperty,
    _: &LogicalPlanRoute,
) -> Result<Box<dyn Task>> {
    Ok(find_best_task_for_expression_default(expression, property)?.task)
}

/// 安装自定义 find-best-task 处理器（仅允许一次）。
pub fn install_cascades_find_best_task_router(
    handler: FindBestTaskHandler,
) -> std::result::Result<(), InstallFindBestTaskRouterError> {
    FIND_BEST_TASK_HANDLER
        .set(handler)
        .map_err(|_| InstallFindBestTaskRouterError::HandlerAlreadyInstalled)
}

/// 安装默认 Cascades find-best-task 路由。
pub fn install_default_cascades_find_best_task_router()
-> std::result::Result<(), InstallFindBestTaskRouterError> {
    install_cascades_find_best_task_router(find_best_task_for_group_expression)
}

/// 通过已安装 handler 为组表达式找最优任务。
pub fn route_find_best_task(
    expression: &RoutedGroupExpression,
    property: &PhysicalProperty,
) -> Result<Box<dyn Task>> {
    let route = LogicalPlanRoute {
        is_group_expression: true,
        wrapped_type_id: expression.logical_plan.as_any().type_id(),
        wrapped_plan_type: expression.logical_plan.plan_type().to_owned(),
    };
    let handler = FIND_BEST_TASK_HANDLER
        .get()
        .ok_or_else(|| Error::new("cascades find-best-task handler is not installed"))?;
    handler(expression, property, &route)
}

/// 物理属性缓存键：(任务类型, expected_count 位模式)。
fn property_key(property: &PhysicalProperty) -> (TaskType, u64) {
    (property.task_type, property.expected_count.to_bits())
}

/// Go 风格命名：检查逻辑计划路由身份。
#[allow(non_snake_case)]
pub fn InspectLogicalPlanRoute(plan: &dyn LogicalPlan) -> LogicalPlanRoute {
    inspect_logical_plan_route(plan)
}
/// Go 风格命名：穷举组表达式的物理计划。
#[allow(non_snake_case)]
pub fn ExhaustPhysicalPlans4GroupExpression(
    expression: &RoutedGroupExpression,
    property: &PhysicalProperty,
) -> Result<Vec<Box<dyn RoutePhysicalPlan>>> {
    exhaust_physical_plans_for_group_expression(expression, property)
}
/// Go 风格命名：为组表达式找最优任务。
#[allow(non_snake_case)]
pub fn FindBestTask4GroupExpression(
    expression: &RoutedGroupExpression,
    property: &PhysicalProperty,
    route: &LogicalPlanRoute,
) -> Result<Box<dyn Task>> {
    find_best_task_for_group_expression(expression, property, route)
}
/// Go 风格命名：安装 Cascades find-best-task 路由。
#[allow(non_snake_case)]
pub fn InstallCascadesFindBestTaskRouter(
    handler: FindBestTaskHandler,
) -> std::result::Result<(), InstallFindBestTaskRouterError> {
    install_cascades_find_best_task_router(handler)
}
/// Go 风格命名：安装默认路由。
#[allow(non_snake_case)]
pub fn InstallDefaultCascadesFindBestTaskRouter()
-> std::result::Result<(), InstallFindBestTaskRouterError> {
    install_default_cascades_find_best_task_router()
}
