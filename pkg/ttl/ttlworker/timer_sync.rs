// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
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

// TTL 定时器记录与 infoschema 物理表的同步器。
//
// 将开启 TTL 的物理表映射为定时器记录（键、标签、调度间隔），缓存本地视图，
// 并对已消失的表做延迟删除，避免 schema 抖动导致定时器被立刻清掉。

use std::collections::{BTreeMap, BTreeSet};

use crate::session::PhysicalTable;

/// 定时器键前缀；完整键为 `{prefix}{physical_id}`。
pub const TIMER_KEY_PREFIX: &str = "/tidb/ttl/physical_table/";
/// 注册到定时器框架的钩子类别名。
pub const TIMER_HOOK_CLASS: &str = "tidb.ttl";
/// 历史默认的作业调度间隔（24 小时）。
pub const OLD_DEFAULT_TTL_JOB_INTERVAL: &str = "24h";

/// 定时器载荷中绑定的表标识。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TtlTimerData {
    /// 逻辑表 ID。
    pub table_id: i64,
    /// 物理表 ID。
    pub physical_id: i64,
}

/// 一条 TTL 定时器记录的本地缓存形态。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TimerRecord {
    /// 定时器记录 ID。
    pub id: String,
    /// 全局唯一键。
    pub key: String,
    /// 钩子类别。
    pub hook_class: String,
    /// 便于检索的标签（表 ID、schema 名等）。
    pub tags: Vec<String>,
    /// 调度间隔字符串，如 `"24h"`。
    pub schedule_interval: String,
    /// 关联的表数据。
    pub data: TtlTimerData,
    /// 是否启用。
    pub enabled: bool,
    /// 标记删除时间；`None` 表示仍存活。
    pub deleted_at: Option<u64>,
    /// 创建时间；Go 版以 timer 的 CreateTime 判断失踪记录是否已超过删除宽限期。
    created_at: u64,
}

/// 将物理表列表同步到本地定时器缓存的同步器。
#[derive(Clone, Debug, Default)]
pub struct TtlTimersSyncer {
    /// 按定时器键索引的缓存。
    cached: BTreeMap<String, TimerRecord>,
    /// 上次同步时间。
    last_sync_time: u64,
    /// 上次同步时的 schema 版本。
    last_sync_version: i64,
    /// 标记删除后保留多久再真正从缓存移除（秒）。
    delay_delete_seconds: u64,
}

impl TtlTimersSyncer {
    /// 创建同步器，默认延迟删除 600 秒。
    pub fn new() -> Self {
        Self {
            delay_delete_seconds: 600,
            ..Self::default()
        }
    }
    /// 设置延迟删除间隔。
    pub fn set_delay_delete_interval(&mut self, seconds: u64) {
        self.delay_delete_seconds = seconds;
    }
    /// 清空缓存与同步元信息。
    pub fn reset(&mut self) {
        self.cached.clear();
        self.last_sync_time = 0;
        self.last_sync_version = 0;
    }
    /// 返回上次同步的 `(时间, schema 版本)`。
    pub fn last_sync_info(&self) -> (u64, i64) {
        (self.last_sync_time, self.last_sync_version)
    }
    /// 按键查询缓存中的定时器记录。
    pub fn cached_timer(&self, key: &str) -> Option<&TimerRecord> {
        self.cached.get(key)
    }
    /// 根据当前启用 TTL 的物理表刷新缓存：upsert、标记删除、延迟清理。
    pub fn sync_timers(
        &mut self,
        tables: &[PhysicalTable],
        schema_version: i64,
        now: u64,
        interval: impl Fn(&PhysicalTable) -> Option<String>,
    ) {
        let mut live = BTreeSet::new();
        // `tables` 对应 Go `ListTablesWithSpecialAttribute(TTLAttribute)` 的输出：
        // 其中只有已定义 TTL 的表，但也包含 TTL_ENABLE=OFF 的表。
        for table in tables {
            let key = timer_key(table.table_id, table.physical_id);
            live.insert(key.clone());
            let schedule_interval = interval(table)
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| OLD_DEFAULT_TTL_JOB_INTERVAL.to_owned());
            let mut next = TimerRecord {
                id: format!("ttl-{}", table.physical_id),
                key: key.clone(),
                hook_class: TIMER_HOOK_CLASS.to_owned(),
                tags: timer_tags(table),
                schedule_interval,
                data: TtlTimerData {
                    table_id: table.table_id,
                    physical_id: table.physical_id,
                },
                enabled: table.ttl_enabled,
                deleted_at: None,
                created_at: now,
            };
            match self.cached.get_mut(&key) {
                Some(record) => {
                    // 当前 key 再次出现在 infoschema 中时，Go 版不会把它当作待删除记录；
                    // 更新 timer 也不会改变原始 CreateTime。
                    next.created_at = record.created_at;
                    if should_sync_timer(record, &next) || record.deleted_at.is_some() {
                        *record = next;
                    }
                }
                None => {
                    self.cached.insert(key, next);
                }
            }
        }
        // Go 版按 timer CreateTime（不是首次发现表消失的时间）应用删除宽限期：
        // 已足够老的记录立即删除，较新的记录先禁用并保留。
        self.cached.retain(|key, record| {
            if live.contains(key) {
                return true;
            }
            if now.saturating_sub(record.created_at) > self.delay_delete_seconds {
                return false;
            }
            if record.deleted_at.is_none() {
                record.enabled = false;
                record.deleted_at = Some(now);
            }
            true
        });
        self.last_sync_time = now;
        self.last_sync_version = schema_version;
    }
    /// 手动触发：返回缓存中启用的定时器 ID 与请求 ID；禁用或不存在则报错。
    pub fn manual_trigger(
        &self,
        table: &PhysicalTable,
        request_id: &str,
    ) -> Result<(String, String), String> {
        let record = self
            .cached
            .get(&timer_key(table.table_id, table.physical_id))
            .ok_or_else(|| "timer not found".to_owned())?;
        if !record.enabled {
            return Err("manual trigger is not allowed when timer is disabled".to_owned());
        }
        Ok((record.id.clone(), request_id.to_owned()))
    }
}

/// 由物理表 ID 生成定时器键。
pub fn timer_key(table_id: i64, physical_id: i64) -> String {
    format!("{TIMER_KEY_PREFIX}{table_id}/{physical_id}")
}
/// 为物理表生成检索标签列表。
pub fn timer_tags(table: &PhysicalTable) -> Vec<String> {
    vec![
        format!("db={}", table.schema),
        format!("table={}", table.table),
    ]
}
/// 判断缓存记录是否需要用期望值覆盖（标签、间隔、数据变更或当前已禁用）。
pub fn should_sync_timer(current: &TimerRecord, desired: &TimerRecord) -> bool {
    current.tags != desired.tags
        || current.schedule_interval != desired.schedule_interval
        || current.data != desired.data
        || current.enabled != desired.enabled
}
