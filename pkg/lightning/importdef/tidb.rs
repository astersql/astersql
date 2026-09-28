// Copyright 2019 PingCAP, Inc.
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

// Lightning 导入用的 TiDB 库表元数据定义。
//
// `DBInfo` / `TableInfo` 对应 Go 侧同名结构，描述一次导入面向的数据库与表，
// 并区分当前生效元数据（Core）与导入完成后的期望元数据（Desired）。

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use crate::model;

/// Go `map[string]*TableInfo` 中的表指针。
pub type TableInfoRef = Arc<RwLock<TableInfo>>;
/// Go 表 map 的已初始化内容。
pub type TableInfoMap = HashMap<String, TableInfoRef>;
/// Go map 是引用类型；克隆 `DBInfo` 后仍共享同一张 map。
pub type TableInfoMapRef = Arc<RwLock<TableInfoMap>>;
/// Go `*model.TableInfo`：可为多个字段或结构副本共享并修改。
pub type ModelTableInfoRef = Arc<RwLock<model::TableInfo>>;

/// `DBInfo` 对应 Go 的同名结构，描述一次导入所面向的 TiDB 数据库。
#[derive(Clone, Default)]
pub struct DBInfo {
    /// 数据库 ID（schema ID）。
    pub ID: i64,
    /// 数据库名。
    pub Name: String,
    /// 表名 → 表信息；`None` 对应 Go nil map，`Some` 对应已初始化 map。
    pub Tables: Option<TableInfoMapRef>,
}

/// `TableInfo` 同时保存当前 TiDB 表定义和导入完成后希望得到的表定义。
#[derive(Clone, Default)]
pub struct TableInfo {
    /// 表 ID。
    pub ID: i64,
    /// 所属数据库名。
    pub DB: String,
    /// 表名。
    pub Name: String,
    /// `Core` 是 TiDB 中当前生效的表元数据；Option 对应 Go 指针可能为 nil。
    pub Core: Option<ModelTableInfoRef>,
    /// `Desired` 是迁移目标表元数据，通常与 Core 相同。
    /// 分离导入索引和数据时，Core 可暂不含索引，而 Desired 仍保留完整索引定义。
    pub Desired: Option<ModelTableInfoRef>,
}
