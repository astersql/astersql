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

#[derive(Clone, Debug, Default)]
pub struct DatabaseRestorePlan {
    pub Source: metautil::Database,
    pub Target: model::DBInfo,
    pub Reused: bool,
}

#[derive(Clone, Debug, Default)]
pub struct TableRestorePlan {
    pub Source: metautil::Table,
    pub TargetDB: model::DBInfo,
    pub TargetInfo: model::TableInfo,
}

impl TableRestorePlan {
    pub fn TargetTable(&self) -> metautil::Table {
        metautil::Table {
            DB: self.TargetDB.clone(),
            Info: self.TargetInfo.clone(),
        }
    }
}

/// 恢复建表后的占位：RewriteRule 映射旧→新键前缀；
/// Table 为集群侧新表元数据，OldTable 保留备份侧表信息以便对照分区/索引 ID。
/// CreatedTable is a table created on restore process, but not yet filled with data.
pub struct CreatedTable {
    pub RewriteRule: Option<Box<RewriteRules>>,
    pub Table: Option<Box<model::TableInfo>>,
    pub OldTable: Option<Box<metautil::Table>>,
    pub TargetDB: model::CIStr,
}

impl CreatedTable {
    pub fn TargetDBName(&self) -> &model::CIStr {
        if self.TargetDB.O.is_empty() {
            self.OldTable
                .as_ref()
                .map(|table| &table.DB.Name)
                .unwrap_or(&self.TargetDB)
        } else {
            &self.TargetDB
        }
    }
}

pub fn DatabaseSettingsCompatible(first: &model::DBInfo, second: &model::DBInfo) -> bool {
    first.Charset.eq_ignore_ascii_case(&second.Charset)
        && first.Collate.eq_ignore_ascii_case(&second.Collate)
        && first.PlacementPolicyRef.as_ref().map(|policy| &policy.L)
            == second.PlacementPolicyRef.as_ref().map(|policy| &policy.L)
}

pub fn SourceDBPrecedes(candidate: &model::DBInfo, existing: &model::DBInfo) -> bool {
    candidate.Name.L < existing.Name.L
        || (candidate.Name.L == existing.Name.L && candidate.ID < existing.ID)
}
