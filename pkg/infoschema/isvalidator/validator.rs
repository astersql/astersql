// Copyright 2016 PingCAP, Inc.
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

// Schema Validator：校验事务使用的 schema 版本是否仍在租约（lease）内、
// 以及相关物理表是否在更新后发生过 DDL 变更。
//
// 对应 TiDB `infoschema/isvalidator`。事务在两阶段提交（2PC）前调用 `check`：
// 若 schema 已变且涉及本事务表则 Fail；若租约过期则 Unknown（需向 PD 重新确认）。
// 内部用有界 delta 队列记录近期 schema 变更历史。

use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::sync::{RwLock, RwLockReadGuard, RwLockWriteGuard};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::logutil;
use crate::validatorapi::Validator as ValidatorApi;
use crate::vardef;

pub use crate::validatorapi::Result;

/// TSO（Timestamp Oracle）物理时间左移位数：低 18 位为逻辑时钟。
const PHYSICAL_SHIFT_BITS: u32 = 18;

/// RelatedSchemaChange is the Rust counterpart of TiKV client-go's transaction
/// schema-change payload.
/// 事务携带的相关 schema 变更：物理表 ID 与 DDL action 类型一一对应。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RelatedSchemaChange {
    /// 物理表 ID 列表（含分区表的物理 ID）。
    pub phy_tbl_ids: Vec<i64>,
    /// 与 `phy_tbl_ids` 等长的 DDL action 类型编码。
    pub action_types: Vec<u64>,
}

/// DeltaSchemaInfo records one schema version and the physical tables/actions
/// changed by that version.
/// 单个 schema 版本的增量摘要，入队供后续相关表判定。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DeltaSchemaInfo {
    /// 该增量对应的 schema 版本号。
    pub schema_version: i64,
    /// 本版本改动过的物理表 ID。
    pub related_ids: Vec<i64>,
    /// 与 `related_ids` 对齐的 action 类型。
    pub related_actions: Vec<u64>,
}

/// Validator 内部可变状态，由 `RwLock` 保护。
#[derive(Debug)]
struct ValidatorState {
    /// 是否处于可服务状态；stop 后为 false。
    is_started: bool,
    /// 最近一次 update 写入的最新 schema 版本。
    latest_schema_ver: i64,
    /// reconnect/PD 恢复后加载的最低可用版本；更旧事务直接 Fail。
    restart_schema_ver: i64,
    /// 当前租约过期墙钟时间。
    latest_schema_expire: SystemTime,
    // The queue is ordered by schema version in ascending order.
    // 按 schema 版本升序排列的有界增量队列。
    delta_schema_infos: Vec<DeltaSchemaInfo>,
}

/// ValidatorSnapshot exposes a consistent, read-only state image for
/// diagnostics and focused tests.
/// 只读快照，便于诊断与单测断言。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ValidatorSnapshot {
    pub is_started: bool,
    pub latest_schema_ver: i64,
    pub restart_schema_ver: i64,
    pub latest_schema_expire: SystemTime,
    pub delta_schema_infos: Vec<DeltaSchemaInfo>,
}

/// Validator implements TiDB's schema lease and delta-history checks.
/// 实现 schema 租约与 delta 历史校验的核心结构。
#[derive(Debug)]
pub struct Validator {
    /// schema lease 时长；update 时据此计算 `latest_schema_expire`。
    lease: Duration,
    state: RwLock<ValidatorState>,
}

/// ValidatorMetricsSnapshot mirrors the five schema-validator counter labels
/// plus the lease-expiry gauge updated by the Go implementation. The package
/// integration layer can export this snapshot through its metrics backend.
/// 五个计数器标签 + 租约过期 Unix 秒的只读镜像，供上层导出 Prometheus 指标。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ValidatorMetricsSnapshot {
    pub stop: u64,
    pub restart: u64,
    pub reset: u64,
    pub cache_empty: u64,
    pub cache_miss: u64,
    pub lease_expire_unix: i64,
}

/// 进程内原子计数器，对应 Go 侧 schema validator 指标。
struct ValidatorMetrics {
    stop: AtomicU64,
    restart: AtomicU64,
    reset: AtomicU64,
    cache_empty: AtomicU64,
    cache_miss: AtomicU64,
    lease_expire_unix: AtomicI64,
}

static VALIDATOR_METRICS: ValidatorMetrics = ValidatorMetrics {
    stop: AtomicU64::new(0),
    restart: AtomicU64::new(0),
    reset: AtomicU64::new(0),
    cache_empty: AtomicU64::new(0),
    cache_miss: AtomicU64::new(0),
    lease_expire_unix: AtomicI64::new(0),
};

/// 读取当前指标快照（Relaxed 即可，诊断用）。
pub fn validator_metrics_snapshot() -> ValidatorMetricsSnapshot {
    ValidatorMetricsSnapshot {
        stop: VALIDATOR_METRICS.stop.load(Ordering::Relaxed),
        restart: VALIDATOR_METRICS.restart.load(Ordering::Relaxed),
        reset: VALIDATOR_METRICS.reset.load(Ordering::Relaxed),
        cache_empty: VALIDATOR_METRICS.cache_empty.load(Ordering::Relaxed),
        cache_miss: VALIDATOR_METRICS.cache_miss.load(Ordering::Relaxed),
        lease_expire_unix: VALIDATOR_METRICS.lease_expire_unix.load(Ordering::Relaxed),
    }
}

/// Converts a TiKV TSO timestamp to wall-clock time, matching
/// oracle.GetTimeFromTS (the low 18 logical bits are ignored).
/// 将 TSO 转为墙钟时间：右移丢掉逻辑时钟部分。
pub fn time_from_ts(ts: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_millis(ts >> PHYSICAL_SHIFT_BITS)
}

/// Converts wall-clock time to a TiKV TSO with logical part zero, matching
/// oracle.GoTimeToTS.
/// 墙钟转 TSO：毫秒物理时间左移，逻辑部分为 0。
pub fn system_time_to_ts(time: SystemTime) -> u64 {
    let millis = time
        .duration_since(UNIX_EPOCH)
        .expect("TiKV timestamps cannot represent time before the Unix epoch")
        .as_millis();
    let physical = u64::try_from(millis).expect("timestamp milliseconds exceed u64");
    physical
        .checked_mul(1_u64 << PHYSICAL_SHIFT_BITS)
        .expect("timestamp exceeds TiKV TSO range")
}

/// SystemTime 转 Unix 秒；早于 epoch 时返回负值。
pub(crate) fn unix_seconds(time: SystemTime) -> i64 {
    match time.duration_since(UNIX_EPOCH) {
        Ok(duration) => i64::try_from(duration.as_secs()).unwrap_or(i64::MAX),
        Err(error) => {
            let duration = error.duration();
            let seconds = i128::from(duration.as_secs()) + i128::from(duration.subsec_nanos() != 0);
            i64::try_from(-seconds).unwrap_or(i64::MIN)
        }
    }
}

/// 读锁；poison 时取出内层状态以免测试 panic 级联。
fn read_state(lock: &RwLock<ValidatorState>) -> RwLockReadGuard<'_, ValidatorState> {
    lock.read().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// 写锁；poison 处理同 `read_state`。
fn write_state(lock: &RwLock<ValidatorState>) -> RwLockWriteGuard<'_, ValidatorState> {
    lock.write()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn log_info(message: &str) {
    logutil::log::BgLogger().info(message);
}

fn log_info_fields(message: &str, fields: Vec<logutil::log::LogField>) {
    logutil::log::BgLogger().log(logutil::log::LogLevel::Info, message, fields);
}

fn log_debug_fields(message: &str, fields: Vec<logutil::log::LogField>) {
    logutil::log::BgLogger().log(logutil::log::LogLevel::Debug, message, fields);
}

/// new corresponds to Go's New.
/// 对应 Go `New`：按租约时长构造 Validator。
pub fn new(lease: Duration) -> Box<Validator> {
    Validator::new(lease)
}

impl Validator {
    /// 构造并立即处于 started 状态。
    ///
    /// Go 仅在 `intest` / `enableassert` 构建中断言 lease 大于 0；默认构建
    /// 接受零租约，因此这里不做无条件断言。
    pub fn new(lease: Duration) -> Box<Self> {
        Box::new(Self {
            lease,
            state: RwLock::new(ValidatorState {
                is_started: true,
                latest_schema_ver: 0,
                restart_schema_ver: 0,
                latest_schema_expire: UNIX_EPOCH,
                delta_schema_infos: Vec::with_capacity(
                    vardef::DefTiDBMaxDeltaSchemaCount.max(0) as usize
                ),
            }),
        })
    }

    /// 复制一份一致的只读状态快照。
    pub fn snapshot(&self) -> ValidatorSnapshot {
        let state = read_state(&self.state);
        ValidatorSnapshot {
            is_started: state.is_started,
            latest_schema_ver: state.latest_schema_ver,
            restart_schema_ver: state.restart_schema_ver,
            latest_schema_expire: state.latest_schema_expire,
            delta_schema_infos: state.delta_schema_infos.clone(),
        }
    }

    /// 是否已启动（未 stop）。
    pub fn is_started(&self) -> bool {
        read_state(&self.state).is_started
    }

    /// 停止校验器：清空 delta，后续 update/check 视为未知。
    pub fn stop(&self) {
        log_info("the schema validator stops");
        VALIDATOR_METRICS.stop.fetch_add(1, Ordering::Relaxed);

        let mut state = write_state(&self.state);
        state.is_started = false;
        state.latest_schema_ver = 0;
        state.delta_schema_infos.clear();
    }

    /// 重新启动并记录 reconnect 后的 schema 版本下界。
    pub fn restart(&self, curr_schema_ver: i64) {
        VALIDATOR_METRICS.restart.fetch_add(1, Ordering::Relaxed);
        log_info("the schema validator restarts");

        let mut state = write_state(&self.state);
        state.is_started = true;
        // Record the version loaded after reconnecting to PD so transactions
        // using an older version fail before commit.
        // 记录重连 PD 后加载的版本，更旧事务在提交前直接失败。
        state.restart_schema_ver = curr_schema_ver;
    }

    /// 重置为初始 started 状态（版本与队列清零）。
    pub fn reset(&self) {
        VALIDATOR_METRICS.reset.fetch_add(1, Ordering::Relaxed);

        let mut state = write_state(&self.state);
        state.is_started = true;
        state.latest_schema_ver = 0;
        state.delta_schema_infos.clear();
        state.restart_schema_ver = 0;
    }

    /// 刷新租约与最新 schema 版本；版本变化时入队 delta。
    pub fn update(
        &self,
        lease_grant_ts: u64,
        old_ver: i64,
        curr_ver: i64,
        change: Option<&RelatedSchemaChange>,
    ) {
        let mut state = write_state(&self.state);
        if !state.is_started {
            log_info("the schema validator stopped before updating");
            return;
        }

        state.latest_schema_ver = curr_ver;
        // 过期时刻 = 授予时刻 + lease - 1ms，与 Go 对齐。
        let lease_expire = time_from_ts(lease_grant_ts)
            .checked_add(self.lease)
            .and_then(|time| time.checked_sub(Duration::from_millis(1)))
            .expect("schema lease expiration is outside SystemTime range");
        state.latest_schema_expire = lease_expire;
        VALIDATOR_METRICS
            .lease_expire_unix
            .store(unix_seconds(lease_expire), Ordering::Relaxed);

        // 仅当版本前进时入队变更摘要。
        if curr_ver != old_ver {
            Self::enqueue_locked(&mut state, curr_ver, change);
            let ids = change
                .map(|value| value.phy_tbl_ids.clone())
                .unwrap_or_default();
            let actions = change
                .map(|value| value.action_types.clone())
                .unwrap_or_default();
            log_debug_fields(
                "update schema validator",
                vec![
                    logutil::log::LogField::I64("oldVer".into(), old_ver),
                    logutil::log::LogField::I64("currVer".into(), curr_ver),
                    logutil::log::LogField::String("changedTableIDs".into(), format!("{ids:?}")),
                    logutil::log::LogField::String(
                        "changedActionTypes".into(),
                        format!("{actions:?}"),
                    ),
                ],
            );
        }
    }

    /// 当前墙钟是否已超过租约过期时间。
    pub fn is_lease_expired(&self) -> bool {
        SystemTime::now() > read_state(&self.state).latest_schema_expire
    }

    /// 自 `curr_ver` 之后，事务相关表是否发生过 schema 变更。
    pub(crate) fn is_related_tables_changed(&self, curr_ver: i64, table_ids: &[i64]) -> bool {
        let state = read_state(&self.state);
        Self::is_related_tables_changed_locked(&state, curr_ver, table_ids)
    }

    /// 在已持锁状态下做相关表变更判定。
    fn is_related_tables_changed_locked(
        state: &ValidatorState,
        curr_ver: i64,
        table_ids: &[i64],
    ) -> bool {
        // 历史为空：无法证明安全，保守返回 true（已变更）。
        if state.delta_schema_infos.is_empty() {
            VALIDATOR_METRICS
                .cache_empty
                .fetch_add(1, Ordering::Relaxed);
            log_info_fields(
                "schema change history is empty",
                vec![logutil::log::LogField::I64("currVer".into(), curr_ver)],
            );
            return true;
        }

        let newer_deltas = Self::find_newer_deltas(state, curr_ver);
        if newer_deltas.len() == state.delta_schema_infos.len() {
            // Only the latest N changes are retained. If every retained item is
            // newer, an evicted delta may exist and safety cannot be proven.
            // 队列仅保留最近 N 条；若全部都比 curr_ver 新，可能已淘汰更早变更，无法证明安全。
            VALIDATOR_METRICS.cache_miss.fetch_add(1, Ordering::Relaxed);
            log_info_fields(
                "the schema version is much older than the latest version",
                vec![
                    logutil::log::LogField::I64("currVer".into(), curr_ver),
                    logutil::log::LogField::I64("latestSchemaVer".into(), state.latest_schema_ver),
                ],
            );
            return true;
        }

        // 扫描更新的 delta，收集与事务表交集的 action 位图。
        let mut changed_tables: HashMap<i64, u64> = HashMap::new();
        let mut changed_schema_versions = Vec::new();
        for item in newer_deltas {
            let mut affected = false;
            for (index, table_id) in item.related_ids.iter().enumerate() {
                for related_table_id in table_ids {
                    // -1 表示“关注所有表”（Go 侧约定）。
                    if table_id == related_table_id || *related_table_id == -1 {
                        // Go's uint64 left shift produces zero when action >= 64.
                        // Go 对 action>=64 的左移结果为 0，此处对齐。
                        let flag = item
                            .related_actions
                            .get(index)
                            .copied()
                            .map(|action| {
                                if action < u64::BITS as u64 {
                                    1_u64 << action
                                } else {
                                    0
                                }
                            })
                            .expect("related table IDs and action types must have equal length");
                        *changed_tables.entry(*table_id).or_default() |= flag;
                        affected = true;
                    }
                }
            }
            if affected {
                changed_schema_versions.push(item.schema_version);
            }
        }

        if !changed_tables.is_empty() {
            let mut ids: Vec<_> = changed_tables.keys().copied().collect();
            ids.sort_unstable();
            log_info_fields(
                "schema of tables in the transaction are changed",
                vec![
                    logutil::log::LogField::String(
                        "conflicted table IDs".into(),
                        format!("{ids:?}"),
                    ),
                    logutil::log::LogField::I64("transaction schema".into(), curr_ver),
                    logutil::log::LogField::String(
                        "schema versions that changed the tables".into(),
                        format!("{changed_schema_versions:?}"),
                    ),
                ],
            );
            return true;
        }
        false
    }

    /// 返回队列中 schema_version > curr_ver 的后缀切片。
    fn find_newer_deltas(state: &ValidatorState, curr_ver: i64) -> &[DeltaSchemaInfo] {
        let queue = &state.delta_schema_infos;
        let mut position = queue.len();
        for index in (0..queue.len()).rev() {
            if queue[index].schema_version <= curr_ver {
                break;
            }
            position = index;
        }
        &queue[position..]
    }

    /// 事务提交前校验：返回 (可选 RelatedSchemaChange, Result)。
    ///
    /// - Fail：确定相关 schema 已变或版本过旧。
    /// - Unknown：租约过期或校验器已停，需重新向 PD 确认。
    /// - Succ：当前信息下可安全提交。
    pub fn check(
        &self,
        txn_ts: u64,
        schema_ver: i64,
        related_physical_table_ids: Option<&[i64]>,
        need_check_schema_by_delta: bool,
    ) -> (Option<RelatedSchemaChange>, Result) {
        let state = read_state(&self.state);
        if !state.is_started {
            log_info("the schema validator stopped before checking");
            return (None, Result::ResultUnknown);
        }

        // 事务 schema 早于 restart 下界：集群曾不健康，直接 Fail。
        if schema_ver < state.restart_schema_ver {
            log_info_fields(
                "the schema version is too old, TiDB and PD maybe unhealthy after the transaction started",
                vec![logutil::log::LogField::I64("schemaVer".into(), schema_ver)],
            );
            return (None, Result::ResultFail);
        }

        if schema_ver < state.latest_schema_ver {
            // None represents Go's nil slice: callers only want to check the
            // schema version, so any version change fails immediately. Some(&[])
            // remains distinct for temporary-table-only transactions.
            // None ≈ Go nil：只要版本前进就失败；Some(&[]) 给仅临时表的事务。
            let Some(table_ids) = related_physical_table_ids else {
                log_info_fields(
                    "the related physical table ID is empty",
                    vec![
                        logutil::log::LogField::I64("schemaVer".into(), schema_ver),
                        logutil::log::LogField::I64(
                            "latestSchemaVer".into(),
                            state.latest_schema_ver,
                        ),
                    ],
                );
                return (None, Result::ResultFail);
            };

            // 需要按 delta 检查，或未开启 MDL 时，走相关表变更判定。
            if (need_check_schema_by_delta || !vardef::IsMDLEnabled())
                && Self::is_related_tables_changed_locked(&state, schema_ver, table_ids)
            {
                return (None, Result::ResultFail);
            }
            return (None, Result::ResultSucc);
        }

        // 事务开始时间已超过租约：无法仅凭本地状态判定，返回 Unknown。
        if time_from_ts(txn_ts) > state.latest_schema_expire {
            return (None, Result::ResultUnknown);
        }
        (None, Result::ResultSucc)
    }

    /// 测试入口：直接入队一条 schema 增量。
    pub(crate) fn enqueue(&self, schema_version: i64, change: Option<&RelatedSchemaChange>) {
        let mut state = write_state(&self.state);
        Self::enqueue_locked(&mut state, schema_version, change);
    }

    /// 入队并按 `contain_in` 压缩；超长时淘汰队首。
    fn enqueue_locked(
        state: &mut ValidatorState,
        schema_version: i64,
        change: Option<&RelatedSchemaChange>,
    ) {
        let max_count = vardef::GetMaxDeltaSchemaCount();
        // max_count<=0 表示禁用增量缓存。
        if max_count <= 0 {
            log_info_fields(
                "the schema validator enqueue",
                vec![logutil::log::LogField::I64(
                    "delta max count".into(),
                    max_count,
                )],
            );
            return;
        }
        let max_count = max_count as usize;

        let delta = DeltaSchemaInfo {
            schema_version,
            related_ids: change
                .map(|value| value.phy_tbl_ids.clone())
                .unwrap_or_default(),
            related_actions: change
                .map(|value| value.action_types.clone())
                .unwrap_or_default(),
        };
        if state.delta_schema_infos.is_empty() {
            state.delta_schema_infos.push(delta);
            return;
        }

        let last_offset = state.delta_schema_infos.len() - 1;
        // Never merge the first item; retaining it covers more old versions.
        // 不合并队首：保留最旧版本可覆盖更大历史窗口。
        if last_offset != 0 && contain_in(&state.delta_schema_infos[last_offset], &delta) {
            state.delta_schema_infos[last_offset] = delta;
        } else {
            state.delta_schema_infos.push(delta);
        }

        if state.delta_schema_infos.len() > max_count {
            let removed_version = state.delta_schema_infos[0].schema_version;
            log_info_fields(
                "the schema validator enqueue, queue is too long",
                vec![
                    logutil::log::LogField::I64("delta max count".into(), max_count as i64),
                    logutil::log::LogField::I64("remove schema version".into(), removed_version),
                ],
            );
            state.delta_schema_infos.remove(0);
        }
    }
}

/// 对接 `validatorapi::Validator` 的 Go 风格方法名。
impl ValidatorApi for Validator {
    type RelatedSchemaChange = RelatedSchemaChange;

    fn Update(
        &self,
        leaseGrantTime: u64,
        oldSchemaVer: i64,
        newSchemaVer: i64,
        change: Option<&Self::RelatedSchemaChange>,
    ) {
        self.update(leaseGrantTime, oldSchemaVer, newSchemaVer, change);
    }

    fn Check(
        &self,
        txnTS: u64,
        schemaVer: i64,
        relatedPhysicalTableIDs: Option<&[i64]>,
        needCheckSchema: bool,
    ) -> (Option<Self::RelatedSchemaChange>, Result) {
        self.check(txnTS, schemaVer, relatedPhysicalTableIDs, needCheckSchema)
    }

    fn Stop(&self) {
        self.stop();
    }

    fn Restart(&self, currSchemaVer: i64) {
        self.restart(currSchemaVer);
    }

    fn Reset(&self) {
        self.reset();
    }

    fn IsStarted(&self) -> bool {
        self.is_started()
    }

    fn IsLeaseExpired(&self) -> bool {
        self.is_lease_expired()
    }
}

/// contain_in checks whether every table/action pair in `last` occurs in
/// `current`, preserving Go's nested-loop and duplicate handling.
/// 判断 `last` 的每个 (表, action) 是否都出现在 `current` 中（嵌套循环，保留重复项语义）。
pub fn contain_in(last: &DeltaSchemaInfo, current: &DeltaSchemaInfo) -> bool {
    if last.related_ids.len() > current.related_ids.len() {
        return false;
    }

    for (index, last_table_id) in last.related_ids.iter().enumerate() {
        let last_action = last
            .related_actions
            .get(index)
            .expect("related table IDs and action types must have equal length");
        let mut equal = false;
        for (current_index, current_table_id) in current.related_ids.iter().enumerate() {
            if last_table_id == current_table_id
                && last_action == &current.related_actions[current_index]
            {
                equal = true;
                break;
            }
        }
        if !equal {
            return false;
        }
    }
    true
}
