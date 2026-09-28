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

// Cascades 优化器门户：Memo、共享任务调度器与优化阶段 Context。
//
// Cascades 将逻辑计划拷贝进 Memo（记忆化搜索空间），按 Group/GroupExpression
// 组织等价子树，再通过串行 Scheduler 驱动 OptGroup 等任务完成规则探索与代价选择。
// 本文件提供会话侧 PlanContext、最小 Memo 门户、规则掩码，以及 Optimizer 入口。

use base::{Scheduler, Task};
use cascades_memo::{GroupExpressionRef, GroupRef};
use cascades_pattern::Operand;
use cascades_rule::Rule;
use logicalop::LogicalPlanRef;
use std::cell::{Ref, RefCell, RefMut};
use std::collections::{BTreeSet, HashMap};
use std::rc::Rc;
use task::{ContextRef, RuleRef, TaskError};

/// Session information used to size a cascades memo.
/// 会话侧计划上下文：用各算子数量向量预估 Memo 容量。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PlanContext {
    /// 各类算子数量，用于 `Memo::NewMemo` 容量预分配。
    pub operator_num: Vec<usize>,
}

/// Logical plans provide the context and root expression used by `Memo::Init`.
/// 逻辑计划入口：提供会话上下文与根逻辑表达式，供 Memo 初始化。
pub trait LogicalPlan {
    /// 返回用于尺寸化 Memo 的会话计划上下文。
    fn SCtx(&self) -> PlanContext;
    /// 构造根逻辑表达式；失败时返回 TaskError。
    fn RootExpression(&self) -> Result<LogicalPlanRef, TaskError>;
}

/// Minimal memo portal required by the optimizer and task context.
/// 优化器与任务上下文所需的最小 Memo 门户：持有根 GroupExpression 与根 Group。
pub struct Memo {
    /// 真实 Cascades Memo，负责 Group/GroupExpression 去重、合并和属性派生。
    inner: cascades_memo::Memo,
    /// 各类算子容量提示，来自 PlanContext.operator_num。
    capacities: Vec<usize>,
    /// 根 GroupExpression（表达式节点）的强引用。
    root: Option<GroupExpressionRef>,
}

impl Memo {
    /// 按容量向量构造空 Memo。
    pub fn NewMemo(capacities: Vec<usize>) -> Self {
        Self {
            inner: cascades_memo::Memo::NewMemo(
                &capacities
                    .iter()
                    .map(|capacity| *capacity as u64)
                    .collect::<Vec<_>>(),
            ),
            capacities,
            root: None,
        }
    }

    /// 用逻辑计划初始化根 Group / GroupExpression，并返回根表达式引用。
    pub fn Init(&mut self, plan: &dyn LogicalPlan) -> Result<GroupExpressionRef, TaskError> {
        let expression = self
            .inner
            .Init(plan.RootExpression()?)
            .map_err(|error| TaskError::New(error.to_string()))?;
        self.root = Some(expression.clone());
        Ok(expression)
    }

    /// 将逻辑表达式拷贝进目标 Group，返回新建的 GroupExpression。
    pub fn CopyIn(
        &mut self,
        target: &GroupRef,
        expression: LogicalPlanRef,
    ) -> Result<GroupExpressionRef, TaskError> {
        self.inner
            .CopyIn(Some(target.clone()), expression)
            .map_err(|error| TaskError::New(error.to_string()))
    }

    /// 从目标 Group 移除表达式，并标记该 GroupExpression 为已废弃（abandoned）。
    pub fn RemoveOut(&mut self, target: &GroupRef, expression: &GroupExpressionRef) {
        self.inner.RemoveOut(target, expression);
    }

    /// 取得当前根 GroupExpression（若已 Init）。
    pub fn GetRootGroupExpression(&self) -> Option<GroupExpressionRef> {
        self.root.clone()
    }

    /// 返回容量提示切片。
    pub fn Capacities(&self) -> &[usize] {
        &self.capacities
    }

    /// 清空根引用与容量，释放 Memo 持有状态。
    pub fn Destroy(&mut self) {
        self.inner.Destroy();
        self.root = None;
        self.capacities.clear();
    }
}

/// 规则启用掩码：可“全部启用”，或按规则 id 集合选择性启用。
#[derive(Default)]
pub struct RuleMask {
    /// 为 true 时 Test 对任意 id 返回 true。
    all_enabled: bool,
    /// 显式启用的规则 id 集合。
    enabled: BTreeSet<usize>,
    /// 在全启用模式下显式禁用的规则 id 集合。
    disabled: BTreeSet<usize>,
}

impl RuleMask {
    /// 开启“全部规则启用”模式。
    pub fn SetAll(&mut self) {
        self.all_enabled = true;
        self.disabled.clear();
    }

    /// 启用单个规则 id。
    pub fn Set(&mut self, id: usize) {
        self.enabled.insert(id);
        self.disabled.remove(&id);
    }

    /// Disable a rule id while retaining the all-enabled state when it is not
    /// explicitly represented by the Go bitset.  This operation is primarily
    /// useful to callers constructing a selective mask from scratch.
    pub fn Clear(&mut self, id: usize) {
        if self.all_enabled {
            self.disabled.insert(id);
        } else {
            self.enabled.remove(&id);
        }
    }

    /// 判断规则 id 是否可用（全部启用或集合包含）。
    pub fn Test(&self, id: usize) -> bool {
        let within_default_mask = id < cascades_rule::XFMaximumRuleLength as usize;
        (self.all_enabled && within_default_mask && !self.disabled.contains(&id))
            || self.enabled.contains(&id)
    }
}

/// A cloneable scheduler handle lets tasks enqueue work while the execution loop runs.
/// 可克隆的调度器句柄：执行循环运行中任务仍可通过共享栈入队后续工作。
#[derive(Clone, Default)]
struct SharedTaskScheduler {
    /// 共享任务栈；Rc+RefCell 对应 Go 侧可并发入队的队列语义（此处为单线程内部可变）。
    stack: Rc<RefCell<Vec<Box<dyn Task>>>>,
}

impl Scheduler for SharedTaskScheduler {
    fn ExecuteTasks(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        loop {
            // Drop the stack borrow before Execute: the task may push follow-up work.
            // 先结束对栈的借用再 Execute：任务执行中可能再 PushTask。
            let Some(mut next) = self.stack.borrow_mut().pop() else {
                return Ok(());
            };
            next.Execute()?;
        }
    }

    fn Destroy(&mut self) {
        self.stack.borrow_mut().clear();
    }

    fn PushTask(&mut self, task: Box<dyn Task>) {
        self.stack.borrow_mut().push(task);
    }
}

/// 任务侧上下文：向 Task 暴露 Memo 拷贝/移除、规则查询与入队入口。
struct TaskContext {
    memo: Rc<RefCell<Memo>>,
    scheduler: SharedTaskScheduler,
    rule_mask: Rc<RefCell<RuleMask>>,
    /// Operand -> 适用变换规则列表。
    rules: Rc<RefCell<HashMap<Operand, Vec<RuleRef>>>>,
}

impl task::Context for TaskContext {
    fn PushTask(&mut self, task: Box<dyn Task>) {
        self.scheduler.PushTask(task);
    }

    fn CopyIn(
        &mut self,
        target: &GroupRef,
        expression: LogicalPlanRef,
    ) -> Result<GroupExpressionRef, TaskError> {
        self.memo.borrow_mut().CopyIn(target, expression)
    }

    fn CopyInWithChildren(
        &mut self,
        target: &GroupRef,
        expression: LogicalPlanRef,
        child_groups: Vec<GroupRef>,
    ) -> Result<GroupExpressionRef, TaskError> {
        self.memo
            .borrow_mut()
            .inner
            .CopyInWithGroupChildren(Some(target.clone()), expression, child_groups)
            .map_err(|error| TaskError::New(error.to_string()))
    }

    fn RemoveOut(&mut self, target: &GroupRef, expression: &GroupExpressionRef) {
        self.memo.borrow_mut().RemoveOut(target, expression);
    }

    fn RulesFor(&self, operand: Operand) -> Vec<RuleRef> {
        self.rules
            .borrow()
            .get(&operand)
            .cloned()
            .unwrap_or_default()
    }

    fn RuleEnabled(&self, rule_id: usize) -> bool {
        self.rule_mask.borrow().Test(rule_id)
    }
}

/// All state owned by one memo optimization phase.
/// 一次 Memo 优化阶段拥有的全部状态：计划上下文、Memo、调度器与规则表。
pub struct Context {
    pctx: PlanContext,
    mm: Rc<RefCell<Memo>>,
    scheduler: SharedTaskScheduler,
    ruleMask: Rc<RefCell<RuleMask>>,
    rules: Rc<RefCell<HashMap<Operand, Vec<RuleRef>>>>,
}

impl Context {
    /// 按会话上下文构造优化阶段；默认 SetAll 启用全部规则。
    pub fn NewContext(pctx: PlanContext) -> Self {
        let mut rule_mask = RuleMask::default();
        rule_mask.SetAll();
        Self {
            mm: Rc::new(RefCell::new(Memo::NewMemo(pctx.operator_num.clone()))),
            pctx,
            scheduler: SharedTaskScheduler::default(),
            ruleMask: Rc::new(RefCell::new(rule_mask)),
            rules: Rc::new(RefCell::new(HashMap::new())),
        }
    }

    /// 生成可交给具体 Task 的 ContextRef（共享 Memo/调度器/规则状态）。
    fn TaskContext(&self) -> ContextRef {
        Rc::new(RefCell::new(TaskContext {
            memo: self.mm.clone(),
            scheduler: self.scheduler.clone(),
            rule_mask: self.ruleMask.clone(),
            rules: self.rules.clone(),
        }))
    }

    /// 销毁 Memo 与调度器持有资源。
    pub fn Destroy(&mut self) {
        self.mm.borrow_mut().Destroy();
        self.scheduler.Destroy();
    }

    /// 取得调度器只读引用。
    pub fn GetScheduler(&self) -> &dyn Scheduler {
        &self.scheduler
    }

    /// 取得调度器可变引用。
    pub fn GetSchedulerMut(&mut self) -> &mut dyn Scheduler {
        &mut self.scheduler
    }

    /// 将任务压入共享调度栈。
    pub fn PushTask(&mut self, task: Box<dyn Task>) {
        self.scheduler.PushTask(task);
    }

    /// 借用内部 Memo（Ref 守卫，结束借用后可再可变访问）。
    pub fn GetMemo(&self) -> Ref<'_, Memo> {
        self.mm.borrow()
    }

    /// 取得会话计划上下文。
    pub fn GetPlanContext(&self) -> &PlanContext {
        &self.pctx
    }

    /// Return the rule mask exposed by Go's `Context.GetRuleMask` contract.
    pub fn GetRuleMask(&self) -> Ref<'_, RuleMask> {
        self.ruleMask.borrow()
    }

    /// Return mutable access to the rule mask for context owners.
    pub fn GetRuleMaskMut(&self) -> RefMut<'_, RuleMask> {
        self.ruleMask.borrow_mut()
    }

    /// 按 operand 名注册一批变换规则。
    pub fn RegisterRules(&mut self, operand: Operand, rules: Vec<RuleRef>) {
        self.rules.borrow_mut().insert(operand, rules);
    }
}

/// Cascades search portal driven by a memo and a serial task scheduler.
/// Cascades 搜索门户：持有逻辑计划与优化 Context，由串行任务调度驱动。
pub struct Optimizer {
    logic: Box<dyn LogicalPlan>,
    ctx: Context,
}

impl Optimizer {
    /// 构造优化器：Init Memo 根，并压入对根 Group 的 OptGroup 任务。
    pub fn NewOptimizer(logic: Box<dyn LogicalPlan>) -> Result<Self, TaskError> {
        let mut optimizer = Self {
            ctx: Context::NewContext(logic.SCtx()),
            logic,
        };
        let root_expression = optimizer
            .ctx
            .mm
            .borrow_mut()
            .Init(optimizer.logic.as_ref())?;
        let root_group = root_expression
            .borrow()
            .GetGroup()
            .expect("initialized root expression must belong to a group");
        // 从根 Group 开始探索：NewOptGroupTask 会继续派生表达式级任务。
        optimizer.ctx.PushTask(task::NewOptGroupTask(
            optimizer.ctx.TaskContext(),
            root_group,
        ));
        Ok(optimizer)
    }

    /// 运行调度循环直至任务栈清空或出错。
    pub fn Execute(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        self.ctx.scheduler.ExecuteTasks()
    }

    /// 销毁内部 Context。
    pub fn Destroy(&mut self) {
        self.ctx.Destroy();
    }

    /// 取得当前 Memo 的只读借用。
    pub fn GetMemo(&self) -> Ref<'_, Memo> {
        self.ctx.GetMemo()
    }

    /// 按规则 id 列表显式启用规则（在默认全部启用之外再 Set 指定 id）。
    pub fn SetRules(&mut self, ids: &[usize]) {
        for id in ids {
            self.ctx.ruleMask.borrow_mut().Set(*id);
        }
    }

    /// 转发到 Context::RegisterRules。
    pub fn RegisterRules(&mut self, operand: Operand, rules: Vec<RuleRef>) {
        self.ctx.RegisterRules(operand, rules);
    }
}

/// 包级构造函数：转发到 `Context::NewContext`。
pub fn NewContext(pctx: PlanContext) -> Context {
    Context::NewContext(pctx)
}

/// 包级构造函数：转发到 `Optimizer::NewOptimizer`。
pub fn NewOptimizer(logic: Box<dyn LogicalPlan>) -> Result<Optimizer, TaskError> {
    Optimizer::NewOptimizer(logic)
}
