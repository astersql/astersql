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
// 会话表锁上下文接口定义。
//
// 对应 Go `lock/context`：只读查询与可写维护会话持有的表锁映射；
// 实现方通常是 session，由上层 `LOCK TABLES` 与表锁检查器调用。

// 本文件由 pkg/lock/context/lockcontext.go 迁移而来，保留会话表锁读写接口的嵌入关系。
// 接口只规定查询和维护会话锁映射的契约，不会获取真实锁、访问数据库或执行任何业务动作。
// 所有权形状使用切片表达 Go slice 输入，并以 Vec 表达由调用方拥有的返回值。

use crate::{ast, model};

/// TableLockReadContext 对应 Go 的只读接口，用于查询当前会话持有的表锁。
pub trait TableLockReadContext {
    /// CheckTableLocked 检查表 ID 是否已锁定，并返回对应的锁类型。
    fn CheckTableLocked(&self, tbl_id: i64) -> (bool, ast::TableLockType);

    /// GetAllTableLocks 返回会话持有的全部锁，元素同时包含 table ID 与 database ID。
    fn GetAllTableLocks(&self) -> Vec<model::TableLockTpInfo>;

    /// HasLockedTables 判断当前会话是否持有任意表锁。
    fn HasLockedTables(&self) -> bool;
}

/// TableLockContext 对应 Go 嵌入 TableLockReadContext 的写接口，并补充锁映射变更操作。
pub trait TableLockContext: TableLockReadContext {
    /// AddTableLock 把一批表锁加入当前会话的锁映射。
    fn AddTableLock(&mut self, locks: &[model::TableLockTpInfo]);

    /// ReleaseTableLocks 按完整锁信息从会话映射释放指定表锁。
    fn ReleaseTableLocks(&mut self, locks: &[model::TableLockTpInfo]);

    /// ReleaseTableLockByTableIDs 只按 table ID 批量释放锁。
    fn ReleaseTableLockByTableIDs(&mut self, table_ids: &[i64]);

    /// ReleaseAllTableLocks 释放当前会话持有的全部表锁。
    fn ReleaseAllTableLocks(&mut self);
}
