// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
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

//! MockGen port of `br/pkg/mock/task_register.go` (TaskRegister).
//! gomock 风格 TaskRegister 替身：EXPECT/RecordCall 驱动行为。
//! 用于注册/关闭任务路径的单测，不连接真实任务注册表。

use crate::stubs::{Call, Context, Controller, Result, take_error};

/// MockTaskRegister is a mock of TaskRegister interface.
/// 持有 Controller 与 recorder；方法调用转发到 ctrl.Call。
pub struct MockTaskRegister {
    pub ctrl: Controller,
    pub recorder: MockTaskRegisterMockRecorder,
}

/// MockTaskRegisterMockRecorder is the mock recorder for MockTaskRegister.
/// 记录期望调用，供后续 ASSERT。
pub struct MockTaskRegisterMockRecorder {
    ctrl: Controller,
}

/// NewMockTaskRegister creates a new mock instance.
/// recorder 共享同一 Controller 克隆。
pub fn NewMockTaskRegister(ctrl: Controller) -> MockTaskRegister {
    MockTaskRegister {
        recorder: MockTaskRegisterMockRecorder { ctrl: ctrl.clone() },
        ctrl,
    }
}

impl MockTaskRegister {
    /// EXPECT returns an object that allows the caller to indicate expected use.
    /// 返回 recorder 以链式声明期望。
    pub fn EXPECT(&self) -> &MockTaskRegisterMockRecorder {
        &self.recorder
    }

    /// ISGOMOCK indicates that this struct is a gomock mock.
    /// 标记符号，供测试框架识别 gomock 实例。
    pub fn ISGOMOCK(&self) {}

    /// Close mocks base method.
    /// 关闭注册；错误经 take_error 还原。
    pub fn Close(&self, arg0: Context) -> Result<()> {
        self.ctrl.Helper();
        take_error(self.ctrl.Call("Close", vec![Box::new(arg0)]))
    }

    /// RegisterTask mocks base method.
    /// 注册长驻任务。
    pub fn RegisterTask(&self, arg0: Context) -> Result<()> {
        self.ctrl.Helper();
        take_error(self.ctrl.Call("RegisterTask", vec![Box::new(arg0)]))
    }

    /// RegisterTaskOnce mocks base method.
    /// 一次性注册语义，与 Go 接口同名。
    pub fn RegisterTaskOnce(&self, arg0: Context) -> Result<()> {
        self.ctrl.Helper();
        take_error(self.ctrl.Call("RegisterTaskOnce", vec![Box::new(arg0)]))
    }
}

impl MockTaskRegisterMockRecorder {
    /// Close indicates an expected call of Close.
    /// 记录 Close 期望；参数类型擦除为 Any。
    pub fn Close(&self, _arg0: &dyn std::any::Any) -> Call {
        self.ctrl.Helper();
        self.ctrl
            .RecordCallWithMethodType("Close", "MockTaskRegister.Close", vec![])
    }

    /// RegisterTask indicates an expected call of RegisterTask.
    /// 记录 RegisterTask 期望调用。
    pub fn RegisterTask(&self, _arg0: &dyn std::any::Any) -> Call {
        self.ctrl.Helper();
        self.ctrl
            .RecordCallWithMethodType("RegisterTask", "MockTaskRegister.RegisterTask", vec![])
    }

    /// RegisterTaskOnce indicates an expected call of RegisterTaskOnce.
    /// 记录 RegisterTaskOnce 期望调用。
    pub fn RegisterTaskOnce(&self, _arg0: &dyn std::any::Any) -> Call {
        self.ctrl.Helper();
        self.ctrl.RecordCallWithMethodType(
            "RegisterTaskOnce",
            "MockTaskRegister.RegisterTaskOnce",
            vec![],
        )
    }
}
