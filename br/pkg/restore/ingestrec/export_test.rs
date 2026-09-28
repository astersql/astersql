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

//! Go-equivalent export helpers from `export_test.go` (same-package test accessors).
//! 为 ingestrec 测试暴露 FK 记录 map 只读访问器，对齐 Go export_test。
//! 不改动生产 API；仅同包测试可见的辅助方法。
//! IngestRecorder 与 Go 一样借用 manager 的实时 map；manager 缺失时 panic。
//! Table/ForeignKey 两级 manager 各自暴露正向/反向映射。
//! 与 Go 方法名一致，便于对照单测迁移。

use std::collections::HashMap;

use crate::{
    ForeignKeyRecord, ForeignKeyRecordKey, ForeignKeyRecordManager, IngestRecorder,
    TableForeignKeyRecordManager,
};

impl TableForeignKeyRecordManager {
    /// Go `(*TableForeignKeyRecordManager).GetFKRecordMap`.
    /// 表级外键记录正向映射。
    pub fn GetFKRecordMap(&self) -> &HashMap<ForeignKeyRecordKey, ForeignKeyRecord> {
        &self.fkRecordMap
    }

    /// Go `(*TableForeignKeyRecordManager).GetReferredFKRecordMap`.
    /// 被引用侧映射，用于校验反向依赖。
    pub fn GetReferredFKRecordMap(&self) -> &HashMap<ForeignKeyRecordKey, ForeignKeyRecord> {
        &self.referredFKRecordMap
    }
}

impl ForeignKeyRecordManager {
    /// Go `(*ForeignKeyRecordManager).GetFKRecordMap`.
    /// 全局 FK 记录映射只读视图。
    pub fn GetFKRecordMap(&self) -> &HashMap<ForeignKeyRecordKey, ForeignKeyRecord> {
        &self.fkRecordMap
    }
}

impl IngestRecorder {
    /// Go `(*IngestRecorder).GetFKRecordMap`.
    /// 直接借用内部 map；Go 在 manager 为 nil 时也会解引用失败。
    pub fn GetFKRecordMap(&self) -> &HashMap<ForeignKeyRecordKey, ForeignKeyRecord> {
        self.foreignKeyRecordManager
            .as_ref()
            .map(|m| &m.fkRecordMap)
            .expect("foreignKeyRecordManager is nil")
    }
}
