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

// SchemaLoader 的 GoMock 风格可控替身。
//
// SchemaLoader 负责在 DDL 后触发 Schema（库表元数据）重载，使会话看到
// 最新 InfoSchema。本文件提供无序匹配未满足调用、校验参数并返回预设
// 结果的 `Controller`，以及仅暴露 `Reload` 的 `MockSchemaLoader`。

use std::fmt;
use std::sync::{Arc, Mutex};

/// Mock 调用参数的类型化取值。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Argument {
    /// 文本参数。
    Text(String),
    /// 整型参数。
    Int(i64),
    /// Opaque production session argument.
    Session,
}

/// 期望参数的匹配器：任意值或精确相等。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Matcher {
    /// 接受任意实参。
    Any,
    /// 要求与给定 `Argument` 完全一致。
    Exact(Argument),
}

/// Mock 方法的预设返回值。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ReturnValue {
    /// 无返回值（对应 Go 的 nil error 成功）。
    Unit,
    /// 序列化后的 Job 字节。
    Job(Vec<u8>),
    /// 原始字节切片。
    Bytes(Vec<u8>),
    /// 整型结果。
    Int(i64),
    /// 布尔结果。
    Bool(bool),
    /// A production system-table error returned by a Manager expectation.
    ManagerError(ddl_systable::Error),
}

/// Mock 调用失败时的错误包装。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MockError(pub String);

impl fmt::Display for MockError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for MockError {}

/// 一条预先录制的期望调用：方法名、参数匹配器与返回结果。
#[derive(Clone, Debug)]
struct ExpectedCall {
    method: &'static str,
    arguments: Vec<Matcher>,
    result: Result<ReturnValue, MockError>,
}

/// Controller 内部状态：待消费期望队列与已发生调用记录。
#[derive(Default)]
struct ControllerState {
    expected: Vec<ExpectedCall>,
    calls: Vec<(&'static str, Vec<Argument>)>,
}

/// Small typed controller with GoMock's default matching and error semantics.
/// Calls search all unsatisfied expectations and argument mismatches fail the
/// invocation rather than silently returning defaults.
///
/// 小型类型化调用控制器：默认在全部未满足期望中搜索；方法名或参数不匹配时
/// 直接报错，而不是静默返回默认值（对齐 GoMock 默认语义）。
#[derive(Clone, Default)]
pub struct Controller {
    state: Arc<Mutex<ControllerState>>,
}

impl Controller {
    /// 录制一条期望调用及其返回值。
    pub fn record(
        &self,
        method: &'static str,
        arguments: Vec<Matcher>,
        result: Result<ReturnValue, MockError>,
    ) {
        self.state
            .lock()
            .expect("mock controller mutex poisoned")
            .expected
            .push(ExpectedCall {
                method,
                arguments,
                result,
            });
    }

    /// Execute one call by finding any matching unsatisfied expectation.
    ///
    /// GoMock is unordered by default; ordering is only added explicitly with
    /// `InOrder`/`After`.
    pub fn call(
        &self,
        method: &'static str,
        arguments: Vec<Argument>,
    ) -> Result<ReturnValue, MockError> {
        let mut state = self.state.lock().expect("mock controller mutex poisoned");
        state.calls.push((method, arguments.clone()));
        let position = state
            .expected
            .iter()
            .position(|expected| {
                expected.method == method
                    && expected.arguments.len() == arguments.len()
                    && expected
                        .arguments
                        .iter()
                        .zip(&arguments)
                        .all(|(matcher, actual)| match matcher {
                            Matcher::Any => true,
                            Matcher::Exact(expected) => expected == actual,
                        })
            })
            .ok_or_else(|| MockError(format!("unexpected call to {method} with {arguments:?}")))?;
        let expected = state.expected.remove(position);
        expected.result
    }

    /// 断言所有已录制期望均已被消费；否则返回剩余次数错误。
    pub fn verify(&self) -> Result<(), MockError> {
        let state = self.state.lock().expect("mock controller mutex poisoned");
        if state.expected.is_empty() {
            Ok(())
        } else {
            Err(MockError(format!(
                "{} expected calls were not made",
                state.expected.len()
            )))
        }
    }
}

pub use ddl_systable::{SchemaLoader, SchemaLoaderError};

/// 由 `Controller` 驱动的 `SchemaLoader` Mock。
pub struct MockSchemaLoader {
    controller: Controller,
    recorder: MockSchemaLoaderRecorder,
}

/// 录制 `MockSchemaLoader` 期望调用的辅助对象。
#[derive(Clone)]
pub struct MockSchemaLoaderRecorder {
    controller: Controller,
}

/// 用给定控制器构造 Mock SchemaLoader。
pub fn new_mock_schema_loader(controller: Controller) -> MockSchemaLoader {
    MockSchemaLoader {
        recorder: MockSchemaLoaderRecorder {
            controller: controller.clone(),
        },
        controller,
    }
}

impl MockSchemaLoader {
    /// 返回用于录制期望的 recorder。
    pub fn expect(&self) -> &MockSchemaLoaderRecorder {
        &self.recorder
    }

    /// 类型标记方法：表明实现为 Mock（Go 侧常见约定）。
    pub fn is_mock(&self) {}
}

impl ddl_systable::SchemaLoader for MockSchemaLoader {
    fn reload(&self) -> Result<(), ddl_systable::SchemaLoaderError> {
        // Reload 无参数；返回值必须是 Unit。
        match self
            .controller
            .call("Reload", Vec::new())
            .map_err(|error| ddl_systable::SchemaLoaderError::new(error.0))?
        {
            ReturnValue::Unit => Ok(()),
            _ => Err(ddl_systable::SchemaLoaderError::new(
                "Reload returned the wrong type",
            )),
        }
    }
}

impl MockSchemaLoaderRecorder {
    /// 录制一次 `Reload` 调用的期望结果。
    pub fn reload(&self, result: Result<(), MockError>) {
        self.controller
            .record("Reload", Vec::new(), result.map(|()| ReturnValue::Unit));
    }
}
