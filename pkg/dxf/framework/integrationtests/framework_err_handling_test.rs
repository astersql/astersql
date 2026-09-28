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

// DXF 框架任务错误处理集成测试。
//
// 覆盖 scheduler plan 可/不可重试错误、ManualRecovery（人工恢复）进入
// awaiting-resolution 后恢复或取消，以及错误 JSON 往返的 code/message 保真。

// Go `TestOnTaskError` 的 plan-error 分支在本文件直接执行；ManualRecovery
// 的 awaiting-resolution、人工恢复与取消状态机由 scheduler 独立测试执行。

/// 校验 DXF storage Error 的 JSON 序列化往返保留消息、RFCCode 与错误码。
#[test]
fn task_error_round_trip_keeps_pingcap_code_and_message() {
    let original = astersql_dxf_framework_storage::Error::pingcap("boom", "DXF:1", 42);
    let bytes = original.MarshalJSON().unwrap();
    let mut decoded = astersql_dxf_framework_storage::Error::new("");
    decoded.UnmarshalJSON(bytes).unwrap();
    assert_eq!(decoded.to_string(), "boom");
    assert_eq!(decoded.RFCCode(), "DXF:1");
    assert_eq!(decoded.Code(), 42);
}

/// The Go test exercises retryable planning, permanent planning errors, and
/// the one-shot on-done error independently.  Keep those branches executable
/// through the Rust test scheduler rather than only in the archived snippet.
#[test]
fn plan_error_scheduler_preserves_retry_and_permanent_error_branches() {
    use astersql_dxf_framework_testutil::{
        GetPlanErrSchedulerExt, GetPlanNotRetryableErrSchedulerExt,
        GetStepTwoPlanNotRetryableErrSchedulerExt, STEP_INIT, STEP_ONE, STEP_TWO, Task,
        TestContext,
    };
    use std::sync::Arc;

    let retryable = GetPlanErrSchedulerExt(Arc::new(TestContext::default()));
    let mut task = Task::default();
    task.base.step = STEP_INIT;
    assert!(retryable.next_subtasks_batch(&task, STEP_ONE).is_err());
    assert_eq!(
        retryable
            .next_subtasks_batch(&task, STEP_ONE)
            .unwrap()
            .len(),
        3
    );
    task.base.step = STEP_ONE;
    assert_eq!(
        retryable
            .next_subtasks_batch(&task, STEP_TWO)
            .unwrap()
            .len(),
        1
    );
    assert!(retryable.on_done().is_err());
    assert!(retryable.on_done().is_ok());

    let permanent = GetPlanNotRetryableErrSchedulerExt();
    assert!(
        !permanent.is_retryable_error(&astersql_dxf_framework_testutil::DxfError(
            "planned error".into(),
        ))
    );
    assert!(
        permanent
            .next_subtasks_batch(&Task::default(), STEP_ONE)
            .is_err()
    );

    let step_two = GetStepTwoPlanNotRetryableErrSchedulerExt();
    let mut step_one_task = Task::default();
    step_one_task.base.step = STEP_INIT;
    assert_eq!(
        step_two
            .next_subtasks_batch(&step_one_task, STEP_ONE)
            .unwrap()
            .len(),
        10
    );
    step_one_task.base.step = STEP_ONE;
    assert!(
        step_two
            .next_subtasks_batch(&step_one_task, STEP_TWO)
            .is_err()
    );
}
