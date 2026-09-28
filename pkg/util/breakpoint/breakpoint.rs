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

// 会话断点（breakpoint）注入工具。
//
// 对应 Go `util/breakpoint`：通过 failpoint 在命名点触发，
// 从会话上下文取出回调并通知测试方等待继续，用于分布式/异步流程的可控同步。

#![allow(non_snake_case, non_upper_case_globals)]

use std::sync::LazyLock;

use crate::contextutil::context::ValueStoreContext;
use crate::stringutil::string_util::StringerStr;

// NotifyBreakPointFuncKey is the key where break point notify function located
/// 会话上下文中存放断点通知回调的键（`"breakPointNotifyFunc"`）。
pub static NotifyBreakPointFuncKey: LazyLock<StringerStr> =
    LazyLock::new(|| StringerStr("breakPointNotifyFunc".to_owned()));

/// Owned Rust representation of Go's `func(string)` breakpoint callback.
/// 对应 Go `func(string)` 的断点回调：收到断点名后通知并等待继续。
pub type BreakPointNotifyFunc = Box<dyn Fn(String) + 'static>;

// Inject injects a break point to a session
/// 向会话注入命名断点：failpoint 命中时从上下文取回调并调用。
pub fn Inject<C>(sctx: &C, name: &str)
where
    C: ValueStoreContext + ?Sized,
{
    // failpoint 未启用或未命中时 eval 闭包不执行；命中则查找并调用通知函数。
    let _ = fail::eval(name, |_| {
        let key = NotifyBreakPointFuncKey.String();
        if let Some(breakPointNotifyAndWaitContinue) = sctx
            .Value(&key)
            .and_then(|value| value.downcast_ref::<BreakPointNotifyFunc>())
        {
            breakPointNotifyAndWaitContinue(name.to_owned());
        }
    });
}
