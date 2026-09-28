// Copyright 2026 AsterSQL.

// Join（连接）执行器子模块入口。
//
// 汇集 Hash Join、Merge Join（排序归并连接）、Index Lookup Join 等连接算法，
// 以及 build/probe、spill（内存不足时落盘）、row table（按行紧凑存储的 build 侧表）
// 等支撑组件。Join 把两路输入按等值或条件组合成结果行，是 SQL `JOIN` 的执行层实现。

#![allow(dead_code)]

/// Anti Semi Join（反半连接）probe：仅输出左表中无匹配右表的行。
pub mod anti_semi_join_probe;
/// Hash Join probe 公共基类：管理 probe chunk、匹配链表与结果构造。
pub mod base_join_probe;
/// Semi / Anti Semi Join 共享状态与 other condition（额外过滤条件）处理。
pub mod base_semi_join;
/// Build 侧并发分片哈希表，降低多 worker 插入时的锁竞争。
pub mod concurrent_map;
/// Hash Join 执行器公共基座：分区、worker 调度与上下文。
pub mod hash_join_base;
/// Hash Join spill：内存超限时将分区行写入临时存储。
pub mod hash_join_spill;
/// Spill 读写辅助：分区落盘、恢复与字节统计。
pub mod hash_join_spill_helper;
#[cfg(test)]
mod hash_join_spill_helper_test;
/// Hash Join 运行时统计指标采集。
pub mod hash_join_stats;
#[cfg(test)]
mod hash_join_stats_test;
/// Hash Join 单元测试共用工具与数据构造。
pub mod hash_join_test_util;
#[cfg(test)]
mod hash_join_test_util_test;
/// Hash Join v1 实现路径。
pub mod hash_join_v1;
/// Hash Join v2 实现路径（分区 row table + tagged pointer）。
pub mod hash_join_v2;
#[cfg(test)]
mod hash_join_v2_test;
/// Hash Join v1 所用哈希表结构。
pub mod hash_table_v1;
/// Hash Join v2 所用哈希表结构。
pub mod hash_table_v2;
/// Index Lookup Hash Join：索引回表后的哈希连接变体。
pub mod index_lookup_hash_join;
#[cfg(test)]
mod index_lookup_hash_join_test;
/// Index Lookup Join：按外层行探测索引获取内层行。
pub mod index_lookup_join;
/// Index Lookup Merge Join：索引回表结果再做归并连接。
pub mod index_lookup_merge_join;
#[cfg(test)]
mod index_lookup_merge_join_test;
/// Inner Join（内连接）probe：只输出键匹配成功的组合行。
pub mod inner_join_probe;
/// Row table：build 侧按行序列化后的分段存储。
pub mod join_row_table;
/// Join 表元数据：列布局、key 编码模式、null map 等。
pub mod join_table_meta;
/// Joiner：按 Join 类型拼装匹配/未匹配结果行。
pub mod joiner;
/// Left Outer Semi Join probe：为左行追加匹配标记列（含 anti / null-aware）。
pub mod left_outer_semi_join_probe;
/// Merge Join（排序归并连接）执行器与分组推进逻辑。
pub mod merge_join;
#[cfg(test)]
mod merge_join_test;
/// Left / Right Outer Join 共用 probe 实现。
pub mod outer_join_probe;
#[cfg(test)]
mod outer_join_probe_test;
/// 将 build chunk 序列化并写入分区 row table 的构建器。
pub mod row_table_builder;
/// Semi Join（半连接）probe：左行存在匹配时输出左行。
pub mod semi_join_probe;
/// Tagged pointer：在指针低位嵌入分区/标记信息以加速探测。
pub mod tagged_ptr;

#[cfg(test)]
/// Anti Semi Join probe 单元测试。
mod anti_semi_join_probe_test;
#[cfg(test)]
/// Hash Join probe 公共状态与键比较单元测试。
mod base_join_probe_test;
#[cfg(test)]
/// Semi / Anti Semi Join 共用状态单元测试。
mod base_semi_join_test;
#[cfg(test)]
/// Join 相关基准测试草稿。
mod bench_test;
#[cfg(test)]
/// 并发分片 map 单元测试。
mod concurrent_map_test;
#[cfg(test)]
/// Hash Join spill OOM action 单元测试。
mod hash_join_spill_test;
#[cfg(test)]
/// Hash Join v1 执行器与 Nested Loop Apply 单元测试。
mod hash_join_v1_test;
#[cfg(test)]
/// Hash table v1 单元测试。
mod hash_table_v1_test;
#[cfg(test)]
/// Hash table v2 单元测试。
mod hash_table_v2_test;
#[cfg(test)]
/// Index Lookup Join 活动路径单元测试。
mod index_lookup_join_test;
#[cfg(test)]
/// Inner Join probe 单元测试。
mod inner_join_probe_test;
#[cfg(test)]
/// Inner Join spill 单元测试。
mod inner_join_spill_test;
#[cfg(test)]
/// Row table 单元测试。
mod join_row_table_test;
#[cfg(test)]
/// Join 统计相关单元测试。
mod join_stats_test;
#[cfg(test)]
/// Join 表元数据单元测试。
mod join_table_meta_test;
#[cfg(test)]
/// Joiner 单元测试。
mod joiner_test;
#[cfg(test)]
/// Left Outer Anti Semi Join probe 单元测试。
mod left_outer_anti_semi_join_probe_test;
#[cfg(test)]
/// Left Outer Join probe 单元测试。
mod left_outer_join_probe_test;
#[cfg(test)]
/// Left Outer Semi Join probe 单元测试。
mod left_outer_semi_join_probe_test;
#[cfg(test)]
/// Outer Join spill 单元测试。
mod outer_join_spill_test;
#[cfg(test)]
/// Right Outer Join probe 单元测试。
mod right_outer_join_probe_test;
#[cfg(test)]
/// Row table builder 单元测试。
mod row_table_builder_test;
#[cfg(test)]
/// Semi Join probe 单元测试。
mod semi_join_probe_test;
#[cfg(test)]
/// Tagged pointer 单元测试。
mod tagged_ptr_test;
