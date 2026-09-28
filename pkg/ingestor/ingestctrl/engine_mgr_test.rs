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

// EngineManager 生命周期与外部引擎查询单元测试。
//
// 覆盖 open/close/reset/cleanup、未知外部引擎统计为 None，以及
// cleanupAllLocalEngines 清空本地 file size 视图。

#![allow(dead_code)]
#![allow(non_snake_case)]

use std::sync::Arc;

use crate::engine_mgr::{StoreHelper, newEngineManager};
use crate::local::BackendConfig;
use crate::{CancellationToken, EngineId, Result};

/// 测试用 StoreHelper：固定返回 (42, 7) TSO 与 api-v2 codec。
struct TestStoreHelper;

impl StoreHelper for TestStoreHelper {
    fn GetTS(&self, token: &CancellationToken) -> Result<(i64, i64)> {
        token.check()?;
        Ok((42, 7))
    }

    fn GetTiKVCodec(&self) -> String {
        "api-v2".to_owned()
    }
}

// getBackendConfig 对应 Go 测试辅助函数：为每个测试创建独立 sorted-kv 临时目录。
/// 为每个测试生成带唯一临时目录的 BackendConfig。
pub fn getBackendConfig() -> BackendConfig {
    let mut config = BackendConfig::default();
    config.local_store_dir = std::env::temp_dir()
        .join(format!("astersql-ingestctrl-{}", EngineId::new()))
        .to_string_lossy()
        .into_owned();
    config.worker_concurrency = 8;
    config
}

// TestEngineManager 对应 Go 的同名测试：覆盖 open、close、reset、cleanup 和本地目录清空。
/// 覆盖 open 分配 TSO、close、未知 ID 失败、reset、cleanup 与 codec 透传。
#[test]
pub fn TestEngineManager() {
    let config = getBackendConfig();
    let path = config.local_store_dir.clone();
    let manager = newEngineManager(config, Arc::new(TestStoreHelper)).unwrap();
    let token = CancellationToken::default();
    let engine_id = EngineId::new();
    let engine = manager.openEngine(&token, engine_id, 96, 32).unwrap();
    // open 后应已写入非零合成 TSO
    assert_ne!(
        0,
        engine
            .engine_meta
            .ts
            .load(std::sync::atomic::Ordering::Acquire)
    );
    assert_eq!(1, manager.engineFileSizes().len());
    manager.closeEngine(engine_id, false).unwrap();
    assert_eq!(0, manager.getImportedKVCount(engine_id));
    assert!(manager.closeEngine(EngineId::new(), false).is_err());
    manager.resetEngine(&token, engine_id, false).unwrap();
    manager.cleanupEngine(EngineId::new()).unwrap();
    manager.cleanupEngine(engine_id).unwrap();
    assert!(manager.engineFileSizes().is_empty());
    assert_eq!("api-v2", manager.GetTiKVCodec());
    manager.close();
    assert!(!std::path::Path::new(&path).exists());
}

/// 与 Go 的 LoadOrStore/open 语义一致：重复打开同一 ID 不新增引擎也不报错。
#[test]
pub fn open_engine_is_idempotent_for_an_existing_id() {
    let config = getBackendConfig();
    let path = config.local_store_dir.clone();
    let manager = newEngineManager(config, Arc::new(TestStoreHelper)).unwrap();
    let token = CancellationToken::default();
    let engine_id = EngineId::new();

    let first = manager.openEngine(&token, engine_id, 96, 32).unwrap();
    let second = manager.openEngine(&token, engine_id, 96, 32).unwrap();

    assert!(Arc::ptr_eq(&first, &second));
    assert_eq!(1, manager.engineFileSizes().len());
    manager.close();
    assert!(!std::path::Path::new(&path).exists());
}

/// Go 的 resetEngine 对未知本地/外部引擎记录告警后返回成功。
#[test]
pub fn reset_missing_engine_is_a_noop() {
    let config = getBackendConfig();
    let path = config.local_store_dir.clone();
    let manager = newEngineManager(config, Arc::new(TestStoreHelper)).unwrap();

    manager
        .resetEngine(&CancellationToken::default(), EngineId::new(), true)
        .unwrap();

    manager.close();
    assert!(!std::path::Path::new(&path).exists());
}

// TestGetExternalEngineKVStatistics 对应 Go 的同名测试：未知外部引擎返回零统计。
/// 未注册外部引擎时 getExternalEngineKVStatistics 返回 None。
#[test]
pub fn TestGetExternalEngineKVStatistics() {
    let config = getBackendConfig();
    let path = config.local_store_dir.clone();
    let manager = newEngineManager(config, Arc::new(TestStoreHelper)).unwrap();
    assert_eq!(None, manager.getExternalEngineKVStatistics(EngineId::new()));
    manager.close();
    assert!(!std::path::Path::new(&path).exists());
}

// TestCleanupAllLocalEnginesLogsErrorOnly 对应 Go 测试：cleanupAllLocalEngines 记录错误但清空 file size 视图。
/// 打开多个引擎后 cleanupAllLocalEngines 应清空 engineFileSizes。
#[test]
pub fn TestCleanupAllLocalEnginesLogsErrorOnly() {
    let config = getBackendConfig();
    let path = config.local_store_dir.clone();
    let manager = newEngineManager(config, Arc::new(TestStoreHelper)).unwrap();
    let token = CancellationToken::default();
    manager.openEngine(&token, EngineId::new(), 64, 16).unwrap();
    manager.openEngine(&token, EngineId::new(), 64, 16).unwrap();
    manager.cleanupAllLocalEngines();
    assert!(manager.engineFileSizes().is_empty());
    manager.close();
    assert!(!std::path::Path::new(&path).exists());
}
