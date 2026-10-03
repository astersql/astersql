// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
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

//! storage 内部行为测试，对齐 Go `storage_internal_test.go`。
//! 焦点：StartAfter 使用大写 hex，以及增量扫描能收录大写命名的 backupmeta。
//! 使用内存 Upstream，不依赖真实对象存储；PD/Sync 桩返回空闲值。
//! 若 StartAfter 误用小写 hex，大写 meta 名会排在游标前被过滤，形成假阴性。
//! 第二则用例用真实风格的长文件名，覆盖 ParseName 与增量过滤联调。
//! 不修改生产逻辑：仅通过注释说明断言意图与桩约束。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use astersql_br_pkg_streamhelper::Store;

use crate::storage::meta_scan_start_after;
use crate::{
    CalculatorDeps, CheckpointCalculatorConfig, Context, Error, NewCalculator, ObjectSyncChecker,
    PDMetaReader, PersistentState, UpstreamStorageReader, WalkOption,
};

/// PD 桩：本测试不依赖全局检查点/store 列表。
/// GetGlobalCheckpoint 返回 0，避免干扰扫描路径选择。
struct StubPDMetaReader;

impl PDMetaReader for StubPDMetaReader {
    fn GetGlobalCheckpointForTask(&self, _ctx: &Context, _task_name: &str) -> Result<u64, Error> {
        Ok(0)
    }

    fn Stores(&self, _ctx: &Context) -> Result<Vec<Store>, Error> {
        Ok(Vec::new())
    }
}

/// Sync 桩：恒为已同步；本文件只测扫描，不等待下游。
struct StubSyncChecker;

impl ObjectSyncChecker for StubSyncChecker {
    fn FileSynced(&self, _ctx: &Context, _name: &str) -> Result<bool, Error> {
        Ok(true)
    }
}

/// 内存对象存储：支持 SubDir/StartAfter 过滤，模拟增量 Walk。
#[derive(Clone)]
struct MemStorage {
    files: Arc<Mutex<HashMap<String, Vec<u8>>>>,
    uri: String,
}

impl MemStorage {
    /// `uri` 仅供 URI() 返回，Walk/Read 不解析它。
    fn new(uri: &str) -> Self {
        Self {
            files: Arc::new(Mutex::new(HashMap::new())),
            uri: uri.to_string(),
        }
    }

    /// 覆盖写入；测试只放空 meta 体亦可。
    fn write_file(&self, path: &str, data: Vec<u8>) {
        self.files.lock().unwrap().insert(path.to_string(), data);
    }
}

impl UpstreamStorageReader for MemStorage {
    fn WalkDir(
        &self,
        _ctx: &Context,
        opt: &WalkOption,
        callback: &mut dyn FnMut(&str, i64) -> Result<(), Error>,
    ) -> Result<(), Error> {
        let mut paths: Vec<String> = self.files.lock().unwrap().keys().cloned().collect();
        // 排序保证与对象存储字典序列举一致，便于 StartAfter 语义。
        paths.sort();
        let prefix = if opt.SubDir.is_empty() {
            String::new()
        } else {
            format!("{}/", opt.SubDir.trim_end_matches('/'))
        };
        for path in paths {
            if !prefix.is_empty() && !path.starts_with(&prefix) {
                continue;
            }
            // 严格大于 StartAfter；等于则跳过，对齐 S3 StartAfter 语义。
            if !opt.StartAfter.is_empty() && path <= opt.StartAfter {
                continue;
            }
            let size = self.files.lock().unwrap()[&path].len() as i64;
            callback(&path, size)?;
        }
        Ok(())
    }

    fn ReadFile(&self, _ctx: &Context, name: &str) -> Result<Vec<u8>, Error> {
        // 扫描测试通常不读内容；保留实现供将来扩展 load 用例。
        self.files
            .lock()
            .unwrap()
            .get(name)
            .cloned()
            .ok_or_else(|| Error::new(format!("file not found: {name}")))
    }

    fn URI(&self) -> String {
        // 返回构造时写入的 scheme，便于 validate_* 路径复用此桩。
        self.uri.clone()
    }
}

/// 断言 StartAfter 游标为大写 hex(ts)+MAX_STORE_ID_SUFFIX。
/// 若误用小写，真实大写 meta 名会排在游标之前被漏扫。
#[test]
fn test_meta_scan_start_after_uses_uppercase_hex_for_uppercase_backup_meta_names() {
    let stalled_synced_ts: u64 = 0x06745A03F7B80004;
    let expected = "v1/backupmeta/06745A03F7B80004FFFFFFFFFFFFFFFF~";
    assert_eq!(expected, meta_scan_start_after(stalled_synced_ts));
}

/// 恢复 stalled SyncedTS 后，迭代器应包含 flushTS 更大的大写命名 meta。
/// 内容可为空：本测试只验证文件名解析与 StartAfter 过滤，不 load JSON。
#[test]
fn test_meta_file_seq_includes_uppercase_meta_names_after_synced_ts() {
    let ctx = Context::Background();
    let upstream = MemStorage::new("file:///tmp/upstream");
    // 两条路径均使用大写 hex 前缀，模拟生产 backupmeta 命名。
    let meta_paths = vec![
        "v1/backupmeta/06745D833D8C001400000000000003E9-d06745D77FC50000El06745D77FC50002Eu06745D833D8C0005.meta".to_string(),
        "v1/backupmeta/06745D856390000400000000000003EC-d06745D77FAC0001Al06745D77FB88000Fu06745D8562C80030.meta".to_string(),
    ];
    for meta_path in &meta_paths {
        upstream.write_file(meta_path, Vec::new());
    }

    // Observer/配置用默认即可；关键是 RestorePersistentState 注入 SyncedTS。
    let mut calc = NewCalculator(
        CalculatorDeps {
            PD: Box::new(StubPDMetaReader),
            Upstream: Box::new(upstream),
            Sync: Box::new(StubSyncChecker),
        },
        CheckpointCalculatorConfig {
            TaskName: "task".into(),
            ..Default::default()
        },
        None,
    )
    .expect("calculator");
    // SyncedTS 小于两条 meta 的 flushTS，且 StartAfter 应落在它们之前。
    calc.RestorePersistentState(PersistentState {
        SyncedTS: 0x06745A03F7B80004,
        ..Default::default()
    })
    .expect("restore state");

    let mut got = Vec::new();
    // 任一 ParseName 失败会 Err，此处 expect 即断言扫描成功。
    for item in calc.new_meta_file_iter(&ctx) {
        let meta_file = item.expect("meta file");
        got.push(meta_file.path);
    }
    got.sort();
    let mut expected = meta_paths;
    expected.sort();
    // 集合相等即可；顺序在断言前统一排序。
    assert_eq!(expected, got);
}

#[test]
fn test_load_empty_meta_file_skips_content_read() {
    let ctx = Context::Background();
    let upstream = MemStorage::new("file:///tmp/upstream");
    let meta_path = "v1/backupmeta/06745D833D8C001400000000000003E9-d0000000000000000l0000000000000000u0000000000000000p0000000000000002.meta";
    // Invalid payload must not be parsed for an empty name.
    upstream.write_file(meta_path, b"invalid metadata payload".to_vec());
    let calc = NewCalculator(
        CalculatorDeps {
            PD: Box::new(StubPDMetaReader),
            Upstream: Box::new(upstream.clone()),
            Sync: Box::new(StubSyncChecker),
        },
        CheckpointCalculatorConfig {
            TaskName: "task".into(),
            ..Default::default()
        },
        None,
    )
    .unwrap();
    let parsed = calc.new_meta_file_iter(&ctx).next().unwrap().unwrap();
    assert!(parsed.empty);
    // Remove the content to prove that loading never touches storage.
    upstream.files.lock().unwrap().clear();
    let (loaded, ignored) = crate::storage::load_meta_file(&ctx, &upstream, parsed).unwrap();
    assert!(!ignored);
    assert_eq!(loaded.path, meta_path);
    assert_eq!(loaded.flush_ts, 0x06745D833D8C0014);
    assert_eq!(loaded.store_id, 1001);
    assert!(loaded.empty);
    assert!(loaded.data_file_paths.is_empty());
}

#[test]
fn test_load_empty_meta_file_with_zero_store_id_is_ignored() {
    let (loaded, ignored) = crate::storage::load_meta_file(&Context::Background(), &MemStorage::new("file:///tmp/upstream"), crate::storage::parsedMetaFile {
        path: "v1/backupmeta/00000000000000140000000000000000-d0000000000000000l0000000000000000u0000000000000000p0000000000000002.meta".into(),
        flush_ts: 20, store_id: 0, empty: true,
    }).unwrap();
    assert!(ignored);
    assert!(loaded.empty);
    assert_eq!(loaded.flush_ts, 20);
    assert_eq!(loaded.store_id, 0);
    assert!(loaded.data_file_paths.is_empty());
}
