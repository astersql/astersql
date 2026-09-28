// Copyright 2026 AsterSQL.

// 索引顾问（Index Advisor）crate 入口。
//
// 根据 workload（一组 SQL 及其出现频率）推荐可降低查询代价的二级索引。
// 二级索引是按列值排序的辅助结构，用于加速过滤、排序与连接。
// 子模块覆盖候选枚举算法、数据模型、what-if 优化器、配置项与 SQL 工具函数。

#![allow(dead_code)]

/// 索引推荐主算法：按列宽逐轮扩展候选并贪心选优。
pub mod algorithm;
/// 对外入口：从 SQL 文本或摘要生成推荐结果。
pub mod indexadvisor;
/// 查询、列、索引与推荐结果等核心数据结构。
pub mod model;
/// what-if 优化器接口：在假设索引下估算执行计划代价。
pub mod optimizer;
/// 顾问参数（最大索引数、列宽、超时等）的读写与校验。
pub mod options;
/// SQL 规范化、表名收集、可索引列识别与代价汇总工具。
pub mod utils;

/// 核心数据模型与 Go 构造语义的对齐测试。
#[cfg(test)]
#[path = "model_test.rs"]
mod model_test;

#[cfg(test)]
#[path = "algorithm_test.rs"]
mod algorithm_test;

/// SQL 接口相关单测：验证等价查询规范化与 digest 一致。
#[cfg(test)]
#[path = "indexadvisor_sql_test.rs"]
mod indexadvisor_sql_test;
/// 顾问模型语义单测：索引前缀包含关系等。
#[cfg(test)]
#[path = "indexadvisor_test.rs"]
mod indexadvisor_test;
/// TPC-H 风格 join 场景下的表名收集单测。
#[cfg(test)]
#[path = "indexadvisor_tpch_test.rs"]
mod indexadvisor_tpch_test;
/// what-if 优化器代价比较规则单测。
#[cfg(test)]
#[path = "optimizer_test.rs"]
mod optimizer_test;
/// 配置项 duration 解析单测。
#[cfg(test)]
#[path = "options_test.rs"]
mod options_test;
/// utils 模块辅助逻辑单测。
#[cfg(test)]
#[path = "utils_test.rs"]
mod utils_test;
