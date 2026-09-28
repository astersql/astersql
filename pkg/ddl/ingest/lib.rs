// Copyright 2026 AsterSQL.

// DDL ingest（快速导入）crate 入口。
//
// 在执行加索引等 DDL（Data Definition Language，数据定义语言）操作时，
// ingest 模式先将索引键值对写入本地引擎再批量导入存储层，以加速回填。
// 本 crate 聚合后端上下文、引擎、内存/磁盘配额、检查点、消息与测试替身等子模块。

#![allow(dead_code)]

/// 后端上下文：管理单个 DDL 任务下的写入引擎与导入决策。
pub mod backend;
/// 后端管理器：按任务 ID 创建、查找与销毁后端实例。
pub mod backend_mgr;
/// 检查点（checkpoint）：记录回填进度，支持断点续传。
pub mod checkpoint;
/// 数据收集器：汇总回填过程中的统计与结果。
pub mod collector;
#[cfg(test)]
mod collector_test;
/// ingest 配置：并发度、内存缓存与中间文件目录等参数。
pub mod config;
#[cfg(test)]
mod config_test;
/// 磁盘配额根（DiskRoot）：跟踪本地磁盘占用并判断是否需导入。
pub mod disk_root;
#[cfg(test)]
mod disk_root_test;
/// 本地写入引擎与写入器抽象及实现。
pub mod engine;
/// 引擎注册/注销的便捷封装。
pub mod engine_mgr;
#[cfg(test)]
mod engine_mgr_test;
#[cfg(test)]
mod engine_test;
/// 全局 ingest 环境与临时目录管理。
pub mod env;
/// 内存配额根（MemRoot）：跟踪并限制 ingest 过程的内存使用。
pub mod mem_root;
/// 内存分配失败等错误消息与错误结构。
pub mod message;
#[cfg(test)]
mod message_test;
/// 测试用 mock：模拟后端与引擎以便单元测试。
pub mod mock;
#[cfg(test)]
mod mock_test;
/// 通用工具函数。
pub mod util;
#[cfg(test)]
mod util_test;

/// 后端管理器相关单元测试。
#[cfg(test)]
mod backend_mgr_test;

/// 后端上下文相关单元测试。
#[cfg(test)]
mod backend_test;

/// 检查点相关单元测试。
#[cfg(test)]
mod checkpoint_test;
/// 环境与临时目录相关单元测试。
#[cfg(test)]
mod env_test;
/// 跨模块集成测试。
#[cfg(test)]
mod integration_test;
/// 测试入口与 Go `TestMain` 契约。
#[cfg(test)]
mod main_test;
/// 内存配额根相关单元测试。
#[cfg(test)]
mod mem_root_test;
