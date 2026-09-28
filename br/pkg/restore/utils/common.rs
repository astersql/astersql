// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
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

//! 恢复过程中「已建表、尚未灌数」的中间结构，对齐 Go `common.go`。
//! CreatedTable 把新表元数据、备份侧旧表与键重写规则绑在一起，
//! 供后续 SST/日志导入阶段按规则改写 table/index 前缀。

//! CreatedTable and related restore utils types matching `common.go`.

use crate::rewrite_rule::RewriteRules;
use crate::stubs::{metautil, model};

/// 恢复建表后的占位：RewriteRule 映射旧→新键前缀；
/// Table 为集群侧新表元数据，OldTable 保留备份侧表信息以便对照分区/索引 ID。
/// CreatedTable is a table created on restore process, but not yet filled with data.
pub struct CreatedTable {
    pub RewriteRule: Option<Box<RewriteRules>>,
    pub Table: Option<Box<model::TableInfo>>,
    pub OldTable: Option<Box<metautil::Table>>,
}
