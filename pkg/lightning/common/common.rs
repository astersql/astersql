// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
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

// Lightning 公共模块：表级自增 ID / RowID / AUTO_RANDOM 分配器抽象。
//
// 导入完成后需要把各表已用到的最大自增/行号写回元数据（rebase），
// 避免后续在线写入与导入数据冲突。本文件定义分配器类型、存储侧需求接口，
// 以及按表结构选取并 rebase 全局分配器的辅助函数。

use crate::{CommonError, Context};
use std::collections::HashMap;
use std::sync::{Arc, LazyLock};

/// 索引引擎在部分场景中使用的特殊引擎 ID（非真实表引擎）。
pub const IndexEngineID: i32 = -1;
/// 表示“全部表”的占位名称，用于配置或过滤范围。
pub const AllTables: &str = "all";

/// Lightning 认为重要、导入会话应对齐的系统变量默认值。
///
/// 包括 `max_allowed_packet`、时区、SQL 时间本地化等，保证编码与 TiDB 行为一致。
pub static DefaultImportantVariables: LazyLock<HashMap<&'static str, &'static str>> =
    LazyLock::new(|| {
        HashMap::from([
            ("max_allowed_packet", "67108864"),
            ("div_precision_increment", "4"),
            ("time_zone", "SYSTEM"),
            ("lc_time_names", "en_US"),
            ("default_week_format", "0"),
            ("block_encryption_mode", "aes-128-ecb"),
            ("group_concat_max_len", "1024"),
            ("tidb_backoff_weight", "6"),
        ])
    });

/// TiDB 导入路径下默认需要设置的系统变量（如行格式版本）。
pub static DefaultImportVariablesTiDB: LazyLock<HashMap<&'static str, &'static str>> =
    LazyLock::new(|| HashMap::from([("tidb_row_format_version", "1")]));

/// 自增相关分配器的种类。
///
/// - `RowID`：隐式行号（无显式主键时由 TiDB 分配）；
/// - `AutoIncrement`：`AUTO_INCREMENT` 列；
/// - `AutoRandom`：`AUTO_RANDOM` 列（按随机位打散，减轻热点）。
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum AllocatorType {
    RowID,
    AutoIncrement,
    AutoRandom,
}

/// 全局自增/行号分配器接口。
///
/// `NextGlobalAutoID` 返回“下一个可用 ID”；`Rebase` 将分配游标抬到不低于 `base`。
pub trait Allocator: Send + Sync {
    fn NextGlobalAutoID(&self) -> Result<i64, CommonError>;
    fn GetType(&self) -> AllocatorType;
    fn Rebase(&self, ctx: &Context, base: i64, alloc_ids: bool) -> Result<(), CommonError>;
}

/// Supplies the concrete allocator used by a storage implementation.
/// 由存储实现提供：是否可用以及如何按表构造具体分配器。
pub trait AutoIDRequirement: Send + Sync {
    fn StoreAvailable(&self) -> bool;
    fn NewAllocator(
        &self,
        db_id: i64,
        table_id: i64,
        unsigned: bool,
        allocator_type: AllocatorType,
        cache_step: u64,
        table_version: u16,
    ) -> Arc<dyn Allocator>;
}

/// 表上与自增/行号相关的精简元信息，用于决定需要哪些分配器。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TableInfo {
    pub ID: i64,
    pub Name: String,
    pub Version: u16,
    pub HasAutoRowID: bool,
    pub HasAutoIncrement: bool,
    pub HasAutoRandom: bool,
    /// 为 true 时 AutoIncrement 与 RowID 使用独立分配器。
    pub SeparateAutoIncrement: bool,
    pub AutoIncrementUnsigned: bool,
    pub AutoRandomUnsigned: bool,
}

/// 判断表是否带有隐式 Auto RowID。
pub fn TableHasAutoRowID(table: &TableInfo) -> bool {
    table.HasAutoRowID
}

/// 查询该表所有相关分配器的全局 next，返回其中最大 base（next - 1）。
pub fn GetMaxAutoIDBase(
    requirement: Option<&dyn AutoIDRequirement>,
    db_id: i64,
    table: &TableInfo,
) -> Result<i64, CommonError> {
    let mut max_next_id = 1;
    for allocator in GetGlobalAutoIDAlloc(requirement, db_id, table)? {
        max_next_id = max_next_id.max(allocator.NextGlobalAutoID()?);
    }
    Ok(max_next_id - 1)
}

/// 按 `bases` 中给出的各类型 base，对该表对应分配器执行 rebase（不预分配 ID）。
pub fn RebaseTableAllocators(
    ctx: &Context,
    bases: &HashMap<AllocatorType, i64>,
    requirement: Option<&dyn AutoIDRequirement>,
    db_id: i64,
    table: &TableInfo,
) -> Result<(), CommonError> {
    for allocator in GetGlobalAutoIDAlloc(requirement, db_id, table)? {
        if let Some(base) = bases.get(&allocator.GetType()) {
            allocator.Rebase(ctx, *base, false)?;
        }
    }
    Ok(())
}

/// 根据表结构创建所需的全局分配器列表。
///
/// 有 AutoRowID 或 AutoIncrement 时优先走 RowID/可选独立 AutoIncrement；
/// 否则若仅有 AutoRandom 则只建 AutoRandom 分配器。`no_cache=2` 表示几乎不缓存，
/// 便于导入后精确 rebase。
pub fn GetGlobalAutoIDAlloc(
    requirement: Option<&dyn AutoIDRequirement>,
    db_id: i64,
    table: &TableInfo,
) -> Result<Vec<Arc<dyn Allocator>>, CommonError> {
    let requirement = requirement.ok_or_else(|| {
        CommonError::new("internal", "internal error: kv store should not be nil")
    })?;
    if !requirement.StoreAvailable() {
        return Err(CommonError::new(
            "internal",
            "internal error: kv store should not be nil",
        ));
    }
    if db_id == 0 {
        return Err(CommonError::new(
            "internal",
            "internal error: dbID should not be 0",
        ));
    }

    // cache_step=2 近似“不缓存”，导入后 rebase 更精确。
    let no_cache = 2;
    if TableHasAutoRowID(table) || table.HasAutoIncrement {
        let mut allocators = Vec::<Arc<dyn Allocator>>::with_capacity(2);
        // SeparateAutoIncrement：AUTO_INCREMENT 与 RowID 分属不同分配器。
        if table.SeparateAutoIncrement && table.HasAutoIncrement {
            allocators.push(requirement.NewAllocator(
                db_id,
                table.ID,
                table.AutoIncrementUnsigned,
                AllocatorType::AutoIncrement,
                1,
                table.Version,
            ));
        }
        allocators.push(requirement.NewAllocator(
            db_id,
            table.ID,
            table.AutoIncrementUnsigned,
            AllocatorType::RowID,
            no_cache,
            table.Version,
        ));
        return Ok(allocators);
    }

    if table.HasAutoRandom {
        return Ok(vec![requirement.NewAllocator(
            db_id,
            table.ID,
            table.AutoRandomUnsigned,
            AllocatorType::AutoRandom,
            no_cache,
            table.Version,
        )]);
    }

    Err(CommonError::new(
        "internal",
        format!("internal error: table {} has no auto ID", table.Name),
    ))
}
