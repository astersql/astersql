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

// breakpoint 迁移回归测试。
//
// 对应 Go 侧断点注入行为：校验通知键名、failpoint 未启用时不读会话、
// 启用后回调注入名，以及会话中存放非回调类型时的容错。

use std::any::Any;
use std::cell::Cell;
use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, Mutex};

use crate::contextutil::context::ValueStoreContext;
use crate::{BreakPointNotifyFunc, Inject, NotifyBreakPointFuncKey};

/// 串行化 failpoint 全局配置，避免并行测试互相干扰。
static FAILPOINT_TEST_LOCK: Mutex<()> = Mutex::new(());

/// 模拟会话上下文：用哈希表存键值，并统计 `Value` 读取次数。
#[derive(Default)]
struct MockSessionContext {
    /// 键到任意类型值的映射。
    values: HashMap<String, Box<dyn Any>>,
    /// `Value` 被调用的次数，用于断言是否访问了会话。
    value_reads: Cell<usize>,
}

impl ValueStoreContext for MockSessionContext {
    fn SetValue(&mut self, key: &dyn fmt::Display, value: Box<dyn Any>) {
        self.values.insert(key.to_string(), value);
    }

    fn Value(&self, key: &dyn fmt::Display) -> Option<&dyn Any> {
        // 每次读取都计数，便于断言 Inject 是否触碰会话。
        self.value_reads.set(self.value_reads.get() + 1);
        self.values.get(&key.to_string()).map(Box::as_ref)
    }

    fn ClearValue(&mut self, key: &dyn fmt::Display) {
        self.values.remove(&key.to_string());
    }

    fn GetDomain(&self) -> Option<&dyn Any> {
        None
    }
}

/// 通知键的 `String()` 结果须与 Go Stringer 一致。
#[test]
fn notify_key_matches_the_go_stringer_value() {
    assert_eq!(NotifyBreakPointFuncKey.String(), "breakPointNotifyFunc");
}

/// failpoint 未启用时，`Inject` 不应读取会话上下文。
#[test]
fn disabled_failpoint_does_not_read_the_session() {
    // 持锁保证 fail 场景与其它用例互斥。
    let _guard = FAILPOINT_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let _scenario = fail::FailScenario::setup();
    let session = MockSessionContext::default();

    Inject(&session, "breakpoint-disabled");

    assert_eq!(session.value_reads.get(), 0);
}

/// failpoint 启用后，应调用会话中注册的回调，并传入注入点名称。
#[test]
fn enabled_failpoint_notifies_with_the_injected_name() {
    let _guard = FAILPOINT_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let _scenario = fail::FailScenario::setup();
    fail::cfg("breakpoint-enabled", "return").expect("enable failpoint");

    // 通过 Arc 收集回调收到的断点名。
    let notifications = Arc::new(Mutex::new(Vec::new()));
    let captured = Arc::clone(&notifications);
    let callback: BreakPointNotifyFunc = Box::new(move |name| {
        captured.lock().unwrap().push(name);
    });
    let mut session = MockSessionContext::default();
    session.SetValue(&NotifyBreakPointFuncKey.String(), Box::new(callback));

    Inject(&session, "breakpoint-enabled");

    assert_eq!(session.value_reads.get(), 1);
    assert_eq!(
        notifications.lock().unwrap().as_slice(),
        ["breakpoint-enabled"]
    );
}

/// 会话中存放非回调类型时，启用 failpoint 仍可读会话，但不 panic。
#[test]
fn enabled_failpoint_ignores_a_non_callback_value() {
    let _guard = FAILPOINT_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let _scenario = fail::FailScenario::setup();
    fail::cfg("breakpoint-wrong-type", "return").expect("enable failpoint");

    let mut session = MockSessionContext::default();
    // 故意放入 i32，模拟错误类型注册。
    session.SetValue(&NotifyBreakPointFuncKey.String(), Box::new(42_i32));

    Inject(&session, "breakpoint-wrong-type");

    assert_eq!(session.value_reads.get(), 1);
}
