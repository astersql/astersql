// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
// Licensed under the Apache License, Version 2.0.

// 示例 DXF 调度器：Init → 两步生成子任务 → Done/清理。
//
// step 线性推进：Init → One → Two → Done；每步按 taskMeta.SubtaskCount
// 批量产出 subtaskMeta。DXF 调度器负责任务生命周期与子任务分发。

use crate::{subtaskMeta, taskMeta};
use astersql_dxf_framework_taskexecutor::{Context, ExecutorError, Result, Task, TaskBase};
/// 初始 step：尚未开始执行业务步骤。
pub const StepInit: i64 = -1;
/// 第一步：生成并执行第一批子任务。
pub const StepOne: i64 = 1;
/// 第二步：生成并执行第二批子任务。
pub const StepTwo: i64 = 2;
/// 终止 step：全部步骤完成。
pub const StepDone: i64 = -2;
/// 框架 prepare step：准备逻辑已完成但业务步骤尚未开始。
pub const StepPrepared: i64 = -3;
/// 示例调度器实现，持有任务快照与子任务个数。
pub struct schedulerImpl {
    /// 当前调度的任务（含 Meta）。
    task: Task,
    /// 从 taskMeta 解出的每步子任务数。
    pub subtaskCount: i64,
}
/// 构造调度器；Init 前 subtaskCount 为 0。
pub fn newScheduler(_ctx: Context, task: Task) -> schedulerImpl {
    schedulerImpl {
        task,
        subtaskCount: 0,
    }
}
/// 对齐 DXF Scheduler 钩子：Init/Tick/下一批子任务/Done/下一步等。
impl schedulerImpl {
    /// 反序列化 taskMeta，填充 subtaskCount。
    pub fn Init(&mut self) -> Result<()> {
        let meta = taskMeta::Unmarshal(&self.task.Meta)
            .map_err(|e| ExecutorError(format!("unmarshal task meta failed: {e}")))?;
        self.subtaskCount = meta.SubtaskCount;
        Ok(())
    }
    /// 周期性回调；示例为空操作。
    pub fn OnTick(&self, _: &Context, _: &Task) {}
    /// 为 next_step 生成一批子任务 meta（仅 StepOne/StepTwo）。
    pub fn OnNextSubtasksBatch(
        &self,
        _: &Context,
        task: &Task,
        _: &[String],
        next_step: i64,
    ) -> Result<Vec<Vec<u8>>> {
        match next_step {
            StepOne | StepTwo => {
                assert!(self.subtaskCount >= 0, "makeslice len out of range");
                Ok((0..self.subtaskCount)
                    .map(|i| {
                        subtaskMeta {
                            Message: format!(
                                "subtask {i} of step {}",
                                Step2Str(&task.TaskBase.Type, next_step)
                            ),
                        }
                        .Marshal()
                    })
                    .collect())
            }
            _ => panic!("unexpected nextStep: {}", Step2Str("example", next_step)),
        }
    }
    /// 任务完成回调；示例直接成功。
    pub fn OnDone(&self, _: &Context, _: &Task) -> Result<()> {
        Ok(())
    }
    /// 可调度实例列表；示例返回空（由框架默认选节点）。
    pub fn GetEligibleInstances(&self, _: &Context, _: &Task) -> Result<Vec<String>> {
        Ok(vec![])
    }
    /// 错误是否可重试；示例恒为 true。
    pub fn IsRetryableErr(&self, _: &ExecutorError) -> bool {
        true
    }
    /// 根据当前 Step 返回下一 step（Init→One→Two→Done）。
    pub fn GetNextStep(&self, task: &TaskBase) -> i64 {
        match task.Step {
            StepInit => StepOne,
            StepOne => StepTwo,
            _ => StepDone,
        }
    }
    /// 调度前准备；示例直接成功。
    pub fn OnPrepare(&self, _: &Context, _: &Task) -> Result<()> {
        Ok(())
    }
}
/// 将 step 常量转为可读字符串（日志/Message 用）。
pub fn Step2Str(task_type: &str, step: i64) -> String {
    match step {
        StepInit => "init".to_string(),
        StepDone => "done".to_string(),
        StepPrepared => "prepared".to_string(),
        _ if task_type == "example" => match step {
            StepOne => "one".to_string(),
            StepTwo => "two".to_string(),
            3 => "three".to_string(),
            _ => format!("unknown step {step}"),
        },
        _ => format!("unknown type {task_type}"),
    }
}
/// 任务结束后的清理钩子（示例为空实现）。
pub struct postCleanupImpl;
/// postCleanupImpl 方法实现。
impl postCleanupImpl {
    /// 清理中间资源；示例直接成功。
    pub fn CleanUp(&self, _: &Context, _: &Task) -> Result<()> {
        Ok(())
    }
}
