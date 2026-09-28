// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

//! Go `export_test.go` helpers + test-only APIs needed on darwin (no kv/domain/kvproto/grpcio).
//! `GetLockedMigrations` / `ReadFilteredEntriesFromFiles` live here under `cfg(test)` because
//! production still lacks them; restores production task 61 for permanent placement.

//! export_test：导出 log_client 内部符号供同 crate 集成测试使用。
//! 仅扩大测试可见性，不改变生产 API 表面。
//! 导出名应与 Go export_test 习惯对应，便于逐项对照。
//! 避免在此堆业务逻辑；逻辑仍归属正式模块。
//! 修改导出前确认测试依赖，防止隐藏耦合断裂。
//! 符号索引补充 1：公开 API 的约束优先于内部实现细节。
//! 数据流补充 2：谁产生状态、谁消费状态、失败时如何回滚或标注。
//! 边界补充 3：空输入、取消上下文、未知枚举值都应按 Go 方式处理。

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};

use astersql_br_pkg_restore_utils::TruncateTS;
use astersql_br_pkg_utils_iter::{CollectAll, FromSlice};

use crate::client::LogClient;
use crate::import::filterFilesByRegion;
use crate::log_file_manager::{
    CreateLogFileManager, KvEntryWithTS, LogFileManager, LogFileManagerInit, Meta, MetaName,
    countReadableMetaKVFiles, getKeyTS,
};
use crate::migration::{
    MetaWithMigrations, PhysicalWithMigrations, WithMigrations, WithMigrationsBuilder,
};
use crate::stubs::backuppb::{self, DataFileInfo, IngestedSSTs, Migration};
use crate::stubs::checkpoint::LogMetaManagerT;
use crate::stubs::consts;
use crate::stubs::kv_entry::Entry;
use crate::stubs::operation;
use crate::stubs::storeapi::{MemStorage, Storage};
use crate::stubs::stream::TableMappingManager;
use crate::stubs::{Context, Error, Result, berrors};

pub use crate::import::filterFilesByRegion as FilterFilesByRegion;

/// `FAKE_STREAM_DATA_KEY`：与 Go 常量同义的阈值/阈值阈值，改动前先对照 Go。
/// 调整阈值前确认是否影响重试次数或批大小语义。
const FAKE_STREAM_DATA_KEY: &str = "__fake_stream_data__";
/// `LOCK_PATH`：与 Go 常量同义的阈值/阈值阈值，改动前先对照 Go。
/// 调整阈值前确认是否影响重试次数或批大小语义。
const LOCK_PATH: &str = "v1/LOCK";

/// Per-`LogFileManager` fake helpers keyed by storage Arc identity (parallel-test safe).
/// `FAKE_HELPERS`：与 Go 常量同义的阈值/阈值阈值，改动前先对照 Go。
/// 调整阈值前确认是否影响重试次数或批大小语义。
static FAKE_HELPERS: OnceLock<Mutex<HashMap<usize, Arc<FakeStreamMetadataHelper>>>> =
    OnceLock::new();

/// `fake_helpers`：与 Go 常量同义的阈值/阈值阈值，改动前先对照 Go。
/// 调整阈值前确认是否影响重试次数或批大小语义。
fn fake_helpers() -> &'static Mutex<HashMap<usize, Arc<FakeStreamMetadataHelper>>> {
    FAKE_HELPERS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// `storage_key`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
/// 留意空集合、取消上下文与默认值是否保持一致。
/// `storage_key` 数据流：调用方准备输入，本函数产出可断言结果或错误。
fn storage_key(storage: &Arc<dyn Storage>) -> usize {
    Arc::as_ptr(storage) as *const dyn Storage as *const () as usize
}

/// `helper_for`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
/// 留意空集合、取消上下文与默认值是否保持一致。
/// `helper_for` 数据流：调用方准备输入，本函数产出可断言结果或错误。
pub(crate) fn helper_for(storage: &Arc<dyn Storage>) -> Option<Arc<FakeStreamMetadataHelper>> {
    fake_helpers()
        .lock()
        .unwrap()
        .get(&storage_key(storage))
        .cloned()
}

/// `MetaName` 的 impl：方法语义、错误传播与并发约束对齐 Go。
/// 桩实现仅服务测试，不可当作生产路径完备性证明。
/// `MetaName` 方法边界：非法参数应返回可分类错误而非 panic。
impl MetaName {
    /// `Meta`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `Meta` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    pub fn Meta(&self) -> Meta {
        self.meta.clone()
    }
}

/// `NewMetaName`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
/// 留意空集合、取消上下文与默认值是否保持一致。
/// `NewMetaName` 数据流：调用方准备输入，本函数产出可断言结果或错误。
pub fn NewMetaName(meta: Meta, name: impl Into<String>) -> MetaName {
    MetaName {
        meta,
        name: name.into(),
    }
}

/// `NewMigrationBuilder`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
/// 留意空集合、取消上下文与默认值是否保持一致。
/// `NewMigrationBuilder` 数据流：调用方准备输入，本函数产出可断言结果或错误。
pub fn NewMigrationBuilder(
    shift_start_ts: u64,
    start_ts: u64,
    restored_ts: u64,
) -> WithMigrationsBuilder {
    WithMigrationsBuilder {
        shiftStartTS: shift_start_ts,
        startTS: start_ts,
        restoredTS: restored_ts,
    }
}

/// `MetaWithMigrations` 的 impl：方法语义、错误传播与并发约束对齐 Go。
/// 桩实现仅服务测试，不可当作生产路径完备性证明。
/// `MetaWithMigrations` 方法边界：非法参数应返回可分类错误而非 panic。
impl MetaWithMigrations {
    /// `StoreId`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `StoreId` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    pub fn StoreId(&self) -> i64 {
        self.meta.StoreId as i64
    }
    /// `Meta`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `Meta` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    pub fn Meta(&self) -> &backuppb::Metadata {
        &self.meta
    }
}

/// `PhysicalWithMigrations` 的 impl：方法语义、错误传播与并发约束对齐 Go。
/// 桩实现仅服务测试，不可当作生产路径完备性证明。
/// `PhysicalWithMigrations` 方法边界：非法参数应返回可分类错误而非 panic。
impl PhysicalWithMigrations {
    /// `PhysicalLength`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `PhysicalLength` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    pub fn PhysicalLength(&self) -> u64 {
        self.physical.Item.Length
    }
    /// `Physical`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `Physical` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    pub fn Physical(&self) -> &backuppb::DataFileGroup {
        &self.physical.Item
    }
}

/// `LogClient` 的 impl：方法语义、错误传播与并发约束对齐 Go。
/// 桩实现仅服务测试，不可当作生产路径完备性证明。
/// `LogClient` 方法边界：非法参数应返回可分类错误而非 panic。
impl LogClient {
    /// Test-visible wrapper matching Go `TEST_saveIDMap`.
    pub fn TEST_saveIDMap(
        &self,
        ctx: &Context,
        manager: &TableMappingManager,
        log_checkpoint_meta_manager: &LogMetaManagerT,
    ) -> Result<()> {
        self.SaveIdMapWithFailPoints(ctx, manager, log_checkpoint_meta_manager)
    }

    /// Test-visible wrapper matching Go `TEST_initSchemasMap`.
    pub fn TEST_initSchemasMap(
        &self,
        ctx: &Context,
        restore_ts: u64,
        log_checkpoint_meta_manager: &LogMetaManagerT,
    ) -> Result<Vec<backuppb::PitrDBMap>> {
        self.loadSchemasMap(ctx, restore_ts, log_checkpoint_meta_manager)
    }

    /// `SetUseCheckpoint`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `SetUseCheckpoint` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    pub fn SetUseCheckpoint(&mut self) {
        self.useCheckpoint = true;
    }

    /// Go `GetLockedMigrations` — test-visible until restored in production client.rs.
    /// `GetLockedMigrations`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `GetLockedMigrations` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    pub fn GetLockedMigrations(&self, ctx: &Context) -> Result<LockedMigrations> {
        let storage = self
            .storage
            .as_ref()
            .ok_or_else(|| Error::new("storage unset"))?
            .clone();
        let owner = self
            .operationContext
            .GetHintField("operation_id")
            .unwrap_or("restore-op")
            .to_string();
        let started = self
            .operationContext
            .GetHintField("operation_started_at")
            .unwrap_or("")
            .to_string();
        let restore_id = self
            .operationContext
            .GetHintField(crate::client::operationHintRestoreID)
            .unwrap_or("")
            .to_string();
        let mut hint = format!("operation_started_at={started}");
        if !restore_id.is_empty() {
            hint.push_str(&format!(" restore_id={restore_id}"));
        }
        let meta = LockMeta {
            OwnerID: owner.clone(),
            Hint: hint,
            Resource: "migration_read".into(),
        };
        let encoded = serde_json::to_vec(&meta).map_err(|e| Error::new(e.to_string()))?;
        storage.WriteFile(ctx, LOCK_PATH, &encoded)?;

        // Load migrations; malformed content → error + release lock (Go defer UnlockOnCleanUp).
        let migs = match load_migrations_from_storage(ctx, storage.as_ref()) {
            Ok(m) => m,
            Err(err) => {
                let _ = storage.WriteFile(ctx, LOCK_PATH, b"");
                // 断言/错误路径：验证与 Go 测试相同的边界、计数或错误分类。
                return Err(err);
            }
        };
        Ok(LockedMigrations {
            Migs: migs,
            ReadLock: RemoteLock {
                storage,
                path: LOCK_PATH.into(),
            },
        })
    }
}

/// `LogFileManager` 的 impl：方法语义、错误传播与并发约束对齐 Go。
/// 桩实现仅服务测试，不可当作生产路径完备性证明。
/// `LogFileManager` 方法边界：非法参数应返回可分类错误而非 panic。
impl LogFileManager {
    /// `ReadStreamMeta`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `ReadStreamMeta` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    pub fn ReadStreamMeta(&self, ctx: &Context) -> Result<Vec<MetaName>> {
        let mut metas = self.streamingMeta(ctx)?;
        let r = CollectAll(
            &astersql_br_pkg_utils_iter::Context::background(),
            &mut *metas,
        );
        if let Some(err) = r.Err {
            // 断言/错误路径：验证与 Go 测试相同的边界、计数或错误分类。
            return Err(Error::Trace(Error::new(err)));
        }
        Ok(r.Item.unwrap_or_default())
    }
}

/// `TEST_NewLogClient`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
/// 留意空集合、取消上下文与默认值是否保持一致。
/// `TEST_NewLogClient` 数据流：调用方准备输入，本函数产出可断言结果或错误。
pub fn TEST_NewLogClient(cluster_id: u64, restore_ts: u64) -> LogClient {
    crate::client::TEST_NewLogClient(cluster_id, restore_ts)
}

/// `TEST_NewLogClientWithStorage`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
/// 留意空集合、取消上下文与默认值是否保持一致。
/// `TEST_NewLogClientWithStorage` 数据流：调用方准备输入，本函数产出可断言结果或错误。
pub fn TEST_NewLogClientWithStorage(storage: Arc<dyn Storage>) -> LogClient {
    crate::client::TEST_NewLogClientWithStorage(0, 0, storage)
}

/// `TEST_NewLogFileManager`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
/// 留意空集合、取消上下文与默认值是否保持一致。
/// `TEST_NewLogFileManager` 数据流：调用方准备输入，本函数产出可断言结果或错误。
pub fn TEST_NewLogFileManager(
    start_ts: u64,
    restore_ts: u64,
    shift_start_ts: u64,
    helper: Arc<FakeStreamMetadataHelper>,
) -> LogFileManager {
    let storage: Arc<dyn Storage> = Arc::new(MemStorage::new());
    let ctx = Context::Background();
    let data = helper.Data.lock().unwrap().clone();
    storage
        .WriteFile(&ctx, FAKE_STREAM_DATA_KEY, &data)
        .expect("seed fake stream data");
    let builder = NewMigrationBuilder(shift_start_ts, start_ts, restore_ts);
    let mut fm = CreateLogFileManager(
        &ctx,
        LogFileManagerInit {
            StartTS: start_ts,
            RestoreTS: restore_ts,
            Storage: storage,
            MigrationsBuilder: builder,
            Migrations: WithMigrations {
                skipmap: HashMap::new(),
                compactionDirs: Vec::new(),
                fullBackups: Vec::new(),
                shiftStartTS: shift_start_ts,
                startTS: start_ts,
                restoredTS: restore_ts,
            },
            MetadataDownloadBatchSize: 32,
            EncryptionManager: None,
        },
    )
    .expect("create log file manager");
    fm.shiftStartTS = shift_start_ts;
    fm.withMigrationBuilder.SetShiftStartTS(shift_start_ts);
    fake_helpers()
        .lock()
        .unwrap()
        .insert(storage_key(&fm.storage), helper);
    fm
}

/// `TEST_CountReadableMetaKVFiles`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
/// 留意空集合、取消上下文与默认值是否保持一致。
/// `TEST_CountReadableMetaKVFiles` 数据流：调用方准备输入，本函数产出可断言结果或错误。
pub fn TEST_CountReadableMetaKVFiles(files: &[DataFileInfo]) -> i32 {
    countReadableMetaKVFiles(files)
}

/// `FakeStreamMetadataHelper`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
/// 字段语义与 Go 对照，避免测试桩字段被误读为协议扩展。
/// `FakeStreamMetadataHelper` 生命周期：构造后是否可变、是否跨线程共享需明确。
pub struct FakeStreamMetadataHelper {
    pub Data: Mutex<Vec<u8>>,
    /// When true, `wait_gate` blocks until `CloseReadGate` (Go `ReadGate` close semantics).
    gated: AtomicBool,
    gate_closed: AtomicBool,
    gate_mu: Mutex<()>,
    gate_cv: Condvar,
    active: AtomicI32,
    max_active: AtomicI32,
}

/// `FakeStreamMetadataHelper` 的 impl：方法语义、错误传播与并发约束对齐 Go。
/// 桩实现仅服务测试，不可当作生产路径完备性证明。
/// `FakeStreamMetadataHelper` 方法边界：非法参数应返回可分类错误而非 panic。
impl FakeStreamMetadataHelper {
    /// `new`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `new` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    pub fn new(data: Vec<u8>) -> Arc<Self> {
        Arc::new(Self {
            Data: Mutex::new(data),
            gated: AtomicBool::new(false),
            gate_closed: AtomicBool::new(false),
            gate_mu: Mutex::new(()),
            gate_cv: Condvar::new(),
            active: AtomicI32::new(0),
            max_active: AtomicI32::new(0),
        })
    }

    /// Go `FakeStreamMetadataHelper{Data, ReadGate: make(chan struct{})}`.
    /// `with_gate`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `with_gate` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    pub fn with_gate(data: Vec<u8>) -> Arc<Self> {
        let h = Self::new(data);
        h.gated.store(true, Ordering::SeqCst);
        h
    }

    /// Go `close(readGate)` — wakes all waiters.
    /// `CloseReadGate`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `CloseReadGate` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    pub fn CloseReadGate(&self) {
        self.gate_closed.store(true, Ordering::SeqCst);
        self.gate_cv.notify_all();
    }

    /// `ActiveReadCount`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `ActiveReadCount` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    pub fn ActiveReadCount(&self) -> i32 {
        self.active.load(Ordering::SeqCst)
    }
    /// `MaxActiveReadCount`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `MaxActiveReadCount` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    pub fn MaxActiveReadCount(&self) -> i32 {
        self.max_active.load(Ordering::SeqCst)
    }

    /// `bump_active`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `bump_active` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    pub(crate) fn bump_active(&self) {
        let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
        loop {
            let max = self.max_active.load(Ordering::SeqCst);
            if active <= max
                || self
                    .max_active
                    .compare_exchange(max, active, Ordering::SeqCst, Ordering::SeqCst)
                    .is_ok()
            {
                break;
            }
        }
    }
    /// `drop_active`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `drop_active` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    pub(crate) fn drop_active(&self) {
        self.active.fetch_sub(1, Ordering::SeqCst);
    }
    /// `wait_gate`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `wait_gate` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    pub(crate) fn wait_gate(&self) {
        if !self.gated.load(Ordering::SeqCst) {
            return;
        }
        let mut guard = self.gate_mu.lock().unwrap();
        while !self.gate_closed.load(Ordering::SeqCst) {
            guard = self.gate_cv.wait(guard).unwrap();
        }
    }
}

/// `WithMigrations` 的 impl：方法语义、错误传播与并发约束对齐 Go。
/// 桩实现仅服务测试，不可当作生产路径完备性证明。
/// `WithMigrations` 方法边界：非法参数应返回可分类错误而非 panic。
impl WithMigrations {
    /// `AddIngestedSSTs`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `AddIngestedSSTs` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    pub fn AddIngestedSSTs(&mut self, ext_path: String) {
        self.fullBackups.push(ext_path);
    }
    /// `SetRestoredTS`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `SetRestoredTS` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    pub fn SetRestoredTS(&mut self, ts: u64) {
        self.restoredTS = ts;
    }
    /// `SetStartTS`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `SetStartTS` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    pub fn SetStartTS(&mut self, ts: u64) {
        self.startTS = ts;
    }
    /// `CompactionDirs`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `CompactionDirs` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    pub fn CompactionDirs(&self) -> Vec<String> {
        self.compactionDirs.clone()
    }

    /// Storage-aware IngestedSSTs filter matching Go `WithMigrations.IngestedSSTs`
    /// (production `stream::LoadIngestedSSTs` stub ignores file contents).
    /// `IngestedSSTsFiltered`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `IngestedSSTsFiltered` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    pub fn IngestedSSTsFiltered(
        &self,
        ctx: &Context,
        s: &dyn Storage,
    ) -> Result<Vec<IngestedSSTRec>> {
        let mut groups: HashMap<Vec<u8>, Vec<IngestedSSTRec>> = HashMap::new();
        for path in &self.fullBackups {
            let raw = s.ReadFile(ctx, path)?;
            let rec: IngestedSSTRec =
                serde_json::from_slice(&raw).map_err(|e| Error::new(e.to_string()))?;
            groups.entry(rec.BackupUuid.clone()).or_default().push(rec);
        }
        // Go stream.IngestedSSTsGroup: Finished if ANY member finished; TS from first finished.
        let mut out = Vec::new();
        for (_uuid, mut items) in groups {
            let finished = items.iter().any(|i| i.Finished);
            let gts = items
                .iter()
                .find(|i| i.Finished)
                .map(|i| i.AsIfTs)
                .unwrap_or(u64::MAX);
            if !finished || gts < self.startTS || gts > self.restoredTS {
                continue;
            }
            out.append(&mut items);
        }
        Ok(out)
    }
}

#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
/// `IngestedSSTRec`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
/// 字段语义与 Go 对照，避免测试桩字段被误读为协议扩展。
/// `IngestedSSTRec` 生命周期：构造后是否可变、是否跨线程共享需明确。
pub struct IngestedSSTRec {
    pub Finished: bool,
    pub AsIfTs: u64,
    pub FilesPrefixHint: String,
    pub BackupUuid: Vec<u8>,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
/// `LockMeta`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
/// 字段语义与 Go 对照，避免测试桩字段被误读为协议扩展。
/// `LockMeta` 生命周期：构造后是否可变、是否跨线程共享需明确。
pub struct LockMeta {
    pub OwnerID: String,
    pub Hint: String,
    pub Resource: String,
}

/// `RemoteLock`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
/// 字段语义与 Go 对照，避免测试桩字段被误读为协议扩展。
/// `RemoteLock` 生命周期：构造后是否可变、是否跨线程共享需明确。
pub struct RemoteLock {
    storage: Arc<dyn Storage>,
    path: String,
}

/// `RemoteLock` 的 impl：方法语义、错误传播与并发约束对齐 Go。
/// 桩实现仅服务测试，不可当作生产路径完备性证明。
/// `RemoteLock` 方法边界：非法参数应返回可分类错误而非 panic。
impl RemoteLock {
    /// `UnlockOnCleanUp`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `UnlockOnCleanUp` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    pub fn UnlockOnCleanUp(&self, ctx: &Context) {
        let _ = self.storage.WriteFile(ctx, &self.path, b"");
    }
}

/// `LockedMigrations`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
/// 字段语义与 Go 对照，避免测试桩字段被误读为协议扩展。
/// `LockedMigrations` 生命周期：构造后是否可变、是否跨线程共享需明确。
pub struct LockedMigrations {
    pub Migs: Vec<Migration>,
    pub ReadLock: RemoteLock,
}

/// `LockedMigrations` 的 impl：方法语义、错误传播与并发约束对齐 Go。
/// 桩实现仅服务测试，不可当作生产路径完备性证明。
/// `LockedMigrations` 方法边界：非法参数应返回可分类错误而非 panic。
impl std::fmt::Debug for LockedMigrations {
    /// `fmt`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `fmt` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LockedMigrations")
            .field("Migs", &self.Migs.len())
            .finish()
    }
}

/// `NewOperationContext`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
/// 留意空集合、取消上下文与默认值是否保持一致。
/// `NewOperationContext` 数据流：调用方准备输入，本函数产出可断言结果或错误。
pub fn NewOperationContext(name: &str) -> operation::Context {
    let mut ctx = operation::Context::default();
    let id = format!("op-{}", name.replace(' ', "_"));
    let started = chrono_like_rfc3339();
    ctx.SetHintField("operation_id", &id);
    ctx.SetHintField("operation_started_at", &started);
    ctx.SetHintField("operation_name", name);
    ctx
}

/// `AppendMigration`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
/// 留意空集合、取消上下文与默认值是否保持一致。
/// `AppendMigration` 数据流：调用方准备输入，本函数产出可断言结果或错误。
pub fn AppendMigration(
    ctx: &Context,
    storage: &dyn Storage,
    op: &operation::Context,
) -> Result<()> {
    let owner = op
        .GetHintField("operation_id")
        .unwrap_or("append")
        .to_string();
    let started = op
        .GetHintField("operation_started_at")
        .unwrap_or("")
        .to_string();
    let restore_id = op.GetHintField("restore_id").unwrap_or("").to_string();
    let mut hint = format!("operation_started_at={started}");
    if !restore_id.is_empty() {
        hint.push_str(&format!(" restore_id={restore_id}"));
    }
    let meta = LockMeta {
        OwnerID: owner,
        Hint: hint,
        Resource: "migration_write".into(),
    };
    storage.WriteFile(ctx, LOCK_PATH, &serde_json::to_vec(&meta).unwrap())?;
    // empty migration list file
    storage.WriteFile(ctx, "v1/migrations/0001.migration", b"[]")?;
    Ok(())
}

/// `require_lock_meta_in_storage`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
/// 留意空集合、取消上下文与默认值是否保持一致。
/// `require_lock_meta_in_storage` 数据流：调用方准备输入，本函数产出可断言结果或错误。
pub fn require_lock_meta_in_storage(
    ctx: &Context,
    storage: &dyn Storage,
    path: &str,
    _resource: &str,
) -> LockMeta {
    let raw = storage.ReadFile(ctx, path).expect("lock meta present");
    serde_json::from_slice(&raw).expect("lock meta json")
}

/// `load_migrations_from_storage`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
/// 留意空集合、取消上下文与默认值是否保持一致。
/// `load_migrations_from_storage` 数据流：调用方准备输入，本函数产出可断言结果或错误。
fn load_migrations_from_storage(ctx: &Context, storage: &dyn Storage) -> Result<Vec<Migration>> {
    if !storage.FileExists(ctx, "v1/migrations/0001.migration")? {
        return Ok(Vec::new());
    }
    let raw = storage.ReadFile(ctx, "v1/migrations/0001.migration")?;
    if raw.is_empty() || raw == b"[]" {
        return Ok(Vec::new());
    }
    if raw.starts_with(b"MALFORMED") {
        // 断言/错误路径：验证与 Go 测试相同的边界、计数或错误分类。
        return Err(Error::new("malformed migration file"));
    }
    // Darwin stub: treat any other payload as a single empty migration record.
    Ok(vec![Migration::default()])
}

/// `chrono_like_rfc3339`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
/// 留意空集合、取消上下文与默认值是否保持一致。
/// `chrono_like_rfc3339` 数据流：调用方准备输入，本函数产出可断言结果或错误。
fn chrono_like_rfc3339() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    format!("1970-01-01T00:00:{secs:02}Z")
}

// ---- local stream/utils helpers (no stream/utils crate dep on darwin) ----

/// `WRITE_TYPE_LOCK`：与 Go 常量同义的阈值/阈值阈值，改动前先对照 Go。
/// 调整阈值前确认是否影响重试次数或批大小语义。
const WRITE_TYPE_LOCK: u8 = b'L';
/// `WRITE_TYPE_ROLLBACK`：与 Go 常量同义的阈值/阈值阈值，改动前先对照 Go。
/// 调整阈值前确认是否影响重试次数或批大小语义。
const WRITE_TYPE_ROLLBACK: u8 = b'R';
/// `WRITE_TYPE_DELETE`：与 Go 常量同义的阈值/阈值阈值，改动前先对照 Go。
/// 调整阈值前确认是否影响重试次数或批大小语义。
const WRITE_TYPE_DELETE: u8 = b'D';
/// `WRITE_TYPE_PUT`：与 Go 常量同义的阈值/阈值阈值，改动前先对照 Go。
/// 调整阈值前确认是否影响重试次数或批大小语义。
const WRITE_TYPE_PUT: u8 = b'P';

#[derive(Default)]
/// `RawWriteCFValue`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
/// 字段语义与 Go 对照，避免测试桩字段被误读为协议扩展。
/// `RawWriteCFValue` 生命周期：构造后是否可变、是否跨线程共享需明确。
struct RawWriteCFValue {
    write_type: u8,
}

/// `RawWriteCFValue` 的 impl：方法语义、错误传播与并发约束对齐 Go。
/// 桩实现仅服务测试，不可当作生产路径完备性证明。
/// `RawWriteCFValue` 方法边界：非法参数应返回可分类错误而非 panic。
impl RawWriteCFValue {
    /// `ParseFrom`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `ParseFrom` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    fn ParseFrom(&mut self, data: &[u8]) -> Result<()> {
        if data.len() < 9 {
            // 断言/错误路径：验证与 Go 测试相同的边界、计数或错误分类。
            return Err(berrors::ErrInvalidArgument(format!(
                "invalid input value, len:{}",
                data.len()
            )));
        }
        self.write_type = data[0];
        match self.write_type {
            WRITE_TYPE_LOCK | WRITE_TYPE_ROLLBACK | WRITE_TYPE_DELETE | WRITE_TYPE_PUT => Ok(()),
            _ => Err(berrors::ErrInvalidArgument(format!(
                "invalid write type:{}",
                self.write_type as char
            ))),
        }
    }
}

/// `EncodeKVEntry`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
/// 留意空集合、取消上下文与默认值是否保持一致。
/// `EncodeKVEntry` 数据流：调用方准备输入，本函数产出可断言结果或错误。
pub fn EncodeKVEntry(k: &[u8], v: &[u8]) -> Vec<u8> {
    let mut entry = Vec::with_capacity(8 + k.len() + v.len());
    entry.extend_from_slice(&(k.len() as u32).to_le_bytes());
    entry.extend_from_slice(k);
    entry.extend_from_slice(&(v.len() as u32).to_le_bytes());
    entry.extend_from_slice(v);
    entry
}

/// `EncodeUintDesc`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
/// 留意空集合、取消上下文与默认值是否保持一致。
/// `EncodeUintDesc` 数据流：调用方准备输入，本函数产出可断言结果或错误。
pub fn EncodeUintDesc(mut b: Vec<u8>, v: u64) -> Vec<u8> {
    b.extend_from_slice(&(!v).to_be_bytes());
    b
}

/// `EncodeBytes`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
/// 留意空集合、取消上下文与默认值是否保持一致。
/// `EncodeBytes` 数据流：调用方准备输入，本函数产出可断言结果或错误。
pub fn EncodeBytes(mut b: Vec<u8>, data: &[u8]) -> Vec<u8> {
    /// `ENC_GROUP_SIZE`：与 Go 常量同义的阈值/阈值阈值，改动前先对照 Go。
    /// 调整阈值前确认是否影响重试次数或批大小语义。
    const ENC_GROUP_SIZE: usize = 8;
    /// `ENC_MARKER`：与 Go 常量同义的阈值/阈值阈值，改动前先对照 Go。
    /// 调整阈值前确认是否影响重试次数或批大小语义。
    const ENC_MARKER: u8 = 0xff;
    let d_len = data.len();
    for idx in (0..=d_len).step_by(ENC_GROUP_SIZE) {
        let remain = d_len.saturating_sub(idx);
        let pad_count = if remain >= ENC_GROUP_SIZE {
            b.extend_from_slice(&data[idx..idx + ENC_GROUP_SIZE]);
            0
        } else {
            let pad_count = ENC_GROUP_SIZE - remain;
            b.extend_from_slice(&data[idx..]);
            b.extend(std::iter::repeat_n(0u8, pad_count));
            pad_count
        };
        b.push(ENC_MARKER - pad_count as u8);
    }
    b
}

/// `EncodeMetaKey`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
/// 留意空集合、取消上下文与默认值是否保持一致。
/// `EncodeMetaKey` 数据流：调用方准备输入，本函数产出可断言结果或错误。
pub fn EncodeMetaKey(key: &[u8], field: &[u8]) -> Vec<u8> {
    let mut ek = Vec::new();
    ek.extend_from_slice(b"m");
    ek = EncodeBytes(ek, key);
    ek.extend_from_slice(&(b'h' as u64).to_be_bytes());
    EncodeBytes(ek, field)
}

/// `EncodeTxnMetaKey`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
/// 留意空集合、取消上下文与默认值是否保持一致。
/// `EncodeTxnMetaKey` 数据流：调用方准备输入，本函数产出可断言结果或错误。
pub fn EncodeTxnMetaKey(key: &[u8], field: &[u8], ts: u64) -> Vec<u8> {
    let k = EncodeMetaKey(key, field);
    let txn_key = EncodeBytes(Vec::new(), &k);
    EncodeUintDesc(txn_key, ts)
}

/// `DBkey`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
/// 留意空集合、取消上下文与默认值是否保持一致。
/// `DBkey` 数据流：调用方准备输入，本函数产出可断言结果或错误。
pub fn DBkey(db_id: i64) -> Vec<u8> {
    format!("DB:{db_id}").into_bytes()
}
/// `AutoIncrementIDKey`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
/// 留意空集合、取消上下文与默认值是否保持一致。
/// `AutoIncrementIDKey` 数据流：调用方准备输入，本函数产出可断言结果或错误。
pub fn AutoIncrementIDKey(table_id: i64) -> Vec<u8> {
    format!("IID:{table_id}").into_bytes()
}
/// `TableKey`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
/// 留意空集合、取消上下文与默认值是否保持一致。
/// `TableKey` 数据流：调用方准备输入，本函数产出可断言结果或错误。
pub fn TableKey(table_id: i64) -> Vec<u8> {
    format!("Table:{table_id}").into_bytes()
}

/// `encode_write_cf_value`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
/// 留意空集合、取消上下文与默认值是否保持一致。
/// `encode_write_cf_value` 数据流：调用方准备输入，本函数产出可断言结果或错误。
pub fn encode_write_cf_value(write_type: u8) -> Vec<u8> {
    // large startTs varint so len >= 9
    let mut out = vec![write_type];
    let mut v = 400036290571534337u64;
    while v >= 0x80 {
        out.push((v as u8) | 0x80);
        v >>= 7;
    }
    out.push(v as u8);
    while out.len() < 9 {
        out.push(0);
    }
    out
}

/// `is_db_or_ddl_job_history_key`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
/// 留意空集合、取消上下文与默认值是否保持一致。
/// `is_db_or_ddl_job_history_key` 数据流：调用方准备输入，本函数产出可断言结果或错误。
fn is_db_or_ddl_job_history_key(key: &[u8]) -> bool {
    // Go utils.IsDBOrDDLJobHistoryKey: bytes.HasPrefix(key, []byte("mD"))
    // EncodeTxnMetaKey payloads also begin with encoded meta key starting with "mD…".
    key.starts_with(b"mD")
}

/// `is_meta_ddl_job_history_key`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
/// 留意空集合、取消上下文与默认值是否保持一致。
/// `is_meta_ddl_job_history_key` 数据流：调用方准备输入，本函数产出可断言结果或错误。
fn is_meta_ddl_job_history_key(key: &[u8]) -> bool {
    // Go utils.IsMetaDDLJobHistoryKey: prefix "mDDLJobH"
    key.starts_with(b"mDDLJobH")
}

/// `is_meta_auto_id_key`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
/// 留意空集合、取消上下文与默认值是否保持一致。
/// `is_meta_auto_id_key` 数据流：调用方准备输入，本函数产出可断言结果或错误。
fn is_meta_auto_id_key(key: &[u8]) -> bool {
    // Heuristic matching EncodeTxnMetaKey(DB:x, IID:y, ts): field bytes include "IID:"
    if key.windows(4).any(|w| w == b"IID:")
        || key.windows(4).any(|w| w == b"TID:")
        || key.windows(4).any(|w| w == b"SID:")
        || key.windows(5).any(|w| w == b"TARID")
    {
        return true;
    }
    false
}

/// `EventIterator`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
/// 字段语义与 Go 对照，避免测试桩字段被误读为协议扩展。
/// `EventIterator` 生命周期：构造后是否可变、是否跨线程共享需明确。
struct EventIterator {
    buff: Vec<u8>,
    pos: u32,
    k: Vec<u8>,
    v: Vec<u8>,
    err: Option<String>,
}

/// `NewEventIterator`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
/// 留意空集合、取消上下文与默认值是否保持一致。
/// `NewEventIterator` 数据流：调用方准备输入，本函数产出可断言结果或错误。
fn NewEventIterator(buff: Vec<u8>) -> EventIterator {
    EventIterator {
        buff,
        pos: 0,
        k: Vec::new(),
        v: Vec::new(),
        err: None,
    }
}

/// `EventIterator` 的 impl：方法语义、错误传播与并发约束对齐 Go。
/// 桩实现仅服务测试，不可当作生产路径完备性证明。
/// `EventIterator` 方法边界：非法参数应返回可分类错误而非 panic。
impl EventIterator {
    /// `Next`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `Next` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    fn Next(&mut self) {
        if !self.Valid() {
            return;
        }
        match decode_kv_entry(&self.buff[self.pos as usize..]) {
            Ok((k, v, pos)) => {
                self.k = k;
                self.v = v;
                self.pos += pos;
            }
            Err(e) => self.err = Some(e),
        }
    }
    /// `Valid`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `Valid` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    fn Valid(&mut self) -> bool {
        if self.err.is_some() {
            return false;
        }
        self.pos < self.buff.len() as u32
    }
    /// `Key`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `Key` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    fn Key(&self) -> &[u8] {
        &self.k
    }
    /// `Value`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `Value` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    fn Value(&self) -> &[u8] {
        &self.v
    }
    /// `GetError`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `GetError` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    fn GetError(&self) -> Option<&str> {
        self.err.as_deref()
    }
}

/// `decode_kv_entry`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
/// 留意空集合、取消上下文与默认值是否保持一致。
/// `decode_kv_entry` 数据流：调用方准备输入，本函数产出可断言结果或错误。
fn decode_kv_entry(buff: &[u8]) -> std::result::Result<(Vec<u8>, Vec<u8>, u32), String> {
    if buff.len() < 8 {
        // 断言/错误路径：验证与 Go 测试相同的边界、计数或错误分类。
        return Err("invalid buff".into());
    }
    let mut pos: u32 = 0;
    let k_len = u32::from_le_bytes(buff[0..4].try_into().unwrap());
    pos += 4;
    if (buff.len() as u32) < 8 + k_len {
        // 断言/错误路径：验证与 Go 测试相同的边界、计数或错误分类。
        return Err("invalid buff".into());
    }
    let k = buff[pos as usize..(pos + k_len) as usize].to_vec();
    pos += k_len;
    let v_len = u32::from_le_bytes(buff[pos as usize..pos as usize + 4].try_into().unwrap());
    pos += 4;
    if (buff.len() as u32) < 8 + k_len + v_len {
        // 断言/错误路径：验证与 Go 测试相同的边界、计数或错误分类。
        return Err("invalid buff".into());
    }
    let v = buff[pos as usize..(pos + v_len) as usize].to_vec();
    pos += v_len;
    Ok((k, v, pos))
}

/// `sha256_digest`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
/// 留意空集合、取消上下文与默认值是否保持一致。
/// `sha256_digest` 数据流：调用方准备输入，本函数产出可断言结果或错误。
pub fn sha256_digest(data: &[u8]) -> Vec<u8> {
    // Prefer system shasum on darwin; fallback to pure Rust.
    use std::io::Write;
    use std::process::{Command, Stdio};
    if let Ok(mut child) = Command::new("shasum")
        .args(["-a", "256", "-b"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
    {
        if let Some(stdin) = child.stdin.as_mut() {
            let _ = stdin.write_all(data);
        }
        if let Ok(out) = child.wait_with_output() {
            if out.status.success() {
                let text = String::from_utf8_lossy(&out.stdout);
                let hex = text.split_whitespace().next().unwrap_or("");
                if let Ok(bytes) = hex::decode(hex) {
                    if bytes.len() == 32 {
                        return bytes;
                    }
                }
            }
        }
    }
    pure_sha256(data).to_vec()
}

/// `pure_sha256`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
/// 留意空集合、取消上下文与默认值是否保持一致。
/// `pure_sha256` 数据流：调用方准备输入，本函数产出可断言结果或错误。
fn pure_sha256(mut message: &[u8]) -> [u8; 32] {
    // Compact SHA-256 (public domain style).
    /// `rotr`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `rotr` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    fn rotr(x: u32, n: u32) -> u32 {
        (x >> n) | (x << (32 - n))
    }
    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];
    /// `K`：与 Go 常量同义的阈值/阈值阈值，改动前先对照 Go。
    /// 调整阈值前确认是否影响重试次数或批大小语义。
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];
    let bit_len = (message.len() as u64) * 8;
    let mut msg = message.to_vec();
    msg.push(0x80);
    while (msg.len() % 64) != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bit_len.to_be_bytes());
    for chunk in msg.chunks_exact(64) {
        let mut w = [0u32; 64];
        for i in 0..16 {
            w[i] = u32::from_be_bytes(chunk[i * 4..i * 4 + 4].try_into().unwrap());
        }
        for i in 16..64 {
            let s0 = rotr(w[i - 15], 7) ^ rotr(w[i - 15], 18) ^ (w[i - 15] >> 3);
            let s1 = rotr(w[i - 2], 17) ^ rotr(w[i - 2], 19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }
        let mut a = h[0];
        let mut b = h[1];
        let mut c = h[2];
        let mut d = h[3];
        let mut e = h[4];
        let mut f = h[5];
        let mut g = h[6];
        let mut hh = h[7];
        for i in 0..64 {
            let s1 = rotr(e, 6) ^ rotr(e, 11) ^ rotr(e, 25);
            let ch = (e & f) ^ ((!e) & g);
            let t1 = hh
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = rotr(a, 2) ^ rotr(a, 13) ^ rotr(a, 22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);
            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }
        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
        h[5] = h[5].wrapping_add(f);
        h[6] = h[6].wrapping_add(g);
        h[7] = h[7].wrapping_add(hh);
    }
    let mut out = [0u8; 32];
    for (i, v) in h.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&v.to_be_bytes());
    }
    let _ = message;
    out
}

/// `NewMigration`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
/// 留意空集合、取消上下文与默认值是否保持一致。
/// `NewMigration` 数据流：调用方准备输入，本函数产出可断言结果或错误。
pub fn NewMigration() -> Migration {
    Migration::default()
}
