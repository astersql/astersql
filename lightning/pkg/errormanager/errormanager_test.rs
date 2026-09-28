// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
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

// 自动补充的这个文件用来守住 Rust 端对 Go 契约的可观测行为。
// 注释会重点说明每个测试在搭建什么场景、锁定哪些断言。
// 这样阅读者可以更快区分契约断言和场景铺垫两类代码。
//! Rust equivalents of `errormanager_test.go`.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Barrier, Mutex};
use std::time::Duration;

use crate::atomic;
use crate::config;
use crate::context;
use crate::kv;
use crate::log;
use crate::sql::{self, SqlValue};
use crate::tidbtbl;
use crate::util;
use crate::*;

// 自动补充的`base_config` 是测试文件用来搭建场景的辅助部件。
// 它通常把样例数据、框架对象或重复断言逻辑收敛起来。
// 这样主测例就能更直接地聚焦契约行为而不是铺垫细节。
// 阅读它时可以关注“场景是怎么被搭出来的”。
fn base_config() -> config::Config {
    let mut cfg = config::Config::NewConfig();
    cfg.TikvImporter.Backend = config::BackendLocal.to_string();
    cfg.Conflict.Strategy = config::ReplaceOnDup;
    cfg.App.TaskInfoSchemaName = "lightning_task_info".into();
    cfg
}

// 自动补充的`record_key` 是测试文件用来搭建场景的辅助部件。
// 它通常把样例数据、框架对象或重复断言逻辑收敛起来。
// 这样主测例就能更直接地聚焦契约行为而不是铺垫细节。
// 阅读它时可以关注“场景是怎么被搭出来的”。
fn record_key(handle: i64) -> Vec<u8> {
    let mut key = vec![b't'; 11];
    key.extend_from_slice(&handle.to_be_bytes());
    key
}

// 自动补充的`pair` 是测试文件用来搭建场景的辅助部件。
// 它通常把样例数据、框架对象或重复断言逻辑收敛起来。
// 这样主测例就能更直接地聚焦契约行为而不是铺垫细节。
// 阅读它时可以关注“场景是怎么被搭出来的”。
fn pair(key: &[u8], value: &[u8]) -> kv::KvPair {
    kv::KvPair {
        Key: key.to_vec(),
        Val: value.to_vec(),
    }
}

// 自动补充的`assert_has_query` 是测试文件用来搭建场景的辅助部件。
// 它通常把样例数据、框架对象或重复断言逻辑收敛起来。
// 这样主测例就能更直接地聚焦契约行为而不是铺垫细节。
// 阅读它时可以关注“场景是怎么被搭出来的”。
fn assert_has_query(log: &[(String, Vec<SqlValue>)], needle: &str) {
    // 断言说明：从这里开始校验上一段场景对外暴露的结果。
    // 这些断言关注的是计数、文本、输出形状或资源清理状态。
    // 它们用来锁住 Rust 端需要与 Go 保持一致的可观测合约。
    // 因此只要这些断言成立，就说明测试地图仍然守住了原始意图。
    // 这也是本次只加注释不改逻辑时最需要被明示的部分。
    assert!(
        log.iter().any(|(query, _)| query.contains(needle)),
        "missing query containing {needle:?}: {log:?}"
    );
}

// TestInit verifies all three Go initialization modes: disabled, V1 only,
// and V1+V2 with type-error storage.
// 自动补充的下面的测试围绕 `test_init` 对应的契约或边界展开。
// 它会用构造好的样本输入来锁定与 Go 一致的可观测结果。
// 断言优先锁定输出、计数或错误文本，而不是实现细节。
// 这也是本次只加注释不改逻辑时最需要保护的契约部分。
#[test]
fn test_init() {
    let db = sql::DB::new_memory();
    let mut cfg = base_config();
    cfg.Conflict.PrecheckConflictBeforeImport = true;
    cfg.App.MaxError.Type.Store(10);
    cfg.Conflict.Threshold = 20;
    cfg.App.TaskInfoSchemaName = "lightning_errors".into();

    let mut em = New(Some(db.clone()), &cfg, log::Logger::L());
    // 断言说明：从这里开始校验上一段场景对外暴露的结果。
    // 这些断言关注的是计数、文本、输出形状或资源清理状态。
    // 它们用来锁住 Rust 端需要与 Go 保持一致的可观测合约。
    // 因此只要这些断言成立，就说明测试地图仍然守住了原始意图。
    // 这也是本次只加注释不改逻辑时最需要被明示的部分。
    assert!(em.conflictV1Enabled);
    assert!(em.conflictV2Enabled);
    // 断言说明：从这里开始校验上一段场景对外暴露的结果。
    // 这些断言关注的是计数、文本、输出形状或资源清理状态。
    // 它们用来锁住 Rust 端需要与 Go 保持一致的可观测合约。
    // 因此只要这些断言成立，就说明测试地图仍然守住了原始意图。
    // 这也是本次只加注释不改逻辑时最需要被明示的部分。
    assert_eq!(cfg.App.MaxError.Type.Load(), em.remainingError.Type.Load());
    assert_eq!(cfg.Conflict.Threshold, em.conflictErrRemain.Load());

    em.remainingError.Type.Store(0);
    em.conflictV1Enabled = false;
    em.conflictV2Enabled = false;
    em.Init(context::Background()).unwrap();
    // 断言说明：从这里开始校验上一段场景对外暴露的结果。
    // 这些断言关注的是计数、文本、输出形状或资源清理状态。
    // 它们用来锁住 Rust 端需要与 Go 保持一致的可观测合约。
    // 因此只要这些断言成立，就说明测试地图仍然守住了原始意图。
    // 这也是本次只加注释不改逻辑时最需要被明示的部分。
    assert!(
        db.exec_log().is_empty(),
        "disabled manager must not issue DDL"
    );

    em.conflictV1Enabled = true;
    em.Init(context::Background()).unwrap();
    let v1_log = db.exec_log();
    // 断言说明：从这里开始校验上一段场景对外暴露的结果。
    // 这些断言关注的是计数、文本、输出形状或资源清理状态。
    // 它们用来锁住 Rust 端需要与 Go 保持一致的可观测合约。
    // 因此只要这些断言成立，就说明测试地图仍然守住了原始意图。
    // 这也是本次只加注释不改逻辑时最需要被明示的部分。
    assert_eq!(v1_log.len(), 3);
    assert_has_query(&v1_log, "CREATE SCHEMA IF NOT EXISTS `lightning_errors`");
    // 断言说明：从这里开始校验上一段场景对外暴露的结果。
    // 这些断言关注的是计数、文本、输出形状或资源清理状态。
    // 它们用来锁住 Rust 端需要与 Go 保持一致的可观测合约。
    // 因此只要这些断言成立，就说明测试地图仍然守住了原始意图。
    // 这也是本次只加注释不改逻辑时最需要被明示的部分。
    assert_has_query(
        &v1_log,
        "CREATE TABLE IF NOT EXISTS `lightning_errors`.conflict_error_v4",
    );
    // 断言说明：从这里开始校验上一段场景对外暴露的结果。
    // 这些断言关注的是计数、文本、输出形状或资源清理状态。
    // 它们用来锁住 Rust 端需要与 Go 保持一致的可观测合约。
    // 因此只要这些断言成立，就说明测试地图仍然守住了原始意图。
    // 这也是本次只加注释不改逻辑时最需要被明示的部分。
    assert_has_query(
        &v1_log,
        "CREATE OR REPLACE VIEW `lightning_errors`.conflict_view",
    );
    // 断言说明：从这里开始校验上一段场景对外暴露的结果。
    // 这些断言关注的是计数、文本、输出形状或资源清理状态。
    // 它们用来锁住 Rust 端需要与 Go 保持一致的可观测合约。
    // 因此只要这些断言成立，就说明测试地图仍然守住了原始意图。
    // 这也是本次只加注释不改逻辑时最需要被明示的部分。
    assert!(
        !v1_log
            .iter()
            .any(|(query, _)| query.contains("type_error_v2")
                || query.contains("conflict_records_v2"))
    );

    em.conflictV2Enabled = true;
    em.remainingError.Type.Store(1);
    em.Init(context::Background()).unwrap();
    let all_log = db.exec_log();
    let combined = &all_log[v1_log.len()..];
    // 断言说明：从这里开始校验上一段场景对外暴露的结果。
    // 这些断言关注的是计数、文本、输出形状或资源清理状态。
    // 它们用来锁住 Rust 端需要与 Go 保持一致的可观测合约。
    // 因此只要这些断言成立，就说明测试地图仍然守住了原始意图。
    // 这也是本次只加注释不改逻辑时最需要被明示的部分。
    assert_eq!(combined.len(), 5);
    assert_has_query(combined, "type_error_v2");
    // 断言说明：从这里开始校验上一段场景对外暴露的结果。
    // 这些断言关注的是计数、文本、输出形状或资源清理状态。
    // 它们用来锁住 Rust 端需要与 Go 保持一致的可观测合约。
    // 因此只要这些断言成立，就说明测试地图仍然守住了原始意图。
    // 这也是本次只加注释不改逻辑时最需要被明示的部分。
    assert_has_query(combined, "conflict_error_v4");
    assert_has_query(combined, "conflict_records_v2");
    let view = combined
        .iter()
        .find(|(query, _)| query.contains("CREATE OR REPLACE VIEW"))
        .map(|(query, _)| query)
        .unwrap();
    // 断言说明：从这里开始校验上一段场景对外暴露的结果。
    // 这些断言关注的是计数、文本、输出形状或资源清理状态。
    // 它们用来锁住 Rust 端需要与 Go 保持一致的可观测合约。
    // 因此只要这些断言成立，就说明测试地图仍然守住了原始意图。
    // 这也是本次只加注释不改逻辑时最需要被明示的部分。
    assert!(view.contains("UNION ALL"));

    db.Close().unwrap();
    // 断言说明：从这里开始校验上一段场景对外暴露的结果。
    // 这些断言关注的是计数、文本、输出形状或资源清理状态。
    // 它们用来锁住 Rust 端需要与 Go 保持一致的可观测合约。
    // 因此只要这些断言成立，就说明测试地图仍然守住了原始意图。
    // 这也是本次只加注释不改逻辑时最需要被明示的部分。
    assert!(db.is_closed());
}

// TestReplaceConflictOneKey preserves the Go call graph: one row-key lookup,
// two encoded-KV lookups, and deletion of only the stale secondary-index key.
// 自动补充的下面的测试围绕 `test_replace_conflict_one_key` 对应的契约或边界展开。
// 它会用构造好的样本输入来锁定与 Go 一致的可观测结果。
// 断言优先锁定输出、计数或错误文本，而不是实现细节。
// 这也是本次只加注释不改逻辑时最需要保护的契约部分。
#[test]
fn test_replace_conflict_one_key() {
    let db = sql::DB::new_memory();
    let row_key = record_key(1);
    let row1 = b"row-1".to_vec();
    let row2 = b"row-2".to_vec();
    let index_key = b"idx-b=6".to_vec();
    let index_value = b"handle=1".to_vec();
    db.push_query_rows(
        "kv_type <> 0",
        vec![
            vec![
                SqlValue::Int64(1),
                SqlValue::Bytes(row_key.clone()),
                SqlValue::Bytes(row1.clone()),
            ],
            vec![
                SqlValue::Int64(2),
                SqlValue::Bytes(row_key.clone()),
                SqlValue::Bytes(row2.clone()),
            ],
        ],
        1,
    );

    let em = New(Some(db.clone()), &base_config(), log::Logger::L());
    em.encode_map.lock().unwrap().push((
        row2.clone(),
        vec![pair(&index_key, &index_value), pair(&row_key, &row2)],
    ));
    em.Init(context::Background()).unwrap();

    let gets = atomic::NewInt64(0);
    let deleted = Arc::new(Mutex::new(Vec::<Vec<u8>>::new()));
    let deleted_out = Arc::clone(&deleted);
    let pool = util::WorkerPool::New(16, "resolve duplicate rows by replace");
    em.ReplaceConflictKeys(
        context::Background(),
        tidbtbl::Table {
            meta: tidbtbl::TableMeta { clustered: true },
            cols: vec![],
        },
        "test",
        &pool,
        |_ctx, key| {
            gets.Add(1);
            if key == row_key {
                Ok(row1.clone())
            } else if key == index_key {
                Ok(index_value.clone())
            } else {
                Err(errors::New(format!("unexpected key {key:?}")))
            }
        },
        move |_ctx, keys| {
            deleted_out.lock().unwrap().extend_from_slice(keys);
            Ok(())
        },
    )
    .unwrap();

    // 断言说明：从这里开始校验上一段场景对外暴露的结果。
    // 这些断言关注的是计数、文本、输出形状或资源清理状态。
    // 它们用来锁住 Rust 端需要与 Go 保持一致的可观测合约。
    // 因此只要这些断言成立，就说明测试地图仍然守住了原始意图。
    // 这也是本次只加注释不改逻辑时最需要被明示的部分。
    assert_eq!(gets.Load(), 3);
    assert_eq!(*deleted.lock().unwrap(), vec![index_key]);
    // 断言说明：从这里开始校验上一段场景对外暴露的结果。
    // 这些断言关注的是计数、文本、输出形状或资源清理状态。
    // 它们用来锁住 Rust 端需要与 Go 保持一致的可观测合约。
    // 因此只要这些断言成立，就说明测试地图仍然守住了原始意图。
    // 这也是本次只加注释不改逻辑时最需要被明示的部分。
    assert_has_query(
        &db.exec_log(),
        "DELETE FROM `lightning_task_info`.conflict_error_v4",
    );

    // The production replace path uses this same dynamic queue. Force a split
    // and observe overlapping workers plus complete worker cleanup.
    let active = AtomicUsize::new(0);
    let max_active = AtomicUsize::new(0);
    pool.RunDynamic(0_usize, |item| {
        if item == 0 {
            return Ok((1..=16).collect());
        }
        let now = active.fetch_add(1, Ordering::SeqCst) + 1;
        max_active.fetch_max(now, Ordering::SeqCst);
        std::thread::sleep(Duration::from_millis(5));
        active.fetch_sub(1, Ordering::SeqCst);
        Ok(Vec::new())
    })
    .unwrap();
    // 断言说明：从这里开始校验上一段场景对外暴露的结果。
    // 这些断言关注的是计数、文本、输出形状或资源清理状态。
    // 它们用来锁住 Rust 端需要与 Go 保持一致的可观测合约。
    // 因此只要这些断言成立，就说明测试地图仍然守住了原始意图。
    // 这也是本次只加注释不改逻辑时最需要被明示的部分。
    assert!(
        max_active.load(Ordering::SeqCst) > 1,
        "WorkerPool(16) must process split tasks concurrently"
    );
    // 断言说明：从这里开始校验上一段场景对外暴露的结果。
    // 这些断言关注的是计数、文本、输出形状或资源清理状态。
    // 它们用来锁住 Rust 端需要与 Go 保持一致的可观测合约。
    // 因此只要这些断言成立，就说明测试地图仍然守住了原始意图。
    // 这也是本次只加注释不改逻辑时最需要被明示的部分。
    assert_eq!(active.load(Ordering::SeqCst), 0);

    let active = AtomicUsize::new(0);
    let rendezvous = Barrier::new(3);
    let err = pool
        .RunDynamic(0_usize, |item| {
            if item == 0 {
                return Ok(vec![1, 2, 3]);
            }
            active.fetch_add(1, Ordering::SeqCst);
            rendezvous.wait();
            if item == 1 {
                active.fetch_sub(1, Ordering::SeqCst);
                return Err(errors::New("first worker error"));
            }
            std::thread::sleep(Duration::from_millis(5));
            active.fetch_sub(1, Ordering::SeqCst);
            Ok(Vec::new())
        })
        .unwrap_err();
    // 断言说明：从这里开始校验上一段场景对外暴露的结果。
    // 这些断言关注的是计数、文本、输出形状或资源清理状态。
    // 它们用来锁住 Rust 端需要与 Go 保持一致的可观测合约。
    // 因此只要这些断言成立，就说明测试地图仍然守住了原始意图。
    // 这也是本次只加注释不改逻辑时最需要被明示的部分。
    assert_eq!(err.Error(), "first worker error");
    assert_eq!(
        active.load(Ordering::SeqCst),
        0,
        "all scoped workers must be joined before returning an error"
    );
    db.Close().unwrap();
}

// TestReplaceConflictOneUniqueKey covers the loser-row insertion phase and
// the subsequent row conflict. It keeps Go's 9 lookups and 3 deleted keys.
// 自动补充的下面的测试围绕 `test_replace_conflict_one_unique_key` 对应的契约或边界展开。
// 它会用构造好的样本输入来锁定与 Go 一致的可观测结果。
// 断言优先锁定输出、计数或错误文本，而不是实现细节。
// 这也是本次只加注释不改逻辑时最需要保护的契约部分。
#[test]
fn test_replace_conflict_one_unique_key() {
    let db = sql::DB::new_memory();
    let index1 = b"unique-b=6".to_vec();
    let index3 = b"unique-b=4".to_vec();
    let index1_value = b"handle=1".to_vec();
    let index2_value = b"handle=2".to_vec();
    let index3_value = b"handle=3".to_vec();
    let index4_value = b"handle=4".to_vec();
    let row1_key = record_key(1);
    let row2_key = record_key(2);
    let row3_key = record_key(3);
    let row4_key = record_key(4);
    let row1 = b"row-1".to_vec();
    let row2 = b"row-2".to_vec();
    let row3 = b"row-3".to_vec();
    let row4 = b"row-4".to_vec();
    let stale_index = b"stale-secondary".to_vec();
    let stale_value = b"winner".to_vec();

    db.push_query_rows(
        "kv_type = 0",
        vec![
            vec![
                1.into(),
                index1.clone().into(),
                "uni_b".into(),
                index1_value.clone().into(),
                row1_key.clone().into(),
            ],
            vec![
                2.into(),
                index1.clone().into(),
                "uni_b".into(),
                index2_value.into(),
                row2_key.clone().into(),
            ],
            vec![
                3.into(),
                index3.clone().into(),
                "uni_b".into(),
                index3_value.clone().into(),
                row3_key.clone().into(),
            ],
            vec![
                4.into(),
                index3.clone().into(),
                "uni_b".into(),
                index4_value.into(),
                row4_key.clone().into(),
            ],
        ],
        1,
    );
    db.push_query_rows(
        "kv_type <> 0",
        vec![
            vec![1.into(), row1_key.clone().into(), row1.clone().into()],
            vec![2.into(), row1_key.clone().into(), row3.clone().into()],
        ],
        1,
    );
    db.push_delete_affected(2);
    db.push_delete_affected(0);

    let em = New(Some(db.clone()), &base_config(), log::Logger::L());
    {
        let mut map = em.encode_map.lock().unwrap();
        map.push((row2.clone(), vec![pair(&index1, b"handle=2")]));
        map.push((row4.clone(), vec![pair(&index3, b"handle=4")]));
        map.push((
            row3.clone(),
            vec![pair(&stale_index, &stale_value), pair(&row1_key, &row3)],
        ));
    }
    em.Init(context::Background()).unwrap();

    let gets = atomic::NewInt64(0);
    let deleted = Arc::new(Mutex::new(Vec::<Vec<u8>>::new()));
    let deleted_out = Arc::clone(&deleted);
    let pool = util::WorkerPool::New(16, "resolve duplicate rows by replace");
    em.ReplaceConflictKeys(
        context::Background(),
        tidbtbl::Table {
            meta: tidbtbl::TableMeta { clustered: true },
            cols: vec![],
        },
        "test",
        &pool,
        |_ctx, key| {
            gets.Add(1);
            match key {
                key if key == index1 => Ok(index1_value.clone()),
                key if key == index3 => Ok(index3_value.clone()),
                key if key == row1_key => Ok(row1.clone()),
                key if key == row2_key => Ok(row2.clone()),
                key if key == row4_key => Ok(row4.clone()),
                key if key == stale_index => Ok(stale_value.clone()),
                _ => Err(errors::New(format!("unexpected key {key:?}"))),
            }
        },
        move |_ctx, keys| {
            deleted_out.lock().unwrap().extend_from_slice(keys);
            Ok(())
        },
    )
    .unwrap();

    // 断言说明：从这里开始校验上一段场景对外暴露的结果。
    // 这些断言关注的是计数、文本、输出形状或资源清理状态。
    // 它们用来锁住 Rust 端需要与 Go 保持一致的可观测合约。
    // 因此只要这些断言成立，就说明测试地图仍然守住了原始意图。
    // 这也是本次只加注释不改逻辑时最需要被明示的部分。
    assert_eq!(gets.Load(), 9);
    assert_eq!(
        *deleted.lock().unwrap(),
        vec![row2_key, row4_key, stale_index]
    );
    let log = db.exec_log();
    let inserted = log
        .iter()
        .find(|(query, _)| query.contains("INSERT INTO") && query.contains("conflict_error_v4"))
        .expect("loser rows must be persisted");
    // 断言说明：从这里开始校验上一段场景对外暴露的结果。
    // 这些断言关注的是计数、文本、输出形状或资源清理状态。
    // 它们用来锁住 Rust 端需要与 Go 保持一致的可观测合约。
    // 因此只要这些断言成立，就说明测试地图仍然守住了原始意图。
    // 这也是本次只加注释不改逻辑时最需要被明示的部分。
    assert_eq!(inserted.1.len(), 14, "two loser rows, seven args each");
    assert_eq!(
        log.iter()
            .filter(|(query, _)| query.contains("DELETE FROM"))
            .count(),
        2,
        "cleanup repeats until RowsAffected is zero"
    );
    db.Close().unwrap();
}

#[test]
fn test_replace_conflict_preserves_keys_for_existing_empty_row_value() {
    let db = sql::DB::new_memory();
    let row_key = record_key(1);
    let stale_row = b"stale-row".to_vec();
    let shared_index_key = b"shared-index".to_vec();
    let shared_index_value = b"current-handle".to_vec();

    db.push_query_rows(
        "kv_type <> 0",
        vec![vec![
            SqlValue::Int64(1),
            SqlValue::Bytes(row_key.clone()),
            SqlValue::Bytes(stale_row.clone()),
        ]],
        1,
    );

    let em = New(Some(db.clone()), &base_config(), log::Logger::L());
    {
        let mut map = em.encode_map.lock().unwrap();
        map.push((
            Vec::new(),
            vec![pair(&shared_index_key, &shared_index_value)],
        ));
        map.push((
            stale_row,
            vec![pair(&shared_index_key, &shared_index_value)],
        ));
    }

    let deleted = Arc::new(Mutex::new(Vec::<Vec<u8>>::new()));
    let deleted_out = Arc::clone(&deleted);
    let pool = util::WorkerPool::New(1, "resolve duplicate rows by replace");
    em.ReplaceConflictKeys(
        context::Background(),
        tidbtbl::Table {
            meta: tidbtbl::TableMeta { clustered: true },
            cols: vec![],
        },
        "test",
        &pool,
        move |_ctx, key| {
            if key == row_key {
                Ok(Vec::new())
            } else if key == shared_index_key {
                Ok(shared_index_value.clone())
            } else {
                Err(errors::New(format!("unexpected key {key:?}")))
            }
        },
        move |_ctx, keys| {
            deleted_out.lock().unwrap().extend_from_slice(keys);
            Ok(())
        },
    )
    .unwrap();

    assert!(
        deleted.lock().unwrap().is_empty(),
        "an index KV belonging to the current empty-valued row must be preserved"
    );
}

#[test]
fn test_replace_conflict_reuses_previous_keep_set_after_missing_row() {
    let db = sql::DB::new_memory();
    let first_row_key = record_key(1);
    let missing_row_key = record_key(2);
    let first_row = b"first-row".to_vec();
    let stale_row = b"stale-row".to_vec();
    let shared_index_key = b"shared-index".to_vec();
    let shared_index_value = b"current-handle".to_vec();

    db.push_query_rows(
        "kv_type <> 0",
        vec![
            vec![
                SqlValue::Int64(1),
                SqlValue::Bytes(first_row_key.clone()),
                SqlValue::Bytes(first_row.clone()),
            ],
            vec![
                SqlValue::Int64(2),
                SqlValue::Bytes(missing_row_key.clone()),
                SqlValue::Bytes(stale_row.clone()),
            ],
        ],
        1,
    );

    let em = New(Some(db), &base_config(), log::Logger::L());
    {
        let mut map = em.encode_map.lock().unwrap();
        map.push((
            first_row.clone(),
            vec![pair(&shared_index_key, &shared_index_value)],
        ));
        map.push((
            stale_row,
            vec![pair(&shared_index_key, &shared_index_value)],
        ));
    }

    let deleted = Arc::new(Mutex::new(Vec::<Vec<u8>>::new()));
    let deleted_out = Arc::clone(&deleted);
    let pool = util::WorkerPool::New(1, "resolve duplicate rows by replace");
    em.ReplaceConflictKeys(
        context::Background(),
        tidbtbl::Table {
            meta: tidbtbl::TableMeta { clustered: true },
            cols: vec![],
        },
        "test",
        &pool,
        move |_ctx, key| {
            if key == first_row_key {
                Ok(first_row.clone())
            } else if key == missing_row_key {
                Err(tikverr::ErrNotFound("missing row"))
            } else if key == shared_index_key {
                Ok(shared_index_value.clone())
            } else {
                Err(errors::New(format!("unexpected key {key:?}")))
            }
        },
        move |_ctx, keys| {
            deleted_out.lock().unwrap().extend_from_slice(keys);
            Ok(())
        },
    )
    .unwrap();

    assert!(
        deleted.lock().unwrap().is_empty(),
        "Go keeps the previous mustKeepKvPairs when the next row is not found"
    );
}

// 自动补充的`manager_for_error_summary` 是测试文件用来搭建场景的辅助部件。
// 它通常把样例数据、框架对象或重复断言逻辑收敛起来。
// 这样主测例就能更直接地聚焦契约行为而不是铺垫细节。
// 阅读它时可以关注“场景是怎么被搭出来的”。
fn manager_for_error_summary() -> ErrorManager {
    let mut cfg = config::Config::default();
    cfg.App.MaxError.Syntax.Store(100);
    cfg.App.MaxError.Charset.Store(100);
    cfg.App.MaxError.Type.Store(100);
    cfg.Conflict.Threshold = 100;
    ErrorManager {
        db: None,
        taskID: 0,
        schema: "error_info".into(),
        configError: cfg.App.MaxError.clone(),
        remainingError: cfg.App.MaxError,
        configConflict: cfg.Conflict,
        conflictErrRemain: atomic::NewInt64(100),
        conflictRecordsRemain: atomic::NewInt64(0),
        conflictV1Enabled: true,
        conflictV2Enabled: false,
        logger: log::Logger::L(),
        recordErrorOnce: atomic::NewBool(false),
        encode_map: Default::default(),
    }
}

// 自动补充的下面的测试围绕 `test_error_mgr_has_error` 对应的契约或边界展开。
// 它会用构造好的样本输入来锁定与 Go 一致的可观测结果。
// 断言优先锁定输出、计数或错误文本，而不是实现细节。
// 这也是本次只加注释不改逻辑时最需要保护的契约部分。
#[test]
fn test_error_mgr_has_error() {
    // 断言说明：从这里开始校验上一段场景对外暴露的结果。
    // 这些断言关注的是计数、文本、输出形状或资源清理状态。
    // 它们用来锁住 Rust 端需要与 Go 保持一致的可观测合约。
    // 因此只要这些断言成立，就说明测试地图仍然守住了原始意图。
    // 这也是本次只加注释不改逻辑时最需要被明示的部分。
    assert!(!manager_for_error_summary().HasError());

    let em = manager_for_error_summary();
    em.remainingError.Syntax.Sub(1);
    // 断言说明：从这里开始校验上一段场景对外暴露的结果。
    // 这些断言关注的是计数、文本、输出形状或资源清理状态。
    // 它们用来锁住 Rust 端需要与 Go 保持一致的可观测合约。
    // 因此只要这些断言成立，就说明测试地图仍然守住了原始意图。
    // 这也是本次只加注释不改逻辑时最需要被明示的部分。
    assert!(em.HasError());

    let em = manager_for_error_summary();
    em.remainingError.Charset.Sub(1);
    // 断言说明：从这里开始校验上一段场景对外暴露的结果。
    // 这些断言关注的是计数、文本、输出形状或资源清理状态。
    // 它们用来锁住 Rust 端需要与 Go 保持一致的可观测合约。
    // 因此只要这些断言成立，就说明测试地图仍然守住了原始意图。
    // 这也是本次只加注释不改逻辑时最需要被明示的部分。
    assert!(em.HasError());

    let em = manager_for_error_summary();
    em.remainingError.Type.Sub(1);
    // 断言说明：从这里开始校验上一段场景对外暴露的结果。
    // 这些断言关注的是计数、文本、输出形状或资源清理状态。
    // 它们用来锁住 Rust 端需要与 Go 保持一致的可观测合约。
    // 因此只要这些断言成立，就说明测试地图仍然守住了原始意图。
    // 这也是本次只加注释不改逻辑时最需要被明示的部分。
    assert!(em.HasError());

    let em = manager_for_error_summary();
    em.conflictErrRemain.Sub(1);
    // 断言说明：从这里开始校验上一段场景对外暴露的结果。
    // 这些断言关注的是计数、文本、输出形状或资源清理状态。
    // 它们用来锁住 Rust 端需要与 Go 保持一致的可观测合约。
    // 因此只要这些断言成立，就说明测试地图仍然守住了原始意图。
    // 这也是本次只加注释不改逻辑时最需要被明示的部分。
    assert!(em.HasError());

    let em = manager_for_error_summary();
    em.remainingError.Syntax.Store(0);
    em.remainingError.Charset.Store(0);
    em.remainingError.Type.Store(0);
    em.conflictErrRemain.Store(0);
    // 断言说明：从这里开始校验上一段场景对外暴露的结果。
    // 这些断言关注的是计数、文本、输出形状或资源清理状态。
    // 它们用来锁住 Rust 端需要与 Go 保持一致的可观测合约。
    // 因此只要这些断言成立，就说明测试地图仍然守住了原始意图。
    // 这也是本次只加注释不改逻辑时最需要被明示的部分。
    assert!(em.HasError());
}

// 自动补充的下面的测试围绕 `test_error_mgr_error_output` 对应的契约或边界展开。
// 它会用构造好的样本输入来锁定与 Go 一致的可观测结果。
// 断言优先锁定输出、计数或错误文本，而不是实现细节。
// 这也是本次只加注释不改逻辑时最需要保护的契约部分。
#[test]
fn test_error_mgr_error_output() {
    let em = manager_for_error_summary();
    // 断言说明：从这里开始校验上一段场景对外暴露的结果。
    // 这些断言关注的是计数、文本、输出形状或资源清理状态。
    // 它们用来锁住 Rust 端需要与 Go 保持一致的可观测合约。
    // 因此只要这些断言成立，就说明测试地图仍然守住了原始意图。
    // 这也是本次只加注释不改逻辑时最需要被明示的部分。
    assert_eq!(em.Output(), "");

    em.remainingError.Syntax.Sub(1);
    // 断言说明：从这里开始校验上一段场景对外暴露的结果。
    // 这些断言关注的是计数、文本、输出形状或资源清理状态。
    // 它们用来锁住 Rust 端需要与 Go 保持一致的可观测合约。
    // 因此只要这些断言成立，就说明测试地图仍然守住了原始意图。
    // 这也是本次只加注释不改逻辑时最需要被明示的部分。
    assert_eq!(
        em.Output(),
        "\nImport Data Error Summary: \n\
+---+-------------+-------------+--------------------------------+\n\
| # | ERROR TYPE  | ERROR COUNT | ERROR DATA TABLE               |\n\
+---+-------------+-------------+--------------------------------+\n\
|\u{1b}[31m 1 \u{1b}[0m|\u{1b}[31m Data Syntax \u{1b}[0m|\u{1b}[31m           1 \u{1b}[0m|\u{1b}[31m `error_info`.`syntax_error_v2` \u{1b}[0m|\n\
+---+-------------+-------------+--------------------------------+\n"
    );

    let mut em = manager_for_error_summary();
    em.remainingError.Syntax.Sub(10);
    em.remainingError.Type.Store(10);
    // 断言说明：从这里开始校验上一段场景对外暴露的结果。
    // 这些断言关注的是计数、文本、输出形状或资源清理状态。
    // 它们用来锁住 Rust 端需要与 Go 保持一致的可观测合约。
    // 因此只要这些断言成立，就说明测试地图仍然守住了原始意图。
    // 这也是本次只加注释不改逻辑时最需要被明示的部分。
    assert_eq!(
        em.Output(),
        "\nImport Data Error Summary: \n\
+---+-------------+-------------+--------------------------------+\n\
| # | ERROR TYPE  | ERROR COUNT | ERROR DATA TABLE               |\n\
+---+-------------+-------------+--------------------------------+\n\
|\u{1b}[31m 1 \u{1b}[0m|\u{1b}[31m Data Type   \u{1b}[0m|\u{1b}[31m          90 \u{1b}[0m|\u{1b}[31m `error_info`.`type_error_v2`   \u{1b}[0m|\n\
|\u{1b}[31m 2 \u{1b}[0m|\u{1b}[31m Data Syntax \u{1b}[0m|\u{1b}[31m          10 \u{1b}[0m|\u{1b}[31m `error_info`.`syntax_error_v2` \u{1b}[0m|\n\
+---+-------------+-------------+--------------------------------+\n"
    );

    let mut em = manager_for_error_summary();
    em.remainingError.Syntax.Store(0);
    em.remainingError.Charset.Store(0);
    em.remainingError.Type.Store(0);
    em.conflictErrRemain.Store(0);
    let output = "\nImport Data Error Summary: \n\
+---+---------------------+-------------+--------------------------------+\n\
| # | ERROR TYPE          | ERROR COUNT | ERROR DATA TABLE               |\n\
+---+---------------------+-------------+--------------------------------+\n\
|\u{1b}[31m 1 \u{1b}[0m|\u{1b}[31m Data Type           \u{1b}[0m|\u{1b}[31m         100 \u{1b}[0m|\u{1b}[31m `error_info`.`type_error_v2`   \u{1b}[0m|\n\
|\u{1b}[31m 2 \u{1b}[0m|\u{1b}[31m Data Syntax         \u{1b}[0m|\u{1b}[31m         100 \u{1b}[0m|\u{1b}[31m `error_info`.`syntax_error_v2` \u{1b}[0m|\n\
|\u{1b}[31m 3 \u{1b}[0m|\u{1b}[31m Charset Error       \u{1b}[0m|\u{1b}[31m         100 \u{1b}[0m|\u{1b}[31m                                \u{1b}[0m|\n\
|\u{1b}[31m 4 \u{1b}[0m|\u{1b}[31m Unique Key Conflict \u{1b}[0m|\u{1b}[31m         100 \u{1b}[0m|\u{1b}[31m `error_info`.`conflict_view`   \u{1b}[0m|\n\
+---+---------------------+-------------+--------------------------------+\n";
    // 断言说明：从这里开始校验上一段场景对外暴露的结果。
    // 这些断言关注的是计数、文本、输出形状或资源清理状态。
    // 它们用来锁住 Rust 端需要与 Go 保持一致的可观测合约。
    // 因此只要这些断言成立，就说明测试地图仍然守住了原始意图。
    // 这也是本次只加注释不改逻辑时最需要被明示的部分。
    assert_eq!(em.Output(), output);

    em.conflictV1Enabled = false;
    em.conflictV2Enabled = true;
    // 断言说明：从这里开始校验上一段场景对外暴露的结果。
    // 这些断言关注的是计数、文本、输出形状或资源清理状态。
    // 它们用来锁住 Rust 端需要与 Go 保持一致的可观测合约。
    // 因此只要这些断言成立，就说明测试地图仍然守住了原始意图。
    // 这也是本次只加注释不改逻辑时最需要被明示的部分。
    assert_eq!(
        em.Output(),
        output,
        "V1 and V2 render the same conflict view"
    );

    em.conflictV1Enabled = true;
    // 断言说明：从这里开始校验上一段场景对外暴露的结果。
    // 这些断言关注的是计数、文本、输出形状或资源清理状态。
    // 它们用来锁住 Rust 端需要与 Go 保持一致的可观测合约。
    // 因此只要这些断言成立，就说明测试地图仍然守住了原始意图。
    // 这也是本次只加注释不改逻辑时最需要被明示的部分。
    assert_eq!(
        em.Output(),
        output,
        "combined V1+V2 must not duplicate the row"
    );

    em.schema = "long_long_long_long_long_long_long_long_dbname".into();
    // 断言说明：从这里开始校验上一段场景对外暴露的结果。
    // 这些断言关注的是计数、文本、输出形状或资源清理状态。
    // 它们用来锁住 Rust 端需要与 Go 保持一致的可观测合约。
    // 因此只要这些断言成立，就说明测试地图仍然守住了原始意图。
    // 这也是本次只加注释不改逻辑时最需要被明示的部分。
    assert_eq!(
        em.Output(),
        "\nImport Data Error Summary: \n\
+---+---------------------+-------------+--------------------------------------------------------------------+\n\
| # | ERROR TYPE          | ERROR COUNT | ERROR DATA TABLE                                                   |\n\
+---+---------------------+-------------+--------------------------------------------------------------------+\n\
|\u{1b}[31m 1 \u{1b}[0m|\u{1b}[31m Data Type           \u{1b}[0m|\u{1b}[31m         100 \u{1b}[0m|\u{1b}[31m `long_long_long_long_long_long_long_long_dbname`.`type_error_v2`   \u{1b}[0m|\n\
|\u{1b}[31m 2 \u{1b}[0m|\u{1b}[31m Data Syntax         \u{1b}[0m|\u{1b}[31m         100 \u{1b}[0m|\u{1b}[31m `long_long_long_long_long_long_long_long_dbname`.`syntax_error_v2` \u{1b}[0m|\n\
|\u{1b}[31m 3 \u{1b}[0m|\u{1b}[31m Charset Error       \u{1b}[0m|\u{1b}[31m         100 \u{1b}[0m|\u{1b}[31m                                                                    \u{1b}[0m|\n\
|\u{1b}[31m 4 \u{1b}[0m|\u{1b}[31m Unique Key Conflict \u{1b}[0m|\u{1b}[31m         100 \u{1b}[0m|\u{1b}[31m `long_long_long_long_long_long_long_long_dbname`.`conflict_view`   \u{1b}[0m|\n\
+---+---------------------+-------------+--------------------------------------------------------------------+\n"
    );
}
