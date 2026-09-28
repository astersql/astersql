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

// SimpleTaskScheduler 的单元测试。
//
// 对应 Go `task_scheduler_test.go`：验证 LIFO 执行顺序，以及任务返回错误时
// ExecuteTasks 短路、后续任务不被执行。

// 本文件对应 pkg/planner/cascades/task/task_scheduler_test.go。直接对接生产
// NewSimpleTaskScheduler / Scheduler::ExecuteTasks，验证任务错误短路返回。

#![allow(non_snake_case)]

use crate::{NewSimpleTaskScheduler, Task};
use cascades_base::util::StrBufferWriter;
use std::cell::RefCell;
use std::rc::Rc;

/// TestTaskImpl2 对应 Go 的同名类型：a == 2 时 Execute 返回固定错误。
// TestTaskImpl2 对应 Go 的同名类型：a == 2 时 Execute 返回固定错误。
struct TestTaskImpl2 {
    a: i64,
    executed: Rc<RefCell<Vec<i64>>>,
}

impl Task for TestTaskImpl2 {
    fn Execute(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        self.executed.borrow_mut().push(self.a);
        // a==2 模拟失败任务，用于验证调度器错误短路。
        if self.a == 2 {
            return Err("mock error at task id = 2".into());
        }
        Ok(())
    }

    fn Desc(&self, w: &mut dyn StrBufferWriter) {
        w.WriteString(&self.a.to_string());
    }
}

/// TestSimpleTaskScheduler 对应 Go 的同名测试。
/// LIFO：先执行 3（成功），再执行 2（失败并短路），1 不会被执行。
// TestSimpleTaskScheduler 对应 Go 的同名测试。
// LIFO：先执行 3（成功），再执行 2（失败并短路），1 不会被执行。
#[test]
fn TestSimpleTaskScheduler() {
    let mut test_scheduler = NewSimpleTaskScheduler();
    let executed = Rc::new(RefCell::new(Vec::new()));
    // 压栈顺序 1→2→3，LIFO 弹出顺序为 3→2→1。
    test_scheduler.PushTask(Box::new(TestTaskImpl2 {
        a: 1,
        executed: executed.clone(),
    }));
    test_scheduler.PushTask(Box::new(TestTaskImpl2 {
        a: 2,
        executed: executed.clone(),
    }));
    test_scheduler.PushTask(Box::new(TestTaskImpl2 {
        a: 3,
        executed: executed.clone(),
    }));

    let err = test_scheduler
        .ExecuteTasks()
        .expect_err("second task must fail");
    assert_eq!(err.to_string(), "mock error at task id = 2");
    assert_eq!(*executed.borrow(), vec![3, 2]);
}
