// Copyright 2021 PingCAP, Inc.
// Copyright 2026 AsterSQL.
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

// 临时表会话态接口与可替换工厂。
//
// 对应 Go `pkg/util/tableutil`：全局/局部临时表在会话内需独立的 autoID
//（自动递增 ID）、统计信息与修改标记，避免多会话共享。工厂变量对齐 Go
// 可赋值函数变量；未初始化时调用会 panic。

#![allow(dead_code)]
#![allow(non_snake_case)]

use std::any::Any;
use std::sync::{Arc, LazyLock, RwLock};

use crate::{autoid, model};

// TempTable is used to store transaction-specific or session-specific information for global / local temporary tables.
// For example, stats and autoID should have their own copies of data, instead of being shared by all sessions.
/// 临时表会话副本：独立 autoID、统计、大小与修改标记。
pub trait TempTable: Send + Sync {
    // GetAutoIDAllocator gets the autoID allocator of this table.
    /// 获取本表会话级 autoID 分配器。
    fn GetAutoIDAllocator(&self) -> Arc<dyn autoid::Allocator>;

    // SetModified sets that the table is modified.
    /// 标记表在本会话中已被修改。
    fn SetModified(&mut self, modified: bool);

    // GetModified queries whether the table is modified.
    /// 查询本会话是否已修改该表。
    fn GetModified(&self) -> bool;

    // The stats of this table (*statistics.Table).
    // Define the return type as interface{} here to avoid cycle imports.
    /// 会话级统计对象（Go 为 `*statistics.Table`，此处用 Any 避免循环依赖）。
    fn GetStats(&self) -> Arc<dyn Any + Send + Sync>;

    /// 会话视角下的表大小。
    fn GetSize(&self) -> i64;

    /// 更新会话视角下的表大小。
    fn SetSize(&mut self, size: i64);

    /// 返回表元数据（`TableInfo`）。
    fn GetMeta(&self) -> &model::TableInfo;
}

// TempTableFromMeta builds a TempTable from *model.TableInfo.
// Currently, it is assigned to tables.TempTableFromMeta in tidb package's init function.
/// 由 `TableInfo` 构造 `TempTable` 的工厂类型；对应 Go 可赋值函数变量。
pub type TempTableFactory = Arc<dyn Fn(Arc<model::TableInfo>) -> Box<dyn TempTable> + Send + Sync>;

/// 包级工厂槽位：读写锁保护，对齐 Go 全局函数变量的并发可见性。
static TEMP_TABLE_FROM_META: LazyLock<RwLock<Option<TempTableFactory>>> =
    LazyLock::new(|| RwLock::new(None));

/// Replaces the package-level constructor, matching assignment to the Go
/// function variable. The previous constructor is returned so tests or
/// embedders can restore it without racing through an unsafe mutable static.
/// 替换包级构造工厂并返回旧值，便于测试恢复。
pub fn SetTempTableFromMeta(factory: Option<TempTableFactory>) -> Option<TempTableFactory> {
    let mut slot = TEMP_TABLE_FROM_META
        .write()
        .expect("temporary table factory lock poisoned");
    std::mem::replace(&mut *slot, factory)
}

/// Calls the constructor installed by `SetTempTableFromMeta`.
///
/// Go panics when an uninitialized function variable is called, so the Rust
/// implementation intentionally keeps the same failure behavior.
/// 调用已注册工厂；未初始化时 panic，对齐 Go 空函数变量行为。
pub fn TempTableFromMeta(tblInfo: Arc<model::TableInfo>) -> Box<dyn TempTable> {
    let factory = TEMP_TABLE_FROM_META
        .read()
        .expect("temporary table factory lock poisoned")
        .clone()
        .expect("TempTableFromMeta is not initialized");
    factory(tblInfo)
}
