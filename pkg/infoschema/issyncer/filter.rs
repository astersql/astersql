// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
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

// InfoSchema 同步器（issyncer）的加载过滤器接口。
//
// 对应 Go `issyncer.Filter`：调用方可定制同步时跳过哪些 schema diff 或整库加载。

// Ported from pkg/infoschema/issyncer/filter.go.
use crate::{DBInfo, SchemaDiff, SchemaInfo};

/// Filter allows callers to customize which schema objects should be loaded
/// when syncing the information schema. Mirrors Go's `issyncer.Filter`.
///
/// The semantics of the return values are:
///   - true:  skip the corresponding operation
///   - false: continue the default loading logic
///
/// 同步 InfoSchema 时自定义跳过逻辑；返回 true 表示跳过，false 表示继续默认加载。
/// SchemaDiff：一次 DDL 导致的模式变更描述；SchemaInfo：某一版本的完整模式快照。
pub trait Filter: Send + Sync {
    /// SkipLoadDiff returns true when the given schema diff should be ignored.
    /// `latestIS` is the newest schema cached by the loader; it is `None` when
    /// the loader hasn't loaded any schema yet.
    /// 为 true 时忽略该 SchemaDiff；`latestIS` 为加载器缓存的最新模式，尚未加载时为 None。
    fn SkipLoadDiff(&self, diff: &SchemaDiff, latestIS: Option<&SchemaInfo>) -> bool;

    /// SkipLoadSchema returns true when the given DB should not be loaded
    /// during a full schema load.
    /// 全量加载时为 true 则跳过该库（DBInfo 为库级元数据）。
    fn SkipLoadSchema(&self, dbInfo: Option<&DBInfo>) -> bool;
}
