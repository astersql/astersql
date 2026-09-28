// Copyright 2025 PingCAP, Inc.
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

// 本文件由 pkg/infoschema/issyncer/mdldef/mdl.go 迁移而来。

// JobMDL：单个 DDL 作业的 MDL（Metadata Lock，元数据锁）状态。
//
// 记录作业要求的最低 schema 版本，以及必须满足该版本的相关表 ID 集合。
// 推进 DDL 下一阶段前，访问这些表的会话都须已加载到不低于 `ver` 的版本。

use std::collections::HashSet;

// JobMDL 对应 Go 的同名结构。单独放在 mdldef 子目录是为了延续 Go 避免循环依赖的设计。
/// 单个 DDL 作业关联的 MDL 约束：最低 schema 版本 + 涉及表集合。
#[derive(Debug, Default)]
pub struct JobMDL {
    // ver 是该作业要求当前实例至少加载到的 schema 版本。
    /// 作业要求实例至少加载到的 schema 元版本。
    pub ver: i64,
    // table_ids 记录作业涉及的表；推进下一阶段前，访问这些表的会话都必须使用不低于 ver 的版本。
    /// 作业涉及的物理表 ID；这些表上的会话须使用 ≥ `ver` 的 schema。
    pub table_ids: HashSet<i64>,
}
