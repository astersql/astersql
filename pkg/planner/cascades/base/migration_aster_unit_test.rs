// Copyright 2026 AsterSQL.
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

// Cascades base 迁移回归测试。
//
// 用固定 FNV-1a 向量、字符串长度边界、缓存 Reset 容量复用，以及
// Stack/Scheduler 桩实现，核对 Rust 迁移与 Go 行为一致。

use crate::base::{Equals, Scheduler, Stack, Task};
use crate::util::StrBufferWriter;
use std::any::Any;
use std::cell::RefCell;
use std::error::Error;
use std::rc::Rc;

/// 测试用 Equals 实现：仅比较内层 i32。
#[derive(Debug)]
struct Value(i32);

impl Equals for Value {
    fn Equals(&self, other: &dyn Any) -> bool {
        other
            .downcast_ref::<Self>()
            .is_some_and(|rhs| self.0 == rhs.0)
    }
}

/// 两侧写入相同混合序列，并断言摘要等于 Go 侧已知金值。
#[test]
fn migration_hashes_match_go_fnv1a_sequence_and_boundaries() {
    let mut first = crate::base::NewHashEqualer();
    let mut second = crate::base::NewHashEqualer();

    // 覆盖 bool/有符号/无符号/浮点（含 -0.0）/rune/UTF-8 串/字节切片。
    first.HashBool(true);
    first.HashBool(false);
    first.HashInt(-199);
    first.HashInt64(-13_534_523_462_346);
    first.HashUint64(13_534_523_462_346);
    first.HashFloat64(-0.0);
    first.HashRune('我' as i32);
    first.HashString("我是谁");
    first.HashBytes(b"world");

    second.HashBool(true);
    second.HashBool(false);
    second.HashInt(-199);
    second.HashInt64(-13_534_523_462_346);
    second.HashUint64(13_534_523_462_346);
    second.HashFloat64(-0.0);
    second.HashRune('我' as i32);
    second.HashString("我是谁");
    second.HashBytes(b"world");

    assert_eq!(first.Sum64(), second.Sum64());
    assert_eq!(first.Sum64(), 8_465_102_021_931_103_247);
}

/// 验证字符串长度前缀、UTF-8 展开路径，以及 Equals 的类型敏感比较。
#[test]
fn migration_string_length_and_struct_type_follow_go_behavior() {
    let mut split = crate::base::NewHashEqualer();
    split.HashString("abc");
    split.HashString("def");
    let mut joined = crate::base::NewHashEqualer();
    joined.HashString("abcdef");
    joined.HashString("");
    assert_ne!(split.Sum64(), joined.Sum64());

    // HashString("我") 应等价于先写 UTF-8 字节长度再写单个 rune。
    let mut utf8 = crate::base::NewHashEqualer();
    utf8.HashString("我");
    let mut explicit = crate::base::NewHashEqualer();
    explicit.HashInt("我".len() as isize);
    explicit.HashRune('我' as i32);
    assert_eq!(utf8.Sum64(), explicit.Sum64());

    assert!(Value(7).Equals(&Value(7)));
    assert!(!Value(7).Equals(&Value(8)));
    assert!(!Value(7).Equals(&"7"));
}

/// Reset 清空摘要并复用 Cache 容量（指针不变），且回到 FNV offset。
#[test]
fn migration_cache_reset_reuses_capacity_and_resets_digest() {
    let mut hasher = crate::base::NewHashEqualer();
    hasher.SetCache(vec![1, 2, 3, 4]);
    let allocation = hasher.Cache().as_ptr();
    hasher.HashByte(9);
    assert_ne!(hasher.Sum64(), 14_695_981_039_346_656_037);

    hasher.Reset();
    assert_eq!(hasher.Sum64(), 14_695_981_039_346_656_037);
    assert!(hasher.Cache().is_empty());
    assert_eq!(hasher.Cache().as_ptr(), allocation);
}

/// 测试用字符串缓冲：实现 StrBufferWriter，把写入累积到 String。
struct Buffer(String);

impl StrBufferWriter for Buffer {
    fn WriteString(&mut self, s: &str) {
        self.0.push_str(s);
    }

    fn Flush(&mut self) {}
}

/// 可计数执行次数的测试任务。
struct TestTask {
    executions: Rc<RefCell<usize>>,
    description: &'static str,
}

impl Task for TestTask {
    fn Execute(&mut self) -> Result<(), Box<dyn Error>> {
        *self.executions.borrow_mut() += 1;
        Ok(())
    }

    fn Desc(&self, writer: &mut dyn StrBufferWriter) {
        writer.WriteString(self.description);
    }
}

/// 基于 Vec 的 Stack 桩：Push/Pop/Empty/Destroy 对应 Go 栈契约。
#[derive(Default)]
struct VecStack(Vec<Box<dyn Task>>);

impl Stack for VecStack {
    fn Push(&mut self, one: Box<dyn Task>) {
        self.0.push(one);
    }

    fn Pop(&mut self) -> Option<Box<dyn Task>> {
        self.0.pop()
    }

    fn Empty(&self) -> bool {
        self.0.is_empty()
    }

    fn Destroy(&mut self) {
        self.0.clear();
    }
}

/// 串行调度器：循环 Pop 并 Execute，直到栈空。
struct SerialScheduler {
    stack: VecStack,
    destroyed: bool,
}

impl Scheduler for SerialScheduler {
    fn ExecuteTasks(&mut self) -> Result<(), Box<dyn Error>> {
        while !self.stack.Empty() {
            self.stack
                .Pop()
                .expect("non-empty stack must pop a task")
                .Execute()?;
        }
        Ok(())
    }

    fn Destroy(&mut self) {
        self.stack.Destroy();
        self.destroyed = true;
    }

    fn PushTask(&mut self, task: Box<dyn Task>) {
        self.stack.Push(task);
    }
}

/// 端到端验证 Desc、PushTask、ExecuteTasks 与 Destroy 契约可用。
#[test]
fn migration_task_stack_and_scheduler_contracts_are_usable() {
    let executions = Rc::new(RefCell::new(0));
    let task = TestTask {
        executions: executions.clone(),
        description: "explore",
    };
    let mut buffer = Buffer(String::new());
    task.Desc(&mut buffer);
    assert_eq!(buffer.0, "explore");

    let mut scheduler = SerialScheduler {
        stack: VecStack::default(),
        destroyed: false,
    };
    scheduler.PushTask(Box::new(task));
    scheduler.ExecuteTasks().unwrap();
    assert_eq!(*executions.borrow(), 1);
    scheduler.Destroy();
    assert!(scheduler.destroyed);
}
