// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
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

// Planner：从逻辑计划创建分布式任务。
//
// 流程：序列化任务 meta → 解析 target scope（执行范围）→ 通过 TaskCreator
// 调用存储层 CreateTaskWithSession。参数列表刻意对齐 Go 侧签名，保证错误
// 与调用顺序可观测一致。

use crate::plan::{LogicalPlan, PlanCtx, PlannerError};
use astersql_dxf_framework_handle as handle;
use astersql_dxf_framework_proto as proto;
use astersql_dxf_framework_storage as storage;

/// 任务创建边界，供 Planner 注入。
/// 参数列表刻意镜像 `storage.TaskManager.CreateTaskWithSession`。
/// Task creation boundary used by Planner. Its argument list deliberately
/// mirrors storage.TaskManager.CreateTaskWithSession.
pub trait TaskCreator {
    /// 带会话创建任务，返回新任务 ID。
    #[allow(clippy::too_many_arguments)]
    fn create_task_with_session(
        &self,
        context: storage::Context,
        session: storage::sessionctx::Context,
        key: String,
        task_type: proto::TaskType,
        keyspace: String,
        required_slots: i32,
        target_scope: String,
        max_node_count: i32,
        extra_params: proto::ExtraParams,
        meta: Vec<u8>,
    ) -> Result<i64, PlannerError>;
}

pub(crate) fn to_storage_extra_params(
    extra_params: proto::ExtraParams,
) -> storage::proto::ExtraParams {
    storage::proto::ExtraParams {
        ManualRecovery: extra_params.ManualRecovery,
        PauseOnKVDiskFull: extra_params.PauseOnKVDiskFull,
        MaxRuntimeSlots: extra_params.MaxRuntimeSlots,
        TargetSteps: extra_params.TargetSteps,
        PrepareMode: extra_params.PrepareMode,
    }
}

/// 将 proto.ExtraParams 适配到当前 storage 边界并转发创建。
impl TaskCreator for storage::TaskManager {
    fn create_task_with_session(
        &self,
        context: storage::Context,
        session: storage::sessionctx::Context,
        key: String,
        task_type: proto::TaskType,
        keyspace: String,
        required_slots: i32,
        target_scope: String,
        max_node_count: i32,
        extra_params: proto::ExtraParams,
        meta: Vec<u8>,
    ) -> Result<i64, PlannerError> {
        self.CreateTaskWithSession(
            context,
            session,
            key,
            task_type,
            keyspace,
            required_slots,
            target_scope,
            max_node_count,
            to_storage_extra_params(extra_params),
            meta,
        )
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
/// 无状态 Planner；通过 `new_planner` 构造。
pub struct Planner;

/// 构造默认 Planner 实例。
pub fn new_planner() -> Planner {
    Planner
}

impl Planner {
    /// 先序列化任务 meta，再读取 target scope，最后创建任务；
    /// 以保持与 Go 一致的可观测错误与调用顺序。
    /// Serializes task meta before reading target scope, then creates the task;
    /// this preserves Go's observable error and call ordering.
    pub fn run(
        &self,
        context: PlanCtx,
        plan: &dyn LogicalPlan,
        task_manager: &dyn TaskCreator,
    ) -> Result<i64, PlannerError> {
        let task_meta = plan.to_task_meta()?;
        let target_scope = handle::GetTargetScope()?;
        self.create(context, plan, task_manager, target_scope, task_meta)
    }

    /// 调用方已解析 target scope 时的确定性入口；
    /// 单元测试可借此避免依赖全局服务运行时状态。
    /// Deterministic form for callers that already resolved target scope and
    /// for unit tests that must not depend on global server runtime state.
    pub fn run_with_target_scope(
        &self,
        context: PlanCtx,
        plan: &dyn LogicalPlan,
        task_manager: &dyn TaskCreator,
        target_scope: String,
    ) -> Result<i64, PlannerError> {
        let task_meta = plan.to_task_meta()?;
        self.create(context, plan, task_manager, target_scope, task_meta)
    }

    /// 组装 PlanCtx 与 ExtraParams，委托 TaskCreator 真正落库创建任务。
    fn create(
        &self,
        context: PlanCtx,
        plan: &dyn LogicalPlan,
        task_manager: &dyn TaskCreator,
        target_scope: String,
        task_meta: Vec<u8>,
    ) -> Result<i64, PlannerError> {
        task_manager.create_task_with_session(
            context.ctx,
            context.session_ctx,
            context.task_key,
            context.task_type,
            context.keyspace,
            context.thread_count,
            target_scope,
            context.max_node_count,
            plan.get_task_extra_params(),
            task_meta,
        )
    }
}
