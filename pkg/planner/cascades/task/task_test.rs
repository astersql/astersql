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

// Cascades 任务栈（Stack）与栈池行为的单元测试。
//
// 对应 Go `task_test.go`：校验 64 位下指针/切片布局、stackPool 容量与 LIFO、
// 脏栈复用、Destroy 清空，以及与 Go benchmark 同形的压弹循环。

// 本文件对应 pkg/planner/cascades/task/task_test.go。直接对接生产 Stack / stackPool
//（takeTaskStack/putTaskStack/Destroy）与 Task::Desc，分支对齐 Go 用例。

#![allow(non_snake_case)]

use crate::{Context, ContextRef, GroupExpressionRef, NewOptGroupTask, RuleRef, TaskError};
use crate::{Task, newTaskStack, newTaskStackWithCap, putTaskStack, takeTaskStack};
use cascades_base::util::{NewStrBuffer, StrBufferWriter};
use cascades_pattern::{EngineAll, NewPattern, Operand, OperandTableDual, Pattern};
use cascades_rule::{BaseRule, BoundPlan, DefaultNone, NewBaseRule, Rule, RuleError};
use logicalop::{LogicalLimit, LogicalPlanRef, LogicalTableDual};
use std::cell::RefCell;
use std::collections::HashMap;
use std::mem::{size_of, size_of_val};
use std::rc::Rc;

/// 测试用 Task：Execute 无操作，Desc 写出字段 a。
struct TestTaskImpl {
    a: i64,
}

impl Task for TestTaskImpl {
    fn Execute(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        Ok(())
    }

    fn Desc(&self, w: &mut dyn StrBufferWriter) {
        w.WriteString(&self.a.to_string());
    }
}

// TestTaskStack 对应 Go 的同名测试：断言 Stack 指针、任务切片、Task 胖指针在 64 位上的布局。
// Go 用 `unsafe.Sizeof` 量 `*Stack` / `[]Task` / `Task` interface；Rust 对等量
// `*const Stack` / `Vec<Box<dyn Task>>` / `Box<dyn Task>`。
#[test]
fn TestTaskStack() {
    let new_ss = newTaskStack();
    // size of pointer to Stack{}
    assert_eq!(size_of::<*const crate::Stack>(), 8);
    assert_eq!(size_of_val(&&new_ss), 8);
    // size of pointer to Stack.[]Task, cap + len + addr
    assert_eq!(size_of::<Vec<Box<dyn Task>>>(), 24);
    // size of Stack's Task element (fat pointer: data + vtable)
    assert_eq!(size_of::<Box<dyn Task>>(), 16);
}

// TestTaskFunctionality 对应 Go 的同名测试：验证 stackPool 初始容量、LIFO、空栈 Pop、
// 未 Destroy 归还后脏栈复用，以及 Destroy 清空后再取回。
#[test]
fn TestTaskFunctionality() {
    let mut ts = takeTaskStack();
    assert_eq!(ts.Len(), 0);
    assert_eq!(ts.Cap(), 4);

    ts.Push(Box::new(TestTaskImpl { a: 1 }));
    ts.Push(Box::new(TestTaskImpl { a: 2 }));

    // 后进先出：先弹出 2。
    let mut buf = Vec::new();
    {
        let one = ts.Pop().expect("pop task 2");
        let mut w = NewStrBuffer(&mut buf);
        one.Desc(w.as_mut());
        w.Flush();
    }
    assert_eq!(std::str::from_utf8(&buf).unwrap(), "2");

    buf.clear();
    {
        let one = ts.Pop().expect("pop task 1");
        let mut w = NewStrBuffer(&mut buf);
        one.Desc(w.as_mut());
        w.Flush();
    }
    assert_eq!(std::str::from_utf8(&buf).unwrap(), "1");

    // empty, pop nil.
    assert!(ts.Pop().is_none());

    ts.Push(Box::new(TestTaskImpl { a: 3 }));
    ts.Push(Box::new(TestTaskImpl { a: 4 }));
    ts.Push(Box::new(TestTaskImpl { a: 5 }));
    ts.Push(Box::new(TestTaskImpl { a: 6 }));
    // no clean, put it back
    // 故意不 Destroy，把脏栈放回池中以验证复用。
    putTaskStack(ts);

    // require again: Go 这里故意确认未 Destroy 的栈被原样复用。
    let mut ts = takeTaskStack();
    assert_eq!(ts.Len(), 4);
    assert_eq!(ts.Cap(), 4);

    for expected in ["6", "5", "4", "3"] {
        buf.clear();
        let one = ts
            .Pop()
            .unwrap_or_else(|| panic!("expected task {expected}"));
        {
            let mut w = NewStrBuffer(&mut buf);
            one.Desc(w.as_mut());
            w.Flush();
        }
        assert_eq!(std::str::from_utf8(&buf).unwrap(), expected);
    }
    assert!(ts.Pop().is_none());

    // self destroy.
    // Destroy 清空并归还空栈，再次 take 应得到空的容量-4 栈。
    ts.Destroy();
    let ts = takeTaskStack();
    assert_eq!(ts.Len(), 0);
    assert_eq!(ts.Cap(), 4);
}

/// taskStackForBench 对应 Go 的 benchmark 专用栈（不走生产栈池）。
// taskStackForBench 对应 Go 的 benchmark 专用栈。
struct TaskStackForBench {
    tasks: Vec<Box<dyn Task>>,
}

/// 按容量构造 benchmark 用栈。
fn new_task_stack_for_bench_with_cap(c: usize) -> TaskStackForBench {
    TaskStackForBench {
        tasks: Vec::with_capacity(c),
    }
}

impl TaskStackForBench {
    fn Push(&mut self, one: Box<dyn Task>) {
        self.tasks.push(one);
    }

    fn Pop(&mut self) -> Option<Box<dyn Task>> {
        self.tasks.pop()
    }
}

// BenchmarkTestStack2Pointer 对应 Go 同名 benchmark：用指针栈跑 1000 次压/弹。
// Rust 没有稳定 #[bench]，这里保留相同循环形状并真实执行一轮（N=1）。
#[test]
fn BenchmarkTestStack2Pointer() {
    let mut stack = new_task_stack_for_bench_with_cap(1000);
    for _ in 0..1 {
        for idx in 0..1000 {
            stack.Push(Box::new(TestTaskImpl { a: idx }));
        }
        for _ in 0..1000 {
            stack.Pop();
        }
    }
    assert!(stack.Pop().is_none());
}

// BenchmarkTestStackInterface 对应 Go 同名 benchmark：用生产 newTaskStackWithCap。
#[test]
fn BenchmarkTestStackInterface() {
    let mut stack = newTaskStackWithCap(1000);
    for _ in 0..1 {
        for idx in 0..1000 {
            stack.Push(Box::new(TestTaskImpl { a: idx }));
        }
        for _ in 0..1000 {
            stack.Pop();
        }
    }
    assert!(stack.Empty());
}

/// 最小真实 Memo 上的规则：把 TableDual 转换成 Limit，避免再次命中同一规则。
struct EmitLimitRule {
    base: BaseRule,
}

impl EmitLimitRule {
    fn new() -> Self {
        Self {
            base: NewBaseRule(DefaultNone, NewPattern(OperandTableDual, EngineAll)),
        }
    }
}

impl Rule for EmitLimitRule {
    fn ID(&self) -> usize {
        self.base.ID()
    }

    fn String(&self, writer: &mut dyn cascades_util::StrBufferWriter) {
        self.base.String(writer);
    }

    fn Pattern(&self) -> &Pattern {
        self.base.Pattern()
    }

    fn XForm(&self, _plan: &BoundPlan) -> Result<(Vec<LogicalPlanRef>, bool), RuleError> {
        Ok((
            vec![Box::new(LogicalLimit {
                Count: 1,
                ..Default::default()
            })],
            false,
        ))
    }
}

/// 真实任务链必须能从 Memo 中枚举 GE、运行 Binder、CopyIn 新计划并继续调度。
struct TaskContextHarness {
    memo: cascades_memo::Memo,
    stack: Rc<RefCell<Vec<Box<dyn Task>>>>,
    rules: HashMap<Operand, Vec<RuleRef>>,
}

impl Context for TaskContextHarness {
    fn PushTask(&mut self, task: Box<dyn Task>) {
        self.stack.borrow_mut().push(task);
    }

    fn CopyIn(
        &mut self,
        target: &cascades_memo::GroupRef,
        expression: LogicalPlanRef,
    ) -> Result<GroupExpressionRef, TaskError> {
        self.memo
            .CopyIn(Some(target.clone()), expression)
            .map_err(|error| TaskError::New(error.to_string()))
    }

    fn CopyInWithChildren(
        &mut self,
        target: &cascades_memo::GroupRef,
        expression: LogicalPlanRef,
        child_groups: Vec<cascades_memo::GroupRef>,
    ) -> Result<GroupExpressionRef, TaskError> {
        self.memo
            .CopyInWithGroupChildren(Some(target.clone()), expression, child_groups)
            .map_err(|error| TaskError::New(error.to_string()))
    }

    fn RemoveOut(&mut self, target: &cascades_memo::GroupRef, expression: &GroupExpressionRef) {
        self.memo.RemoveOut(target, expression);
    }

    fn RulesFor(&self, operand: Operand) -> Vec<RuleRef> {
        self.rules.get(&operand).cloned().unwrap_or_default()
    }

    fn RuleEnabled(&self, _rule_id: usize) -> bool {
        true
    }
}

#[test]
fn task_chain_uses_real_memo_and_rule_contracts() {
    let stack = Rc::new(RefCell::new(Vec::<Box<dyn Task>>::new()));
    let rule: RuleRef = Rc::new(EmitLimitRule::new());
    let mut memo = cascades_memo::Memo::NewMemo(&[]);
    let root = memo
        .Init(Box::new(LogicalTableDual::default()))
        .expect("real memo should initialize a logical plan");
    let group = root
        .borrow()
        .GetGroup()
        .expect("root GE should have an owning group");
    let mut rules = HashMap::new();
    rules.insert(OperandTableDual, vec![rule]);
    let context: ContextRef = Rc::new(RefCell::new(TaskContextHarness {
        memo,
        stack: stack.clone(),
        rules,
    }));

    context
        .borrow_mut()
        .PushTask(NewOptGroupTask(context.clone(), group.clone()));
    loop {
        let next = { stack.borrow_mut().pop() };
        let Some(mut task) = next else { break };
        task.Execute().expect("task chain should execute");
    }

    assert!(group.borrow().IsExplored());
    assert_eq!(group.borrow().GetLogicalExpressions().len(), 2);
    assert!(root.borrow().IsExplored(0));
}
