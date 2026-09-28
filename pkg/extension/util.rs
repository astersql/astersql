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

// 扩展框架通用工具：错误类型、上下文 trait，以及清理回调构建器。
//
// `clearFuncBuilder` 在注册系统变量/权限/函数时收集反向清理闭包，
// 失败或部分成功时按收集顺序回滚，避免资源泄漏。

use std::fmt;

/// 扩展框架统一错误，持有可读消息字符串。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExtensionError(String);

impl ExtensionError {
    /// 由任意可转为 String 的消息构造错误。
    pub fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for ExtensionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for ExtensionError {}

/// 扩展执行上下文基 trait；默认认为未被取消。
pub trait ExtensionContext {
    /// 查询当前操作是否已取消；默认恒为 false。
    fn is_cancelled(&self) -> bool {
        false
    }
}

/// 一次性清理回调：注册失败回滚或进程关闭时调用。
pub type ClearFunc = Box<dyn FnOnce() + Send + Sync + 'static>;

/// 收集多个 `ClearFunc`，最终拼成一个按序执行的清理函数。
#[derive(Default)]
pub struct clearFuncBuilder {
    clears: Vec<ClearFunc>,
}

impl clearFuncBuilder {
    /// 执行 `function`；若返回清理闭包则加入列表。出错时中止收集。
    pub fn DoWithCollectClear<F>(&mut self, function: F) -> Result<(), ExtensionError>
    where
        F: FnOnce() -> Result<Option<ClearFunc>, ExtensionError>,
    {
        if let Some(clear) = function()? {
            self.clears.push(clear);
        }
        Ok(())
    }

    /// 消费自身，生成按收集顺序依次调用各清理回调的 `ClearFunc`。
    pub fn Build(self) -> ClearFunc {
        Box::new(move || {
            // Go iterates the collected slice from first to last.
            // 与 Go 一致：按收集先后顺序执行清理，而非 LIFO。
            for clear in self.clears {
                clear();
            }
        })
    }
}
