// Copyright 2024 PingCAP, Inc.
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

// Schema 变更事件发布入口。
//
// 将一次 DDL job（或其子 job）产生的 `SchemaChangeEvent` 写入 notifier 存储。
// `processedByFlag` 用位图标记哪些订阅方已处理该事件，便于多订阅者可靠投递。

use crate::{Error, SchemaChangeEvent, Session, Store};

/// 通过给定 store session 暂存或自动提交一条 schema 变更记录。
///
/// `ddl_job_id` / `sub_job_id` 标识父 DDL 任务与 multi-schema 子任务；普通 DDL
/// 的 `sub_job_id` 为 -1，多 schema 变更或批量建表时则为子任务索引。
/// 新写入时 `processedByFlag` 置 0，表示尚未被任何订阅方处理。
/// Stages or autocommits a schema change through the supplied store session.
pub fn PubSchemeChangeToStore(
    session: &Session,
    ddl_job_id: i64,
    sub_job_id: i64,
    event: SchemaChangeEvent,
    store: &dyn Store,
) -> Result<(), Error> {
    store.Insert(
        session,
        &SchemaChange {
            ddlJobID: ddl_job_id,
            subJobID: sub_job_id,
            event,
            processedByFlag: 0,
        },
    )
}

/// 持久化中的一条 schema 变更记录：关联 DDL job、事件载荷与处理位图。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SchemaChange {
    /// 父 DDL job 的全局 ID。
    pub ddlJobID: i64,
    /// multi-schema change 中的子 job ID；普通 DDL 为 -1。
    pub subJobID: i64,
    /// 具体的 schema 变更事件载荷。
    pub event: SchemaChangeEvent,
    /// 订阅方处理完成位图：每位对应一个 handler，置位表示已处理。
    pub processedByFlag: u64,
}
