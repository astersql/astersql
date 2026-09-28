// Copyright 2026 AsterSQL.

// 聚合/窗口函数（aggfuncs）crate 入口。
//
// 对应 Go 的 `pkg/executor/aggfuncs`：实现 SQL 聚合函数（COUNT/SUM/AVG/MAX/MIN、
// 方差、比特聚合、JSON 聚合、GROUP_CONCAT 等）与窗口函数（ROW_NUMBER、RANK、
// LEAD/LAG、NTILE、百分位等）的部分结果（partial result）更新、合并与求值。
// 另含 spill（内存不足时把部分结果落盘）的序列化/反序列化辅助模块。
// 聚合执行器在 hash/stream 聚合路径上依赖本 crate 暴露的构建器与具体函数实现。

#![allow(dead_code)]

/// 聚合函数公共类型、trait 与常量定义。
pub mod aggfuncs;
/// 按 AST 函数名与字段类型构建具体聚合函数实例的工厂。
pub mod builder;
/// AVG 平均值聚合。
pub mod func_avg;
/// 比特聚合（BIT_AND / BIT_OR / BIT_XOR）。
pub mod func_bitfuncs;
/// COUNT 计数聚合。
pub mod func_count;
/// COUNT(DISTINCT ...) 去重计数。
pub mod func_count_distinct;
/// CUME_DIST 窗口函数（累积分布）。
pub mod func_cume_dist;
/// FIRST_ROW / 窗口首行取值。
pub mod func_first_row;
/// GROUP_CONCAT 字符串拼接聚合。
pub mod func_group_concat;
/// JSON_ARRAYAGG 聚合。
pub mod func_json_arrayagg;
/// JSON_OBJECTAGG 聚合。
pub mod func_json_objectagg;
/// LEAD / LAG 窗口函数。
pub mod func_lead_lag;
/// MAX / MIN 聚合。
pub mod func_max_min;
/// NTILE 窗口函数（分桶编号）。
pub mod func_ntile;
/// PERCENT_RANK 窗口函数。
pub mod func_percent_rank;
/// 百分位相关聚合/窗口逻辑。
pub mod func_percentile;
/// RANK / DENSE_RANK / ROW 序数窗口函数。
pub mod func_rank;
/// STDDEV_POP 总体标准差。
pub mod func_stddevpop;
/// STDDEV_SAMP 样本标准差。
pub mod func_stddevsamp;
/// SUM 求和（浮点/十进制等）。
pub mod func_sum;
/// 整数路径 SUM。
pub mod func_sum_int;
/// 聚合求值过程中的值类型与堆外内存计量。
pub mod func_value;
/// VAR_POP 总体方差。
pub mod func_varpop;
/// VAR_SAMP 样本方差。
pub mod func_varsamp;
/// ROW_NUMBER 窗口函数。
pub mod row_number;
/// spill 反序列化：从 chunk 列还原各类型 partial result。
pub mod spill_deserialize_helper;
/// spill 序列化：把各类型 partial result 写入 chunk 列。
pub mod spill_serialize_helper;

pub use aggfuncs::*;
/// 反序列化辅助器，供聚合执行器从 spill 列读回部分结果。
pub use spill_deserialize_helper::DeserializeHelper;
/// 序列化辅助器，供聚合执行器把部分结果写入 spill 列。
pub use spill_serialize_helper::SerializeHelper;

#[cfg(test)]
mod aggfunc_test;
#[cfg(test)]
mod aggfuncs_test;
#[cfg(test)]
mod builder_test;
#[cfg(test)]
mod export_test;
#[cfg(test)]
mod func_avg_test;
#[cfg(test)]
mod func_bitfuncs_test;
#[cfg(test)]
mod func_count_distinct_test;
#[cfg(test)]
mod func_count_test;
#[cfg(test)]
mod func_cume_dist_test;
#[cfg(test)]
mod func_distinct_agg_test;
#[cfg(test)]
mod func_first_row_test;
#[cfg(test)]
mod func_group_concat_test;
#[cfg(test)]
mod func_json_arrayagg_test;
#[cfg(test)]
mod func_json_objectagg_test;
#[cfg(test)]
mod func_lead_lag_test;
#[cfg(test)]
mod func_max_min_test;
#[cfg(test)]
mod func_ntile_test;
#[cfg(test)]
mod func_percent_rank_test;
#[cfg(test)]
mod func_percentile_test;
#[cfg(test)]
mod func_rank_test;
#[cfg(test)]
mod func_stddevpop_test;
#[cfg(test)]
mod func_stddevsamp_test;
#[cfg(test)]
mod func_sum_int_test;
#[cfg(test)]
mod func_sum_test;
#[cfg(test)]
mod func_value_test;
#[cfg(test)]
mod func_varpop_test;
#[cfg(test)]
mod func_varsamp_test;
#[cfg(test)]
mod go_scenario_coverage_test;
#[cfg(test)]
mod main_test;
#[cfg(test)]
mod row_number_test;
#[cfg(test)]
mod spill_deserialize_helper_test;
#[cfg(test)]
mod spill_helper_test;
#[cfg(test)]
mod window_func_test;
