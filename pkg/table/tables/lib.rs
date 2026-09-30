// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

// `tables` crate：表实现核心（对应 Go `pkg/table/tables`）。
//
// 提供缓存表（Cached Table）、索引键值生成、分区路由、变更一致性检查、
// 远程状态（StateRemote）以及表实体等子模块。分区表达式求值依赖
// `expression-runtime` feature。

#![allow(non_snake_case)]

/// 断言（assertion）相关辅助：控制事务对键的存在性假设。
pub mod assertion;
/// 缓存表：将整表数据缓存在 TiDB 进程内存中以加速只读查询。
pub mod cache;
/// SQL execution partition routing over the canonical catalog table model.
pub mod canonical_partition;
/// 二级索引键/值编码与 DDL 状态相关的临时索引逻辑。
pub mod index;
/// 行与索引 Mutation 的数据一致性检查。
pub mod mutation_checker;
/// 分区表路由：Hash/Key/Range/List 等分区定位与双写集合。
pub mod partition;
/// 分区表达式与裁剪（pruning）元数据，需 expression 运行时。
#[cfg(feature = "expression-runtime")]
mod partition_expr;
/// 缓存表远程锁状态（读/写 lease）的加载与更新。
pub mod state_remote;
/// 普通表与分区表的 Table 实现主体。
pub mod tables;
/// 测试用表构造与辅助工具。
pub mod testutil;

/// 在启用 expression-runtime 时再导出分区表达式类型。
#[cfg(feature = "expression-runtime")]
pub use partition_expr::*;

#[cfg(all(test, feature = "expression-runtime"))]
#[path = "partition_expr_test.rs"]
mod partition_expr_test;

#[cfg(test)]
mod assertion_test;
#[cfg(test)]
mod bench_test;
#[cfg(test)]
mod cache_test;
#[cfg(test)]
mod export_test;
#[cfg(test)]
mod index_test;
#[cfg(test)]
mod main_test;
#[cfg(test)]
mod mutation_checker_test;
#[cfg(test)]
mod partition_test;
#[cfg(test)]
mod state_remote_test;
#[cfg(test)]
mod tables_test;
