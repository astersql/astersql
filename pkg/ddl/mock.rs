// Copyright 2026 AsterSQL.
// Copyright 2017 PingCAP, Inc.
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

// DDL 测试用 Mock 辅助：删除区间管理器与简易建表元数据构造。
//
// Delete Range（删除区间）是 TiDB/AsterSQL 在 DROP/TRUNCATE 后把待 GC
// （垃圾回收）的键范围登记到系统表的机制。本文件提供可注入的
// `DeleteRangeManager` 空实现，以及由列定义拼出 `TableInfo` 的工具函数。

use std::sync::atomic::{AtomicUsize, Ordering};

use astersql_meta_metabuild as metabuild;
use astersql_meta_model as model;
use astersql_parser as parser;
use astersql_parser_ast as ast;

use crate::BuildTableInfoFromAST;

/// 批量写入 delete-range 任务时的默认批大小（可被测试覆盖）。
static BATCH_INSERT_DELETE_RANGE_SIZE: AtomicUsize = AtomicUsize::new(256);

/// 设置批量插入 delete-range 记录的批大小。
pub fn set_batch_insert_delete_range_size(size: usize) {
    BATCH_INSERT_DELETE_RANGE_SIZE.store(size, Ordering::SeqCst);
}

/// 读取当前批量插入 delete-range 记录的批大小。
pub fn batch_insert_delete_range_size() -> usize {
    BATCH_INSERT_DELETE_RANGE_SIZE.load(Ordering::SeqCst)
}

/// 删除区间任务管理器接口：登记/移除 GC 删除范围作业并控制生命周期。
pub trait DeleteRangeManager: Send {
    /// 为指定 DDL job 登记一条 delete-range 任务。
    fn add_delete_range_job(&mut self, job_id: i64) -> Result<(), String>;
    /// 从 GC delete-range 队列中移除指定 job。
    fn remove_from_gc_delete_range(&mut self, job_id: i64) -> Result<(), String>;
    /// 启动后台处理（Mock 为空操作）。
    fn start(&mut self);
    /// 清空内部状态。
    fn clear(&mut self);
}

/// 空操作的 `DeleteRangeManager` 实现，供单元测试注入。
#[derive(Debug, Default)]
pub struct MockDeleteRange;

impl DeleteRangeManager for MockDeleteRange {
    fn add_delete_range_job(&mut self, _job_id: i64) -> Result<(), String> {
        Ok(())
    }

    fn remove_from_gc_delete_range(&mut self, _job_id: i64) -> Result<(), String> {
        Ok(())
    }

    fn start(&mut self) {}

    fn clear(&mut self) {}
}

/// 构造装箱后的 Mock 删除区间管理器。
pub fn new_mock_delete_range_manager() -> Box<dyn DeleteRangeManager> {
    Box::<MockDeleteRange>::default()
}

/// 根据完整 CREATE TABLE AST 与给定 `table_id` 构造表元数据。
///
/// 与 Go `MockTableInfo` 一样复用正式建表管线，因此列/表选项、索引、约束、
/// 外键、AUTO_RANDOM 与错误传播不会形成一套仅供测试使用的简化语义。
pub fn mock_table_info<C: ?Sized + 'static, E: 'static>(
    context: &metabuild::Context<C, E>,
    statement: &ast::CreateTableStmt,
    table_id: i64,
) -> Result<model::TableInfo, parser::errors::Error> {
    let mut table = BuildTableInfoFromAST(context, statement)?;
    table.ID = table_id;
    Ok(table)
}
