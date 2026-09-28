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

// `ADMIN SHOW BDR ROLE` 执行器：在新建管理员事务中读取 BDR 角色。
//
// BDR（Bi-Directional Replication，双向复制）角色描述当前实例在复制拓扑中的身份；
// 读取走独立管理员事务与元数据变更边界，避免污染业务会话事务。

#![allow(non_snake_case)]

use astersql_util_chunk::Chunk;

/// New-admin-transaction and meta-mutator boundary.
/// 新建管理员事务与元数据变更边界：重置结果块、读角色、写回一行。
pub trait ShowBdrRoleRuntime {
    type Context;
    type Error;

    /// 清空并重置输出 Chunk（列式结果缓冲）。
    fn reset_chunk(&self, request: &mut Chunk);
    /// 在新开的管理员事务回调中读取角色、追加一行并设置 `done`。
    ///
    /// 这三个副作用必须在事务回调返回前完成，以保持 Go `RunInNewTxn` 在回调成功、
    /// 后续提交失败时仍已写入结果并标记完成的精确语义。
    fn run_in_new_admin_transaction(
        &mut self,
        context: &mut Self::Context,
        request: &mut Chunk,
        done: &mut bool,
    ) -> Result<(), Self::Error>;
}

/// `ADMIN SHOW BDR ROLE` 执行器；`done` 保证只产出一行。
pub struct AdminShowBDRRoleExec<R: ShowBdrRoleRuntime> {
    pub runtime: R,
    pub done: bool,
}

impl<R: ShowBdrRoleRuntime> AdminShowBDRRoleExec<R> {
    /// 拉取下一批评列结果；首次调用读角色并写出，之后返回空块。
    pub fn Next(&mut self, context: &mut R::Context, request: &mut Chunk) -> Result<(), R::Error> {
        self.runtime.reset_chunk(request);
        if self.done {
            return Ok(());
        }
        self.runtime
            .run_in_new_admin_transaction(context, request, &mut self.done)
    }
}
