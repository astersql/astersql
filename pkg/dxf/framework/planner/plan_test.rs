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

// PhysicalPlan 单元测试。
//
// 验证按 step 过滤处理器、保持插入顺序生成 subtask meta，并对照 Go 侧
// TestPhysicalPlan 的行为（参考字符串常量 `_GO_PLAN_TEST_REFERENCE`）。

// TestPhysicalPlan 对应 Go 测试：添加一个 processor，并确认指定 step 能调用 pipeline 生成 subtask meta。
/// 保留的 Go 原测试草稿，便于对照迁移语义，不参与编译执行。
const _GO_PLAN_TEST_REFERENCE: &str = r###"
#[test]
pub fn test_physical_plan() {
    let ctrl = gomock::NewController(testing::T);
    defer! {
        // Go defer ctrl.Finish() 用于校验 mock 期望；只保留资源收尾位置。
        ctrl.Finish();
    }
    let mock_pipeline_spec = mock::NewMockPipelineSpec(ctrl);

    let mut plan = planner::PhysicalPlan::default();
    let plan_ctx = planner::PlanCtx::default();
    plan.AddProcessor(planner::ProcessorSpec {
        Pipeline: mock_pipeline_spec,
        Step: 1,
        ..Default::default()
    });
    mock_pipeline_spec
        .EXPECT()
        .ToSubtaskMeta(gomock::Any())
        .Return(vec![b"mock".to_vec()], None);
    let (subtask_metas, err) = plan.ToSubtaskMetas(plan_ctx, 1);
    require::NoError(err);
    require::Equal(vec![b"mock".to_vec()], subtask_metas);
}
"###;

use crate::{PhysicalPlan, PipelineSpec, PlanCtx, ProcessorSpec};

/// 测试用 PipelineSpec：meta = 固定前缀 + PlanCtx.task_key。
struct Pipeline(&'static [u8]);
impl PipelineSpec for Pipeline {
    fn to_subtask_meta(
        &self,
        context: PlanCtx,
    ) -> Result<Vec<u8>, astersql_dxf_framework_storage::Error> {
        let mut result = self.0.to_vec();
        result.extend_from_slice(context.task_key.as_bytes());
        Ok(result)
    }
}

#[test]
/// 对应 Go TestPhysicalPlan：step 匹配时调用 pipeline 并返回其 meta。
fn physical_plan_calls_matching_pipeline_and_returns_meta() {
    let mut plan = PhysicalPlan::default();
    plan.add_processor(ProcessorSpec::new(1, 1, Box::new(Pipeline(b"mock"))));

    let metas = plan.to_subtask_metas(PlanCtx::default(), 1).unwrap();

    assert_eq!(metas, vec![b"mock".to_vec()]);
}

#[test]
/// 仅导出 step==1 的处理器 meta，且顺序为插入序 a、b（跳过 step==2）。
fn physical_plan_filters_step_and_preserves_processor_order() {
    let mut plan = PhysicalPlan::default();
    // 构造三个处理器：两个 step=1，一个 step=2，验证过滤与顺序。
    plan.add_processor(ProcessorSpec::new(1, 1, Box::new(Pipeline(b"a"))));
    plan.add_processor(ProcessorSpec::new(2, 2, Box::new(Pipeline(b"x"))));
    plan.add_processor(ProcessorSpec::new(3, 1, Box::new(Pipeline(b"b"))));
    let metas = plan
        .to_subtask_metas(
            PlanCtx {
                task_key: "k".into(),
                ..Default::default()
            },
            1,
        )
        .unwrap();
    assert_eq!(metas, vec![b"ak".to_vec(), b"bk".to_vec()]);
    assert_eq!(plan.processors().len(), 3);
}
