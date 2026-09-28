// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
// Licensed under the Apache License, Version 2.0.

// 示例任务执行器：包装 BaseTaskExecutor，子任务仅 Unmarshal meta。
//
// Extension 声明幂等与错误可重试，并提供 stepExecutor。
// 任务执行器在 follower 节点上消费调度器下发的子任务。

use crate::subtaskMeta;
use astersql_dxf_framework_taskexecutor as framework;
use astersql_util_logutil::log::{BgLogger, LogField, LogLevel};
use framework::{
    Context, ExecutorError, Extension, Param, Result, StepExecutor, Subtask, Task, TaskExecutor,
};
use std::sync::Arc;
/// 示例 TaskExecutor，委托给框架 BaseTaskExecutor。
pub struct taskExecutor {
    /// 框架提供的通用执行循环与取消/关闭能力。
    pub BaseTaskExecutor: Arc<framework::BaseTaskExecutor>,
}
/// 注入 exampleExtension 后构造执行器（对齐 Go factory）。
pub fn newTaskExecutor(ctx: Context, task: Task, mut param: Param) -> Arc<taskExecutor> {
    param.Extension = Arc::new(exampleExtension);
    Arc::new(taskExecutor {
        BaseTaskExecutor: framework::NewBaseTaskExecutor(ctx, task, param),
    })
}
/// 示例 Extension：幂等、可重试，并返回默认 stepExecutor。
struct exampleExtension;
/// Extension trait：供框架查询幂等性与 step 执行器。
impl Extension for exampleExtension {
    /// 子任务是否幂等（可安全重跑）；示例恒 true。
    fn IsIdempotent(&self, _: &Subtask) -> bool {
        true
    }
    /// 返回当前任务对应的 StepExecutor。
    fn GetStepExecutor(&self, _: &Task) -> Result<Arc<dyn StepExecutor>> {
        Ok(Arc::new(stepExecutor::default()))
    }
    /// 错误是否可重试；示例恒 true。
    fn IsRetryableError(&self, _: &ExecutorError) -> bool {
        true
    }
}
/// taskExecutor 上与 Extension 对齐的便捷方法。
impl taskExecutor {
    pub fn IsIdempotent(&self, _: &Subtask) -> bool {
        true
    }
    pub fn GetStepExecutor(&self, _: &Task) -> Result<Arc<dyn StepExecutor>> {
        Ok(Arc::new(stepExecutor::default()))
    }
    pub fn IsRetryableError(&self, _: &ExecutorError) -> bool {
        true
    }
}
/// 将 Init/Run/Cancel/Close 等委托给 BaseTaskExecutor。
impl TaskExecutor for taskExecutor {
    /// 初始化执行器。
    fn Init(&self, c: &Context) -> Result<()> {
        self.BaseTaskExecutor.Init(c)
    }
    /// 启动执行循环，拉取并跑完当前 step 子任务。
    fn Run(&self) {
        self.BaseTaskExecutor.Run()
    }
    /// 返回任务基础信息快照。
    fn GetTaskBase(&self) -> framework::TaskBase {
        self.BaseTaskExecutor.GetTaskBase()
    }
    /// 取消正在运行的子任务。
    fn CancelRunningSubtask(&self) {
        self.BaseTaskExecutor.CancelRunningSubtask()
    }
    /// 取消整个任务执行。
    fn Cancel(&self) {
        self.BaseTaskExecutor.Cancel()
    }
    /// 关闭执行器并释放资源。
    fn Close(&self) {
        self.BaseTaskExecutor.Close()
    }
    /// 转发到 taskExecutor::IsRetryableError。
    fn IsRetryableError(&self, e: &ExecutorError) -> bool {
        taskExecutor::IsRetryableError(self, e)
    }
}
#[derive(Default)]
/// 单步执行器：对每个子任务 Unmarshal meta 即视为成功。
pub struct stepExecutor {
    /// 框架 StepExecutor 基类字段（本示例未额外使用）。
    pub BaseStepExecutor: framework::BaseStepExecutor,
}
/// StepExecutor：实现 RunSubtask。
impl StepExecutor for stepExecutor {
    /// 反序列化 subtaskMeta；解析失败则返回 ExecutorError。
    fn RunSubtask(&self, _: &Context, subtask: &mut Subtask) -> Result<()> {
        let meta = subtaskMeta::Unmarshal(&subtask.Meta).map_err(ExecutorError)?;
        BgLogger().log(
            LogLevel::Info,
            "RunSubtask",
            [
                LogField::I64("subtaskID".into(), subtask.SubtaskBase.ID),
                LogField::String("message".into(), meta.Message),
            ],
        );
        Ok(())
    }
}
