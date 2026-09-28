// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

// ingest 引擎管理器：封装对 `BackendContext` 的引擎注册与注销。
//
// DDL（Data Definition Language，数据定义语言）快速加索引时，需要为每个
// 待构建索引创建本地 ingest 写入引擎。本模块提供对外的便捷入口：
// - `register_engines`：按索引 ID 批量注册引擎；
// - `finish_and_unregister_engines`：完成导入后按选项关闭、清理或查重。
//
// `UnregisterOpt` 使用位标志（bit flag）组合多种注销行为。

use crate::backend::BackendContext;
use crate::engine::{Engine, EngineInfo};
use std::sync::Arc;
/// 注销引擎时的选项位标志类型。
pub type UnregisterOpt = u8;
/// 注销时关闭（close）已注册的引擎。
pub const OPT_CLOSE_ENGINES: UnregisterOpt = 1 << 0;
/// 注销时清理（clean）引擎中缓冲的本地数据。
pub const OPT_CLEAN_DATA: UnregisterOpt = 1 << 1;
/// 注销时检查唯一索引是否存在重复键（duplicate key）。
pub const OPT_CHECK_DUP: UnregisterOpt = 1 << 2;
/// 在后端上下文中为给定索引 ID 列表注册写入引擎。
///
/// `unique` 与 `index_ids` 一一对应，标记各索引是否为唯一索引；
/// `writer_memory` 为单个写入器预留的内存字节数。
pub fn register_engines(
    backend: &mut BackendContext,
    index_ids: &[i64],
    unique: &[bool],
    writer_memory: i64,
) -> Result<Vec<Arc<EngineInfo>>, String> {
    backend.register(index_ids, unique, writer_memory)
}
/// 完成导入并按选项注销引擎。
///
/// 根据 `options` 位标志决定是否清理本地数据、是否在注销前做重复键检查。
pub fn finish_and_unregister_engines(
    backend: &mut BackendContext,
    options: UnregisterOpt,
) -> Result<(), String> {
    if backend.engines.is_empty() {
        return Ok(());
    }

    let cleanup = options & OPT_CLEAN_DATA != 0;
    for engine in backend.engines.values() {
        engine.close(cleanup);
    }

    if options & OPT_CHECK_DUP != 0 {
        for index_id in backend.engines.keys().copied().collect::<Vec<_>>() {
            if !backend.collect_remote_duplicate_rows(index_id)?.is_empty() {
                return Err(format!("duplicate rows for index {index_id}"));
            }
        }
    }

    backend.engines.clear();
    Ok(())
}
