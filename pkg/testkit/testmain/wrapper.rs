// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
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

// 测试主入口包装器：在测试运行器返回后统一应用退出码回调。

use std::cell::RefCell;

/// 测试运行器的最小接口：执行全部测试并返回退出码。
pub trait TestingM {
    /// 运行测试套件，返回进程退出码（0 表示成功）。
    fn run(&self) -> i32;
}

impl<T: TestingM + ?Sized> TestingM for &T {
    fn run(&self) -> i32 {
        (*self).run()
    }
}

/// A test runner that applies a callback to the wrapped runner's exit code.
/// 包装底层运行器：在其退出码上应用回调后再返回。
pub struct WrapTestingM<'a, M> {
    testing_m: M,
    callback: RefCell<Box<dyn FnMut(i32) -> i32 + 'a>>,
}

impl<'a, M> WrapTestingM<'a, M> {
    /// 构造包装器；`callback` 为 None 时使用恒等函数。
    pub fn new(testing_m: M, callback: Option<Box<dyn FnMut(i32) -> i32 + 'a>>) -> Self {
        Self {
            testing_m,
            // 未提供回调则保持退出码不变，与 Go 侧可选钩子一致。
            callback: RefCell::new(callback.unwrap_or_else(|| Box::new(|exit_code| exit_code))),
        }
    }
}

impl<M: TestingM> TestingM for WrapTestingM<'_, M> {
    fn run(&self) -> i32 {
        (self.callback.borrow_mut())(self.testing_m.run())
    }
}

/// Returns a `TestingM` wrapped with a callback on the value returned by `run`.
/// Go 风格工厂函数：等价于 `WrapTestingM::new`。
#[allow(non_snake_case)]
pub fn WrapTestingM<'a, M: TestingM>(
    testing_m: M,
    callback: Option<Box<dyn FnMut(i32) -> i32 + 'a>>,
) -> WrapTestingM<'a, M> {
    WrapTestingM::new(testing_m, callback)
}
