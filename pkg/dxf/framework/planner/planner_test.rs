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

// Planner 单元测试。
//
// 用可记录调用参数的假 TaskCreator，验证 `run_with_target_scope` 会先拿到
// 逻辑计划 meta，再把 key/type/slots/scope/ExtraParams 原样转发给创建边界。
// `_GO_PLANNER_TEST_REFERENCE` 保留 Go 原测试形状供对照。

/// Go 侧 Planner 测试草稿（含 mock store / session），不参与编译执行。
const _GO_PLANNER_TEST_REFERENCE: &str = r###"
#[test]
fn test_planner() {
    // Go 测试用 gomock.NewController 管理 mock 生命周期，并在 defer ctrl.Finish() 中校验 EXPECT 是否全部命中。
    let ctrl = gomock::NewController(testing::T);
    defer!(ctrl.Finish());

    // 原测试把 context 标记为 InternalDistTask，确保创建任务时走分布式任务内部来源。
    let mut ctx = context::Background();
    ctx = util::WithInternalSourceType(ctx, kv::InternalDistTask);

    // testkit.CreateMockStore/NewTestKit 提供内存 store 和 session；只保留 fixture 连接形状。
    let store = testkit::CreateMockStore(testing::T);
    let gtk = testkit::NewTestKit(testing::T, store);
    let pool = pools::NewResourcePool(
        || -> Result<pools::Resource, errors::Error> { Ok(gtk.Session()) },
        1,
        1,
        time::Second,
    );
    // pool.Close 对应 Go 的 defer 收尾，避免测试结束后泄漏 session resource。
    defer!(pool.Close());

    let mgr = storage::NewTaskManager(pool);
    storage::SetTaskManager(mgr.clone());
    let p = planner::Planner {};
    let p_ctx = planner::PlanCtx {
        Ctx: ctx,
        SessionCtx: gtk.Session(),
        TaskKey: "1".to_string(),
        TaskType: "example",
        ThreadCnt: 1,
        ..Default::default()
    };

    // mock LogicalPlan 同时提供任务 meta 和 ExtraParams；这里是 Planner.Run 的核心输入。
    let mock_logical_plan = mock::NewMockLogicalPlan(ctrl);
    mock_logical_plan
        .EXPECT()
        .ToTaskMeta()
        .Return(Vec::from("mock"), None);
    mock_logical_plan
        .EXPECT()
        .GetTaskExtraParams()
        .Return(proto::ExtraParams {
            ManualRecovery: true,
            ..Default::default()
        });

    let (task_id, err) = p.Run(p_ctx, mock_logical_plan, &mgr);
    require::NoError(testing::T, err);
    let (task, err) = mgr.GetTaskByID(ctx, task_id);
    require::NoError(testing::T, err);
    require::EqualValues(testing::T, 1, task.RequiredSlots);
    require::EqualValues(testing::T, "example", task.Type);
    require::True(testing::T, task.ExtraParams.ManualRecovery);
}
"###;

use std::sync::Mutex;

use crate::planner::to_storage_extra_params;
use crate::{LogicalPlan, PhysicalPlan, PlanCtx, TaskCreator, new_planner};
use astersql_dxf_framework_proto as proto;
use astersql_dxf_framework_storage as storage;

/// 测试用 LogicalPlan：固定 ExtraParams 与 meta 字节。
struct Plan;
impl LogicalPlan for Plan {
    fn get_task_extra_params(&self) -> proto::ExtraParams {
        proto::ExtraParams {
            ManualRecovery: true,
            PauseOnKVDiskFull: true,
            MaxRuntimeSlots: 4,
            TargetSteps: vec![proto::StepOne],
            PrepareMode: proto::PrepareModeRequired,
        }
    }
    fn to_task_meta(&self) -> Result<Vec<u8>, storage::Error> {
        Ok(b"meta".to_vec())
    }
    fn from_task_meta(&mut self, _meta: &[u8]) -> Result<(), storage::Error> {
        Ok(())
    }
    fn to_physical_plan(&self, _context: PlanCtx) -> Result<PhysicalPlan, storage::Error> {
        Ok(PhysicalPlan::default())
    }
}

#[derive(Debug, Eq, PartialEq)]
/// 捕获一次 create_task_with_session 的关键入参，便于断言。
struct Call {
    key: String,
    task_type: proto::TaskType,
    keyspace: String,
    required_slots: i32,
    target_scope: String,
    max_node_count: i32,
    manual_recovery: bool,
    pause_on_kv_disk_full: bool,
    max_runtime_slots: i32,
    target_steps: Vec<proto::Step>,
    prepare_mode: proto::PrepareMode,
    meta: Vec<u8>,
}

#[derive(Default)]
/// 假 TaskCreator：记录调用后返回固定任务 ID 88。
struct Creator(Mutex<Option<Call>>);
impl TaskCreator for Creator {
    fn create_task_with_session(
        &self,
        _context: storage::Context,
        _session: storage::sessionctx::Context,
        key: String,
        task_type: proto::TaskType,
        keyspace: String,
        required_slots: i32,
        target_scope: String,
        max_node_count: i32,
        extra_params: proto::ExtraParams,
        meta: Vec<u8>,
    ) -> Result<i64, storage::Error> {
        *self.0.lock().unwrap() = Some(Call {
            key,
            task_type,
            keyspace,
            required_slots,
            target_scope,
            max_node_count,
            manual_recovery: extra_params.ManualRecovery,
            pause_on_kv_disk_full: extra_params.PauseOnKVDiskFull,
            max_runtime_slots: extra_params.MaxRuntimeSlots,
            target_steps: extra_params.TargetSteps,
            prepare_mode: extra_params.PrepareMode,
            meta,
        });
        Ok(88)
    }
}

#[test]
/// 校验 meta 序列化与创建字段转发（含 ManualRecovery、required_slots）。
fn planner_serializes_meta_and_forwards_creation_fields() {
    let creator = Creator::default();
    let id = new_planner()
        .run_with_target_scope(
            PlanCtx {
                task_key: "task-key".into(),
                task_type: proto::TaskTypeExample,
                thread_count: 6,
                keyspace: "keyspace-1".into(),
                max_node_count: 3,
                ..Default::default()
            },
            &Plan,
            &creator,
            "background".into(),
        )
        .unwrap();
    assert_eq!(id, 88);
    assert_eq!(
        creator.0.lock().unwrap().take().unwrap(),
        Call {
            key: "task-key".into(),
            task_type: proto::TaskTypeExample,
            keyspace: "keyspace-1".into(),
            required_slots: 6,
            target_scope: "background".into(),
            max_node_count: 3,
            manual_recovery: true,
            pause_on_kv_disk_full: true,
            max_runtime_slots: 4,
            target_steps: vec![proto::StepOne],
            prepare_mode: proto::PrepareModeRequired,
            meta: b"meta".to_vec(),
        }
    );
}

/// `ToTaskMeta` 失败时，Go Planner 会立即返回，且不会读取额外参数或创建任务。
struct FailingPlan;
impl LogicalPlan for FailingPlan {
    fn get_task_extra_params(&self) -> proto::ExtraParams {
        panic!("extra params must not be read after serialization failure")
    }

    fn to_task_meta(&self) -> Result<Vec<u8>, storage::Error> {
        Err(storage::Error::new("encode failed"))
    }

    fn from_task_meta(&mut self, _meta: &[u8]) -> Result<(), storage::Error> {
        Ok(())
    }

    fn to_physical_plan(&self, _context: PlanCtx) -> Result<PhysicalPlan, storage::Error> {
        Ok(PhysicalPlan::default())
    }
}

#[test]
fn planner_returns_serialization_error_without_creating_task() {
    let creator = Creator::default();
    let error = new_planner()
        .run_with_target_scope(
            PlanCtx::default(),
            &FailingPlan,
            &creator,
            "background".into(),
        )
        .unwrap_err();

    assert_eq!(error.to_string(), "encode failed");
    assert!(creator.0.lock().unwrap().is_none());
}

#[test]
/// 存储适配必须保留 Go ExtraParams 的全部字段，不能静默丢弃新字段。
fn storage_extra_params_preserve_all_fields() {
    let converted = to_storage_extra_params(proto::ExtraParams {
        ManualRecovery: true,
        PauseOnKVDiskFull: true,
        MaxRuntimeSlots: 4,
        TargetSteps: vec![proto::StepOne],
        PrepareMode: proto::PrepareModeRequired,
    });

    assert_eq!(converted.ManualRecovery, true);
    assert_eq!(converted.PauseOnKVDiskFull, true);
    assert_eq!(converted.MaxRuntimeSlots, 4);
    assert_eq!(converted.TargetSteps, vec![proto::StepOne]);
    assert_eq!(converted.PrepareMode, proto::PrepareModeRequired);
}
