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

// 本地/外部 Engine 生命周期管理。
//
// `EngineManager` 负责在本地排序目录中打开、加锁、关闭、重置与清理 Engine，
// 分配 TSO（时间戳预言机）写入引擎元数据，并维护外部引擎注册表与重复检测
// 共享缓冲；配合 `DiskUsage`/`Writer` 完成导入控制面调度。

use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use crate::engine::{
    Engine, IMPORT_MUTEX_STATE_CLOSE, IMPORT_MUTEX_STATE_IMPORT, IMPORT_MUTEX_STATE_OPEN, Writer,
};
use crate::iterator::{DupDetectKeyAdapter, KeyAdapter, NoopKeyAdapter};
use crate::local::BackendConfig;
use crate::{CancellationToken, ConflictInfo, EngineFileSize, EngineId, Error, KeyRange, Result};

/// 测试开关：部分路径可跳过真实磁盘行为。
pub static RUN_IN_TEST: AtomicBool = AtomicBool::new(false);
/// 内存测试模式：open 时不创建真实目录。
pub static IN_MEM_TEST: AtomicBool = AtomicBool::new(false);

/// 存储辅助：获取 TSO 与 TiKV API 编解码版本。
pub trait StoreHelper: Send + Sync {
    fn GetTS(&self, token: &CancellationToken) -> Result<(i64, i64)>;
    fn GetTiKVCodec(&self) -> String;
}

/// 外部引擎抽象：统计、键范围、Region 切分与关闭。
pub trait ExternalEngine: Send + Sync {
    fn LoadIngestData(
        &self,
        _: &astersql_ingestor_engineapi::Context,
        _: &std::sync::mpsc::SyncSender<astersql_ingestor_engineapi::DataAndRanges>,
    ) -> std::result::Result<(), astersql_ingestor_engineapi::EngineError> {
        Err(Box::new(Error::InvalidArgument(
            "external engine has no data loader".into(),
        )))
    }
    fn SetWorkerPool(&self, _: Arc<dyn crate::import_pipeline::ImportPoolTuner>) {}
    fn GetTotalLoadedKVsCount(&self) -> i64 {
        self.KVStatistics().1
    }
    fn ConflictFiles(&self) -> Vec<String> {
        Vec::new()
    }
    fn ID(&self) -> String;
    fn KVStatistics(&self) -> (i64, i64);
    fn ImportedStatistics(&self) -> (i64, i64);
    fn ConflictInfo(&self) -> ConflictInfo;
    fn GetKeyRange(&self) -> Result<KeyRange>;
    fn GetRegionSplitKeys(&self) -> Result<Vec<Vec<u8>>>;
    fn Close(&self) -> Result<()>;
}

/// 管理本地与外部 Engine 的注册表、目录与共享重复缓冲。
pub struct EngineManager {
    pub config: BackendConfig,
    store_helper: Arc<dyn StoreHelper>,
    engines: Mutex<HashMap<EngineId, Arc<Engine>>>,
    external_engines: Mutex<HashMap<EngineId, Arc<dyn ExternalEngine>>>,
    duplicate_data: Arc<Mutex<Vec<crate::KvPair>>>,
    key_adapter: Arc<dyn KeyAdapter>,
    closed: AtomicBool,
}

/// 准备排序目录并根据是否开启重复检测选择 KeyAdapter。
pub fn newEngineManager(
    config: BackendConfig,
    helper: Arc<dyn StoreHelper>,
) -> Result<EngineManager> {
    prepareSortDir(&config)?;
    let key_adapter: Arc<dyn KeyAdapter> = if config.duplicate_detection {
        Arc::new(DupDetectKeyAdapter)
    } else {
        Arc::new(NoopKeyAdapter)
    };
    Ok(EngineManager {
        config,
        store_helper: helper,
        engines: Mutex::new(HashMap::new()),
        external_engines: Mutex::new(HashMap::new()),
        duplicate_data: Arc::new(Mutex::new(Vec::new())),
        key_adapter,
        closed: AtomicBool::new(false),
    })
}

impl EngineManager {
    /// 对指定引擎尝试读锁并返回句柄。
    pub fn rLockEngine(&self, engine_id: EngineId) -> Option<Arc<Engine>> {
        let engine = self.engines.lock().ok()?.get(&engine_id).cloned()?;
        engine.tryRLock().then_some(engine)
    }

    /// 以给定状态位对引擎加独占锁。
    pub fn lockEngine(&self, engine_id: EngineId, state: u32) -> Option<Arc<Engine>> {
        let engine = self.engines.lock().ok()?.get(&engine_id).cloned()?;
        engine.lockUnless(state, 0).then_some(engine)
    }

    /// 尝试对所有本地引擎加读锁，返回成功加锁的子集。
    pub fn tryRLockAllEngines(&self) -> Vec<Arc<Engine>> {
        self.engines
            .lock()
            .map(|engines| {
                engines
                    .values()
                    .filter(|engine| engine.tryRLock())
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
    }

    /// 对所有本地引擎尝试 `lockUnless`，返回成功加锁的子集。
    pub fn lockAllEnginesUnless(&self, state: u32, ignore_mask: u32) -> Vec<Arc<Engine>> {
        self.engines
            .lock()
            .map(|engines| {
                engines
                    .values()
                    .filter(|engine| engine.lockUnless(state, ignore_mask))
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
    }

    /// 刷盘占位：当前实现仅获取并释放读锁。
    pub fn flushEngine(&self, engine_id: EngineId) -> Result<()> {
        let engine = self
            .rLockEngine(engine_id)
            .ok_or_else(|| Error::NotFound(format!("engine {engine_id}")))?;
        engine.rUnlock();
        Ok(())
    }

    /// 对所有可读锁引擎执行 flush（当前为解锁占位）。
    pub fn flushAllEngines(&self) -> Result<()> {
        let engines = self.tryRLockAllEngines();
        for engine in engines {
            engine.rUnlock();
        }
        Ok(())
    }

    /// 打开新本地引擎：建目录、加 OPEN 锁、分配 TSO 并注册。
    pub fn openEngine(
        &self,
        token: &CancellationToken,
        engine_id: EngineId,
        region_split_size: i64,
        region_split_keys: i64,
    ) -> Result<Arc<Engine>> {
        token.check()?;
        if self.closed.load(Ordering::Acquire) {
            return Err(Error::Closed);
        }
        let mut engines = self.engines.lock().map_err(|_| Error::Poisoned)?;
        // Go uses LoadOrStore here: opening an already registered engine is
        // idempotent and must not create a second registry entry.
        if let Some(engine) = engines.get(&engine_id) {
            return Ok(Arc::clone(engine));
        }
        let engine_path = Path::new(&self.config.local_store_dir).join(engine_id.to_string());
        if !IN_MEM_TEST.load(Ordering::Relaxed) {
            fs::create_dir_all(&engine_path)?;
        }
        let engine = Arc::new(Engine::new(engine_id, region_split_size, region_split_keys));
        engine.lockUnless(IMPORT_MUTEX_STATE_OPEN, 0);
        self.allocateTSIfNotExists(token, &engine)?;
        engine.unlock();
        engines.insert(engine_id, Arc::clone(&engine));
        Ok(engine)
    }

    /// 注册外部引擎到管理器。
    pub fn registerExternalEngine(
        &self,
        id: EngineId,
        engine: Arc<dyn ExternalEngine>,
    ) -> Result<()> {
        self.external_engines
            .lock()
            .map_err(|_| Error::Poisoned)?
            .insert(id, engine);
        Ok(())
    }

    /// 关闭引擎；`clean` 为真时清理目录并从注册表移除。
    pub fn closeEngine(&self, engine_id: EngineId, clean: bool) -> Result<()> {
        let engine = self
            .lockEngine(engine_id, IMPORT_MUTEX_STATE_CLOSE)
            .ok_or_else(|| Error::NotFound(format!("engine {engine_id}")))?;
        engine.finishWrite()?;
        engine.Close()?;
        engine.unlock();
        if clean {
            engine.Cleanup(Path::new(&self.config.local_store_dir))?;
            self.engines
                .lock()
                .map_err(|_| Error::Poisoned)?
                .remove(&engine_id);
        }
        Ok(())
    }

    /// 关闭旧引擎后以相同 ID 重新打开；可选是否保留新分配的 TSO。
    pub fn resetEngine(
        &self,
        token: &CancellationToken,
        engine_id: EngineId,
        allocate_ts: bool,
    ) -> Result<()> {
        let Some(old) = self.lockEngine(engine_id, IMPORT_MUTEX_STATE_IMPORT) else {
            // Go treats an unknown engine as an already-reset no-op.
            return Ok(());
        };
        old.Close()?;
        old.unlock();
        self.engines
            .lock()
            .map_err(|_| Error::Poisoned)?
            .remove(&engine_id);
        let engine = self.openEngine(
            token,
            engine_id,
            old.GetRegionSplitKeys()?.len().max(1) as i64,
            1,
        )?;
        if !allocate_ts {
            // 调用方要求不保留新 TSO，则清零
            engine.engine_meta.ts.store(0, Ordering::Release);
        }
        Ok(())
    }

    /// 若引擎尚未有 ts，则从 StoreHelper 取物理/逻辑 TSO 合成写入。
    pub fn allocateTSIfNotExists(&self, token: &CancellationToken, engine: &Engine) -> Result<()> {
        if engine.engine_meta.ts.load(Ordering::Acquire) != 0 {
            return Ok(());
        }
        let (physical, logical) = self.store_helper.GetTS(token)?;
        if physical < 0 || logical < 0 {
            return Err(Error::InvalidData("negative TSO".into()));
        }
        // TiDB/PD TSO：高 46 位物理时间 + 低 18 位逻辑计数
        engine.engine_meta.ts.store(
            ((physical as u64) << 18) | logical as u64,
            Ordering::Release,
        );
        Ok(())
    }

    /// 关闭并清理本地与同 ID 外部引擎。
    pub fn cleanupEngine(&self, engine_id: EngineId) -> Result<()> {
        if let Some(engine) = self
            .engines
            .lock()
            .map_err(|_| Error::Poisoned)?
            .remove(&engine_id)
        {
            engine.Close()?;
            engine.Cleanup(Path::new(&self.config.local_store_dir))?;
        }
        if let Some(engine) = self
            .external_engines
            .lock()
            .map_err(|_| Error::Poisoned)?
            .remove(&engine_id)
        {
            engine.Close()?;
        }
        Ok(())
    }

    /// 尽力清理全部本地引擎（忽略单引擎错误）。
    pub fn cleanupAllLocalEngines(&self) {
        let ids = self
            .engines
            .lock()
            .map(|map| map.keys().copied().collect::<Vec<_>>())
            .unwrap_or_default();
        for id in ids {
            let _ = self.cleanupEngine(id);
        }
    }

    /// 为指定引擎创建批量 Writer。
    pub fn localWriter(&self, engine_id: EngineId, batch_size: usize) -> Result<Writer> {
        let engine = self
            .engines
            .lock()
            .map_err(|_| Error::Poisoned)?
            .get(&engine_id)
            .cloned()
            .ok_or_else(|| Error::NotFound(format!("engine {engine_id}")))?;
        Ok(Writer::new(engine, batch_size))
    }

    /// 汇总所有本地引擎的文件大小视图。
    pub fn engineFileSizes(&self) -> Vec<EngineFileSize> {
        self.engines
            .lock()
            .map(|engines| {
                engines
                    .values()
                    .map(|engine| engine.getEngineFileSize())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// 查询已导入键计数；未知引擎返回 0。
    pub fn getImportedKVCount(&self, id: EngineId) -> i64 {
        self.engines
            .lock()
            .ok()
            .and_then(|engines| engines.get(&id).map(|engine| engine.ImportedStatistics().1))
            .unwrap_or(0)
    }

    /// 外部引擎 KV 统计；不存在返回 None。
    pub fn getExternalEngineKVStatistics(&self, id: EngineId) -> Option<(i64, i64)> {
        self.external_engines
            .lock()
            .ok()?
            .get(&id)
            .map(|engine| engine.KVStatistics())
    }

    /// 外部引擎冲突信息；不存在返回默认空。
    pub fn getExternalEngineConflictInfo(&self, id: EngineId) -> ConflictInfo {
        self.external_engines
            .lock()
            .ok()
            .and_then(|engines| engines.get(&id).map(|engine| engine.ConflictInfo()))
            .unwrap_or_default()
    }

    /// 按 ID 取外部引擎句柄。
    pub fn getExternalEngine(&self, id: EngineId) -> Option<Arc<dyn ExternalEngine>> {
        self.external_engines.lock().ok()?.get(&id).cloned()
    }

    /// 所有本地引擎内存占用之和。
    pub fn totalMemoryConsume(&self) -> i64 {
        self.engines
            .lock()
            .map(|engines| {
                engines
                    .values()
                    .map(|engine| engine.TotalMemorySize())
                    .sum()
            })
            .unwrap_or(0)
    }

    /// 管理器级共享重复 KV 缓冲。
    pub fn getDuplicateData(&self) -> Arc<Mutex<Vec<crate::KvPair>>> {
        Arc::clone(&self.duplicate_data)
    }
    /// 当前 KeyAdapter（重复检测或 Noop）。
    pub fn getKeyAdapter(&self) -> Arc<dyn KeyAdapter> {
        Arc::clone(&self.key_adapter)
    }
    /// 透传 StoreHelper 的 TiKV codec 标识。
    pub fn GetTiKVCodec(&self) -> String {
        self.store_helper.GetTiKVCodec()
    }

    /// 关闭管理器：清理本地引擎并关闭全部外部引擎。
    pub fn close(&self) {
        if self.closed.swap(true, Ordering::AcqRel) {
            return;
        }
        self.cleanupAllLocalEngines();
        if let Ok(mut external) = self.external_engines.lock() {
            for engine in external.values() {
                let _ = engine.Close();
            }
            external.clear();
        }
        // BackendConfig currently has no checkpoint flag. This corresponds to
        // Go's checkpoint-disabled branch, which removes the local sort root.
        let _ = fs::remove_dir_all(&self.config.local_store_dir);
    }
}

/// 校验并创建本地排序存储目录。
pub fn prepareSortDir(config: &BackendConfig) -> Result<()> {
    if config.local_store_dir.is_empty() {
        return Err(Error::InvalidArgument(
            "local store directory is empty".into(),
        ));
    }
    if !IN_MEM_TEST.load(Ordering::Relaxed) {
        fs::create_dir_all(&config.local_store_dir)?;
    }
    Ok(())
}
