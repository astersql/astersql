// Copyright 2026 AsterSQL.
// Copyright 2019 PingCAP, Inc.
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

// `ADMIN PLUGINS` 语句执行器。
//
// 对应 Go 的 `AdminPluginsExec`：按动作对插件启用/禁用标志做变更并刷盘
//（`ChangeDisableFlagAndFlush`）。本文件用 `PluginFlagFlusher` 抽象领域层操作，
// 使执行器迭代路径与插件加载器的具体同步策略解耦。

/// ADMIN PLUGINS 的动作枚举：启用或禁用指定插件。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(i32)]
pub enum AdminPluginsAction {
    /// 清除禁用标志，使插件生效。
    Enable = 1,
    /// 设置禁用标志，使插件失效。
    Disable = 2,
    /// 保留 Go `AdminPluginsAction int` 可承载的其它值；执行时保持 no-op。
    Unknown(i32),
}

/// Production boundary for the domain/plugin operation used by ADMIN PLUGINS.
///
/// The Go implementation obtains the domain from the executor context and then
/// calls `plugin.ChangeDisableFlagAndFlush`. Keeping that operation behind this
/// trait preserves the same fail-fast ordering without coupling the executor
/// iterator to the plugin loader's concrete synchronization strategy.
///
/// 领域/插件操作的生产边界：对应 Go 中从执行器上下文取 domain 后调用
/// `plugin.ChangeDisableFlagAndFlush`。
pub trait PluginFlagFlusher {
    /// 操作失败时返回的错误类型。
    type Error;

    /// 修改插件禁用标志并刷盘（flush 到持久化存储）。
    fn change_disable_flag_and_flush(
        &mut self,
        plugin_name: &str,
        disabled: bool,
    ) -> Result<(), Self::Error>;
}

/// Executor state for `ADMIN PLUGINS`.
///
/// `B` retains the embedded base-executor state and `F` owns the live domain
/// operation. Both are explicit so construction cannot silently fall back to a
/// disconnected mock.
///
/// `ADMIN PLUGINS` 执行器状态：`B` 为基类执行器嵌入状态，`F` 持有领域刷盘操作。
pub struct AdminPluginsExec<B, F> {
    /// 基类执行器状态（BaseExecutor）。
    pub BaseExecutor: B,
    /// 启用或禁用动作。
    pub Action: AdminPluginsAction,
    /// 目标插件名列表。
    pub Plugins: Vec<String>,
    /// 领域侧标志变更与刷盘实现。
    pub Flusher: F,
}

impl<B, F: PluginFlagFlusher> AdminPluginsExec<B, F> {
    /// Executes one ADMIN PLUGINS iterator step.
    ///
    /// The statement produces no rows. Enable clears the disabled flag and
    /// Disable sets it; an unknown action remains the Go no-op success path.
    ///
    /// 执行一步 ADMIN PLUGINS：不产出行；Enable 清除禁用标志，Disable 设置之。
    pub fn Next<C, Q>(&mut self, _context: C, _chunk: &mut Q) -> Result<(), F::Error> {
        match self.Action {
            AdminPluginsAction::Enable => self.changeDisableFlagAndFlush(false),
            AdminPluginsAction::Disable => self.changeDisableFlagAndFlush(true),
            AdminPluginsAction::Unknown(_) => Ok(()),
        }
    }

    /// 对 Plugins 列表中每个插件调用 Flusher 变更禁用标志。
    fn changeDisableFlagAndFlush(&mut self, disabled: bool) -> Result<(), F::Error> {
        for plugin_name in &self.Plugins {
            self.Flusher
                .change_disable_flag_and_flush(plugin_name, disabled)?;
        }
        Ok(())
    }
}
