// Copyright 2026 AsterSQL.

// executor 内部 MPP（Massively Parallel Processing，大规模并行处理）子模块入口。
//
// MPP 将物理计划切成多个 Fragment，调度到 TiFlash 计算节点并行执行，再经
// Exchange 汇聚结果。本模块提供：
// - `local_mpp_coordinator`：本地协调器，负责派发任务、拉流与 summary 上报；
// - `executor_with_retry`：在可恢复错误时重建 gather 并重试；
// - `recovery_handler`：缓冲响应并按策略尝试恢复。

#![allow(dead_code, non_snake_case)]

// 可重试 MPP 响应包装、本地协调器与恢复处理器。
mod executor_with_retry;
mod local_mpp_coordinator;
mod recovery_handler;

/// 可重试执行器、协调器注册表与恢复配置。
pub use executor_with_retry::{
    CoordinatorFactory, CoordinatorRegistry, CoordinatorUniqueId, ExecutorWithRetry,
    MppCoordinatorManager, MppRecoveryConfig, NewExecutorWithRetry, SharedMppCoordinator,
    SharedMppStatusReporter,
};
/// 本地 MPP 协调器构造入口与会话/计划/上报相关类型。
pub use local_mpp_coordinator::{
    MppCoordinatorPlan, MppDispatchSession, MppReportSink, MppStoreInfo, NewLocalMppCoordinator,
    NoopMppReportSink,
};

// 派发准备、重试执行器、协调器与响应契约的单元测试。
#[cfg(test)]
mod coordinator_prep_aster_unit_test;
#[cfg(test)]
mod executor_with_retry_aster_unit_test;
#[cfg(test)]
mod executor_with_retry_test;
#[cfg(test)]
mod local_mpp_coordinator_aster_unit_test;
#[cfg(test)]
mod local_mpp_coordinator_test;
#[cfg(test)]
mod mpp_response_aster_unit_test;
#[cfg(test)]
mod recovery_handler_aster_unit_test;
