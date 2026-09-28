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

// Workload Repository（负载仓库）快照挂钩的执行器边界。
//
// 对应 `CREATE WORKLOAD REPOSITORY` 一类语句：若运行时已安装快照 hook，
// 则在 `Next` 拉取结果时触发一次快照；否则空操作成功返回。

#![allow(non_snake_case)]

/// Production boundary for the workload-repository snapshot hook.
/// 负载仓库快照挂钩的生产边界：查询 hook 是否安装，并执行快照。
pub trait WorkloadRepoRuntime {
    /// 会话/事务上下文类型。
    type Context;
    /// 快照失败时返回的错误类型。
    type Error;

    /// 是否已安装快照 hook。
    fn snapshot_hook_installed(&self) -> bool;
    /// 对当前上下文执行一次负载仓库快照。
    fn take_snapshot(&mut self, context: &mut Self::Context) -> Result<(), Self::Error>;
}

/// `CREATE WORKLOAD REPOSITORY` 执行器：持有运行时并在 Next 中按需打快照。
pub struct WorkloadRepoCreateExec<R: WorkloadRepoRuntime> {
    /// 注入的负载仓库运行时实现。
    pub runtime: R,
}

impl<R: WorkloadRepoRuntime> WorkloadRepoCreateExec<R> {
    /// 执行一步：若 hook 已安装则调用 `take_snapshot`，否则直接成功。
    pub fn Next<T>(&mut self, context: &mut R::Context, _request: &mut T) -> Result<(), R::Error> {
        // 未安装 hook 时创建语句为空操作，避免误触发快照 IO。
        if self.runtime.snapshot_hook_installed() {
            self.runtime.take_snapshot(context)
        } else {
            Ok(())
        }
    }
}
