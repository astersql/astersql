// Copyright 2026 AsterSQL.

// 测试用 storage 适配层：SessionExecutor / TaskHandle 最小接口。
//
// 对齐 Go dxf storage 包中调度扩展回调所需的会话与历史 subtask 查询边界。

#![allow(non_snake_case)]

/// 统一错误类型。
pub type Error = anyhow::Error;

/// 会话上下文占位（测试中通常为空）。
pub mod sessionctx {
    #[derive(Clone, Debug, Default)]
    /// 空会话上下文。
    pub struct Context;
}

/// 执行侧汇总类型。
pub mod execute {
    use std::sync::atomic::{AtomicI64, AtomicU64};
    use std::time::SystemTime;

    #[derive(Clone, Debug, Eq, PartialEq)]
    /// 子任务进度采样点。
    pub struct Progress {
        /// 预留行计数。
        pub RowCnt: i64,
        /// 已处理的通用计量单位。
        pub Processed: i64,
        /// 采样时间。
        pub UpdateTime: SystemTime,
    }

    #[derive(Debug, Default)]
    /// 子任务运行时汇总。
    pub struct SubtaskSummary {
        /// 处理行数。
        pub RowCnt: AtomicI64,
        /// 已处理的通用计量单位。
        pub Processed: AtomicI64,
        /// 从源读取的字节数。
        pub ReadBytes: AtomicI64,
        /// 对外部存储发出的 GET 请求数。
        pub GetReqCnt: AtomicU64,
        /// 对外部存储发出的 PUT 请求数。
        pub PutReqCnt: AtomicU64,
        /// 历史进度采样。
        pub Progresses: Vec<Progress>,
    }
}

/// 在新 session / 事务中执行回调。
pub trait SessionExecutor {
    /// 打开新 session 并执行回调。
    fn WithNewSession<F>(&self, callback: F) -> Result<(), Error>
    where
        F: FnOnce(sessionctx::Context) -> Result<(), Error>;

    /// 在新事务中执行回调。
    fn WithNewTxn<F>(&self, context: crate::Context, callback: F) -> Result<(), Error>
    where
        F: FnOnce(sessionctx::Context) -> Result<(), Error>;
}

/// 调度扩展可读的历史 subtask 句柄。
pub trait TaskHandle: SessionExecutor {
    /// 读取上一批子任务的 meta 列表。
    fn GetPreviousSubtaskMetas(
        &self,
        task_id: i64,
        step: crate::proto::Step,
    ) -> Result<Vec<Vec<u8>>, Error>;

    /// 读取上一批子任务的汇总信息。
    fn GetPreviousSubtaskSummary(
        &self,
        task_id: i64,
        step: crate::proto::Step,
    ) -> Result<Vec<execute::SubtaskSummary>, Error>;
}
