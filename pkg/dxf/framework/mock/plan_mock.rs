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

// 逻辑计划（LogicalPlan）与流水线规格（PipelineSpec）的 mock。
//
// 通过 `Handler` 注入 FromTaskMeta / ToPhysicalPlan 等回调，对齐 GoMock API
//（EXPECT / ISGOMOCK），供规划器（planner）路径单测使用。
// 逻辑计划描述任务如何拆步；物理计划落到具体可执行的子任务元数据。
// limitations under the License.

use astersql_dxf_framework_planner as planner;
use astersql_dxf_framework_proto as proto;

use crate::Handler;

// 规划器结果别名。
type PlannerResult<T> = Result<T, planner::PlannerError>;

/// 逻辑计划 mock：各方法对应一个可配置 Handler。
#[derive(Default)]
pub struct MockLogicalPlan {
    /// 从任务 meta 字节反序列化/填充计划。
    pub FromTaskMeta: Handler<dyn FnMut(Vec<u8>) -> PlannerResult<()> + Send>,
    /// 返回任务额外参数。
    pub GetTaskExtraParams: Handler<dyn FnMut() -> proto::ExtraParams + Send>,
    /// 在给定 PlanCtx 下生成物理计划。
    pub ToPhysicalPlan:
        Handler<dyn FnMut(planner::PlanCtx) -> PlannerResult<planner::PhysicalPlan> + Send>,
    /// 将计划序列化为任务 meta。
    pub ToTaskMeta: Handler<dyn FnMut() -> PlannerResult<Vec<u8>> + Send>,
}

/// 期望记录器类型别名（与 GoMock recorder 命名对齐）。
pub type MockLogicalPlanMockRecorder = MockLogicalPlan;

/// GoMock 风格 API 与方法派发。
impl MockLogicalPlan {
    /// 返回期望记录器（自身）。
    pub fn EXPECT(&mut self) -> &mut MockLogicalPlanMockRecorder {
        self
    }

    /// GoMock 标记占位。
    pub fn ISGOMOCK(&self) {}

    /// 派发 FromTaskMeta 期望。
    pub fn FromTaskMeta(&self, meta: Vec<u8>) -> PlannerResult<()> {
        self.FromTaskMeta
            .invoke("MockLogicalPlan.FromTaskMeta", |handler| handler(meta))
    }

    /// 派发 GetTaskExtraParams 期望。
    pub fn GetTaskExtraParams(&self) -> proto::ExtraParams {
        self.GetTaskExtraParams
            .invoke("MockLogicalPlan.GetTaskExtraParams", |handler| handler())
    }

    /// 派发 ToPhysicalPlan 期望。
    pub fn ToPhysicalPlan(
        &self,
        context: planner::PlanCtx,
    ) -> PlannerResult<planner::PhysicalPlan> {
        self.ToPhysicalPlan
            .invoke("MockLogicalPlan.ToPhysicalPlan", |handler| handler(context))
    }

    /// 派发 ToTaskMeta 期望。
    pub fn ToTaskMeta(&self) -> PlannerResult<Vec<u8>> {
        self.ToTaskMeta
            .invoke("MockLogicalPlan.ToTaskMeta", |handler| handler())
    }
}

/// 实现真实 LogicalPlan trait，转发到 Handler。
impl planner::LogicalPlan for MockLogicalPlan {
    fn get_task_extra_params(&self) -> proto::ExtraParams {
        self.GetTaskExtraParams()
    }

    fn to_task_meta(&self) -> PlannerResult<Vec<u8>> {
        self.ToTaskMeta()
    }

    fn from_task_meta(&mut self, meta: &[u8]) -> PlannerResult<()> {
        self.FromTaskMeta(meta.to_vec())
    }

    fn to_physical_plan(&self, context: planner::PlanCtx) -> PlannerResult<planner::PhysicalPlan> {
        self.ToPhysicalPlan(context)
    }
}

/// 构造空期望的 MockLogicalPlan（忽略 controller，对齐 GoMock 签名）。
pub fn NewMockLogicalPlan<C: ?Sized>(_controller: &C) -> MockLogicalPlan {
    MockLogicalPlan::default()
}

/// 流水线规格 mock：将计划上下文转为子任务 meta。
#[derive(Default)]
pub struct MockPipelineSpec {
    /// 生成子任务元数据字节。
    pub ToSubtaskMeta: Handler<dyn FnMut(planner::PlanCtx) -> PlannerResult<Vec<u8>> + Send>,
}

/// PipelineSpec 期望记录器别名。
pub type MockPipelineSpecMockRecorder = MockPipelineSpec;

/// GoMock 风格 API。
impl MockPipelineSpec {
    /// 返回期望记录器。
    pub fn EXPECT(&mut self) -> &mut MockPipelineSpecMockRecorder {
        self
    }

    /// GoMock 标记占位。
    pub fn ISGOMOCK(&self) {}

    /// 派发 ToSubtaskMeta 期望。
    pub fn ToSubtaskMeta(&self, context: planner::PlanCtx) -> PlannerResult<Vec<u8>> {
        self.ToSubtaskMeta
            .invoke("MockPipelineSpec.ToSubtaskMeta", |handler| handler(context))
    }
}

/// 实现 PipelineSpec trait。
impl planner::PipelineSpec for MockPipelineSpec {
    fn to_subtask_meta(&self, context: planner::PlanCtx) -> PlannerResult<Vec<u8>> {
        self.ToSubtaskMeta(context)
    }
}

/// 构造空期望的 MockPipelineSpec。
pub fn NewMockPipelineSpec<C: ?Sized>(_controller: &C) -> MockPipelineSpec {
    MockPipelineSpec::default()
}
