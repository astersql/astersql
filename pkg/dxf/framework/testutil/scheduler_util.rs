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

// Mock 调度器扩展（scheduler extension）构造工具。
//
// 调度器负责按步骤生成下一批子任务元数据、判断错误可否重试，
// 以及任务结束时的 on_done 回调。测试通过预设 StepInfo 脚本化这些行为。

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use crate::context::{DxfError, STEP_DONE, STEP_INIT, STEP_ONE, STEP_TWO, Step, Task, TestContext};

#[derive(Clone, Debug)]
/// 构造 mock 调度器的配置。
pub struct SchedulerInfo {
    /// 规划错误是否一律可重试。
    pub all_error_retryable: bool,
    /// 各步骤的脚本信息（按推进顺序）。
    pub step_infos: Vec<StepInfo>,
}

#[derive(Clone, Debug)]
/// 单个步骤的规划行为脚本。
pub struct StepInfo {
    /// 步骤编号。
    pub step: Step,
    /// 规划时要返回的错误（若有）。
    pub error: Option<DxfError>,
    /// 连续返回错误的次数。
    pub error_repeat_count: i64,
    /// 成功规划时生成的子任务数量。
    pub subtask_count: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 任务元数据修改描述（测试用序列化）。
pub struct Modification {
    /// 修改类型名。
    pub modification_type: String,
    /// 目标值。
    pub to: i64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// 调度器内部模式。
enum SchedulerMode {
    /// 按 StepInfo 脚本规划。
    Normal,
    /// 首次 INIT 规划失败一次，随后正常；on_done 首次失败。
    RetryOnceThenOnDoneError,
}

#[derive(Clone)]
/// 可注入的调度器扩展实现。
pub struct SchedulerExtension {
    /// 错误是否可重试。
    retryable: bool,
    /// 步骤转移表：当前 step → 下一 step。
    transitions: Arc<HashMap<Step, Step>>,
    /// 按目标 step 查规划脚本。
    step_infos: Arc<HashMap<Step, StepInfo>>,
    /// 各 step 已被规划调用的次数（用于错误重复）。
    call_counts: Arc<Mutex<HashMap<Step, i64>>>,
    /// 运行模式。
    mode: SchedulerMode,
    /// 计划错误模式下共享的测试上下文。
    test_context: Option<Arc<TestContext>>,
    /// on_done 是否已失败过一次。
    on_done_failed: Arc<AtomicBool>,
}

impl SchedulerExtension {
    /// 调度节拍回调（空实现）。
    pub fn on_tick(&self) {}

    /// 可选执行实例列表（测试返回空）。
    pub fn eligible_instances(&self, _task: &Task) -> Result<Vec<String>, DxfError> {
        Ok(Vec::new())
    }

    /// 错误可重试判定。
    pub fn is_retryable_error(&self, _error: &DxfError) -> bool {
        self.retryable
    }

    /// 查转移表得到下一 step，缺省为 Go map 零值 step。
    pub fn next_step(&self, current: Step) -> Step {
        self.transitions.get(&current).copied().unwrap_or(Step(0))
    }

    /// 为下一 step 规划一批子任务元数据（每项为 `Vec<u8>`）。
    pub fn next_subtasks_batch(
        &self,
        task: &Task,
        next_step: Step,
    ) -> Result<Vec<Vec<u8>>, DxfError> {
        // 特殊模式：INIT 首次返回可重试错，之后生成固定子任务；ONE 生成 task4。
        if self.mode == SchedulerMode::RetryOnceThenOnDoneError {
            if task.base.step == STEP_INIT {
                let context = self.test_context.as_ref().expect("plan-error context");
                if context.next_call_time() == 0 {
                    return Err(DxfError("retryable err".into()));
                }
                return Ok(["task1", "task2", "task3"]
                    .into_iter()
                    .map(|meta| meta.as_bytes().to_vec())
                    .collect());
            }
            if task.base.step == STEP_ONE {
                return Ok(vec![b"task4".to_vec()]);
            }
            return Ok(Vec::new());
        }

        // 常规模式：按 step_info 在 error_repeat_count 次内返回规划错误。
        let Some(step_info) = self.step_infos.get(&next_step) else {
            return Ok(Vec::new());
        };
        let mut call_counts = self.call_counts.lock().unwrap();
        let call_count = call_counts.entry(next_step).or_default();
        if *call_count < step_info.error_repeat_count {
            *call_count += 1;
            return Err(step_info
                .error
                .clone()
                .unwrap_or_else(|| DxfError("planned scheduler error".into())));
        }
        Ok((0..step_info.subtask_count)
            .map(|index| format!("subtask-{index}").into_bytes())
            .collect())
    }

    /// 将修改列表编码为 `type=to,...` 字节串。
    pub fn modify_meta(&self, modifications: &[Modification]) -> Vec<u8> {
        modifications
            .iter()
            .map(|modification| format!("{}={}", modification.modification_type, modification.to))
            .collect::<Vec<_>>()
            .join(",")
            .into_bytes()
    }

    /// 任务完成回调；特殊模式下首次返回不可重试错误。
    pub fn on_done(&self) -> Result<(), DxfError> {
        if self.mode == SchedulerMode::RetryOnceThenOnDoneError
            && !self.on_done_failed.swap(true, Ordering::SeqCst)
        {
            return Err(DxfError("not retryable err".into()));
        }
        Ok(())
    }
}

#[allow(non_snake_case)]
/// 根据 SchedulerInfo 构建链式 step 转移与规划脚本。
pub fn GetMockSchedulerExt(scheduler_info: SchedulerInfo) -> Result<SchedulerExtension, DxfError> {
    if scheduler_info.step_infos.is_empty() {
        panic!("stepInfos should not be empty");
    }
    // 从 STEP_INIT 起按 step_infos 顺序串起转移，最后接到 STEP_DONE。
    let mut transitions = HashMap::new();
    let mut current = STEP_INIT;
    let mut step_infos = HashMap::new();
    for step_info in scheduler_info.step_infos {
        transitions.insert(current, step_info.step);
        current = step_info.step;
        step_infos.insert(step_info.step, step_info);
    }
    transitions.insert(current, STEP_DONE);
    Ok(SchedulerExtension {
        retryable: scheduler_info.all_error_retryable,
        transitions: Arc::new(transitions),
        step_infos: Arc::new(step_infos),
        call_counts: Arc::new(Mutex::new(HashMap::new())),
        mode: SchedulerMode::Normal,
        test_context: None,
        on_done_failed: Arc::new(AtomicBool::new(false)),
    })
}

/// 无错误、生成指定数量子任务的步骤脚本。
fn step(step: Step, subtask_count: usize) -> StepInfo {
    StepInfo {
        step,
        error: None,
        error_repeat_count: 0,
        subtask_count,
    }
}

/// 永久返回不可重试规划错误的步骤脚本。
fn permanent_error(step: Step) -> StepInfo {
    StepInfo {
        step,
        error: Some(DxfError("not retryable err".into())),
        error_repeat_count: i64::MAX,
        subtask_count: 0,
    }
}

#[allow(non_snake_case)]
/// 基础两步调度：STEP_ONE 3 个子任务，STEP_TWO 1 个。
pub fn GetMockBasicSchedulerExt() -> SchedulerExtension {
    GetMockSchedulerExt(SchedulerInfo {
        all_error_retryable: true,
        step_infos: vec![step(STEP_ONE, 3), step(STEP_TWO, 1)],
    })
    .expect("basic scheduler has steps")
}

#[allow(non_snake_case)]
/// 高可用（HA）测试用：子任务数量更大。
pub fn GetMockHATestSchedulerExt() -> SchedulerExtension {
    GetMockSchedulerExt(SchedulerInfo {
        all_error_retryable: true,
        step_infos: vec![step(STEP_ONE, 10), step(STEP_TWO, 5)],
    })
    .expect("HA scheduler has steps")
}

#[allow(non_snake_case)]
/// STEP_ONE 规划即不可重试失败。
pub fn GetPlanNotRetryableErrSchedulerExt() -> SchedulerExtension {
    GetMockSchedulerExt(SchedulerInfo {
        all_error_retryable: false,
        step_infos: vec![permanent_error(STEP_ONE)],
    })
    .expect("error scheduler has steps")
}

#[allow(non_snake_case)]
/// STEP_ONE 正常，STEP_TWO 规划不可重试失败。
pub fn GetStepTwoPlanNotRetryableErrSchedulerExt() -> SchedulerExtension {
    GetMockSchedulerExt(SchedulerInfo {
        all_error_retryable: false,
        step_infos: vec![step(STEP_ONE, 10), permanent_error(STEP_TWO)],
    })
    .expect("step-two error scheduler has steps")
}

#[allow(non_snake_case)]
/// 首次 INIT 规划可重试失败、on_done 首次失败的特殊调度器。
pub fn GetPlanErrSchedulerExt(test_context: Arc<TestContext>) -> SchedulerExtension {
    SchedulerExtension {
        retryable: true,
        transitions: Arc::new(HashMap::from([
            (STEP_INIT, STEP_ONE),
            (STEP_ONE, STEP_TWO),
            (STEP_TWO, STEP_DONE),
        ])),
        step_infos: Arc::new(HashMap::new()),
        call_counts: Arc::new(Mutex::new(HashMap::new())),
        mode: SchedulerMode::RetryOnceThenOnDoneError,
        test_context: Some(test_context),
        on_done_failed: Arc::new(AtomicBool::new(false)),
    }
}

#[allow(non_snake_case)]
/// 回滚测试用单步调度器。
pub fn GetMockRollbackSchedulerExt() -> SchedulerExtension {
    GetMockSchedulerExt(SchedulerInfo {
        all_error_retryable: true,
        step_infos: vec![step(STEP_ONE, 3)],
    })
    .expect("rollback scheduler has steps")
}
