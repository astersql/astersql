// Copyright 2026 AsterSQL.

// `autoanalyze/priorityqueue` crate 入口。
//
// 实现自动 ANALYZE（收集表/索引统计信息）的优先队列：按变更比例、表大小、
// 上次分析间隔等指标计算权重，调度非分区表、静态分区表、动态分区表的分析作业，
// 并响应 DDL（数据定义语言，如加索引、删表）事件重建或删除队列项。

#![allow(non_snake_case, non_upper_case_globals)]

/// 分析作业工厂：按表类型创建对应 AnalysisJob。
mod analysis_job_factory;
/// 优先级权重计算器。
mod calculator;
/// 动态分区表（分区裁剪可见全局统计）分析作业。
mod dynamic_partitioned_table_analysis_job;
/// 优先队列底层最大堆实现。
mod heap;
/// 自动分析时间窗口（按天/时区）判定。
mod interval;
/// AnalysisJob trait 与公共元数据/钩子类型。
mod job;
/// 非分区表分析作业。
mod non_partitioned_table_analysis_job;
/// 分析优先队列主体（初始化、后台刷新、Pop/Push）。
mod queue;
/// 优先队列对 DDL schema 变更事件的处理。
mod queue_ddl_handler;
/// 静态分区模式（逐分区分析）分析作业。
mod static_partitioned_table_analysis_job;

pub use analysis_job_factory::*;
pub use calculator::*;
pub use dynamic_partitioned_table_analysis_job::*;
pub use heap::*;
pub use interval::*;
pub use job::*;
pub use non_partitioned_table_analysis_job::*;
pub use queue::*;
pub use queue_ddl_handler::*;
pub use static_partitioned_table_analysis_job::*;

#[cfg(test)]
#[path = "analysis_job_factory_test.rs"]
/// 作业工厂单元测试。
mod analysis_job_factory_test;
#[cfg(test)]
#[path = "calculator_test.rs"]
/// 权重计算器单元测试。
mod calculator_test;
#[cfg(test)]
#[path = "dynamic_partitioned_table_analysis_job_test.rs"]
/// 动态分区表作业单元测试。
mod dynamic_partitioned_table_analysis_job_test;
#[cfg(test)]
#[path = "heap_test.rs"]
/// 优先堆单元测试。
mod heap_test;
#[cfg(test)]
#[path = "interval_test.rs"]
/// 时间窗口单元测试。
mod interval_test;
#[cfg(test)]
#[path = "job_test.rs"]
/// AnalysisJob 公共逻辑单元测试。
mod job_test;
#[cfg(test)]
#[path = "main_test.rs"]
/// 包级测试入口与后台间隔常量校验。
mod main_test;
#[cfg(test)]
#[path = "non_partitioned_table_analysis_job_test.rs"]
/// 非分区表作业单元测试。
mod non_partitioned_table_analysis_job_test;
#[cfg(test)]
#[path = "queue_ddl_handler_test.rs"]
/// DDL 事件处理单元测试。
mod queue_ddl_handler_test;
#[cfg(test)]
#[path = "queue_test.rs"]
/// 优先队列主体单元测试。
mod queue_test;
#[cfg(test)]
#[path = "static_partitioned_table_analysis_job_test.rs"]
/// 静态分区表作业单元测试。
mod static_partitioned_table_analysis_job_test;
