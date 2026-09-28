// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0

// `cascades_ctx` 的 Aster 单元测试。
//
// 用轻量 `TestTask` / `TestScheduler` / `TestContext` 桩实现验证：
// `Context` 能正确转发任务入队、规则掩码读写，以及 Memo 的 Group 创建与销毁。

use crate::{Context, RuleMask};
use cascades_base::Scheduler as _;
use std::cell::RefCell;
use std::io;
use std::rc::Rc;

/// 测试用空任务：Execute 恒成功，Desc 写入固定文案。
struct TestTask;

impl cascades_base::Task for TestTask {
    fn Execute(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        Ok(())
    }

    fn Desc(&self, writer: &mut dyn cascades_base::util::StrBufferWriter) {
        writer.WriteString("test task");
    }
}

/// Records execution order and optionally reproduces a Go task error.
struct RecordingTask {
    id: usize,
    fail: bool,
    executed: Rc<RefCell<Vec<usize>>>,
}

impl cascades_base::Task for RecordingTask {
    fn Execute(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        self.executed.borrow_mut().push(self.id);
        if self.fail {
            return Err(io::Error::other(format!("mock error at task id = {}", self.id)).into());
        }
        Ok(())
    }

    fn Desc(&self, writer: &mut dyn cascades_base::util::StrBufferWriter) {
        writer.WriteString(&self.id.to_string());
    }
}

/// 串行测试调度器：任务存入向量，ExecuteTasks 按 Go 栈语义后进先出执行。
#[derive(Default)]
struct TestScheduler {
    /// 已入队、待执行的任务列表。
    tasks: Vec<Box<dyn cascades_base::Task>>,
    /// Destroy 是否已被调用。
    destroyed: bool,
}

impl cascades_base::Scheduler for TestScheduler {
    fn ExecuteTasks(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        // Go SimpleTaskScheduler 从栈顶弹出任务；任一任务失败则短路返回。
        while let Some(mut task) = self.tasks.pop() {
            task.Execute()?;
        }
        Ok(())
    }

    fn Destroy(&mut self) {
        self.tasks.clear();
        self.destroyed = true;
    }

    fn PushTask(&mut self, task: Box<dyn cascades_base::Task>) {
        self.tasks.push(task);
    }
}

/// 聚合调度器、Memo 与规则掩码的测试上下文。
struct TestContext {
    scheduler: TestScheduler,
    memo: memo::Memo,
    rules: RuleMask,
}

impl Context for TestContext {
    fn Destroy(&mut self) {
        // 与 Go Context.Destroy 一致：先清理 Memo，再释放调度器。
        self.memo.Destroy();
        self.scheduler.Destroy();
    }

    fn GetScheduler(&self) -> &dyn cascades_base::Scheduler {
        &self.scheduler
    }

    fn GetSchedulerMut(&mut self) -> &mut dyn cascades_base::Scheduler {
        &mut self.scheduler
    }

    fn GetMemo(&self) -> &memo::Memo {
        &self.memo
    }

    fn GetMemoMut(&mut self) -> &mut memo::Memo {
        &mut self.memo
    }

    fn GetRuleMask(&self) -> &RuleMask {
        &self.rules
    }

    fn GetRuleMaskMut(&mut self) -> &mut RuleMask {
        &mut self.rules
    }
}

/// 验证 Context 对 PushTask、RuleMask、Memo 的路由与 Destroy 清理效果。
#[test]
fn context_routes_tasks_memo_and_rule_mask() {
    let mut context = TestContext {
        scheduler: TestScheduler::default(),
        memo: memo::NewMemo(&[]),
        rules: RuleMask::default(),
    };
    // 经 Context 默认方法入队任务，并启用规则下标 7、新建一个 Group。
    context.PushTask(Box::new(TestTask));
    context.GetRuleMaskMut().Set(7);
    context.GetMemoMut().NewGroup();
    assert_eq!(context.scheduler.tasks.len(), 1);
    let _scheduler = context.GetScheduler();
    assert!(context.GetRuleMask().Test(7));
    assert_eq!(context.GetMemo().GetGroups().len(), 1);
    context.GetRuleMaskMut().Clear(7);
    assert!(!context.GetRuleMask().Test(7));
    context.scheduler.ExecuteTasks().unwrap();
    assert!(context.scheduler.tasks.is_empty());
    context.Destroy();
    assert!(context.scheduler.destroyed);
    assert!(context.GetMemo().GetGroups().is_empty());
}

#[test]
fn scheduler_executes_lifo_and_stops_at_first_error() {
    let executed = Rc::new(RefCell::new(Vec::new()));
    let mut scheduler = TestScheduler::default();
    for (id, fail) in [(1, false), (2, true), (3, false)] {
        scheduler.PushTask(Box::new(RecordingTask {
            id,
            fail,
            executed: Rc::clone(&executed),
        }));
    }

    let error = scheduler.ExecuteTasks().unwrap_err();
    assert_eq!(error.to_string(), "mock error at task id = 2");
    assert_eq!(&*executed.borrow(), &[3, 2]);
    assert_eq!(scheduler.tasks.len(), 1);
}
