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

// 自动补充的这个文件用来守住 Rust 端对 Go 契约的可观测行为。
// 注释会重点说明每个测试在搭建什么场景、锁定哪些断言。
// 这样阅读者可以更快区分契约断言和场景铺垫两类代码。
//! Go-equivalent tests for `dup_detect_test.go`.

use crate::duplicate::Handler;
use crate::extsort::ExternalSorter;
use crate::*;
use astersql_lightning_pkg_checkpoints as checkpoints;
use std::collections::HashMap;
use std::sync::Arc;

// 自动补充的`example_handle_key` 是测试文件用来搭建场景的辅助部件。
// 它通常把样例数据、框架对象或重复断言逻辑收敛起来。
// 这样主测例就能更直接地聚焦契约行为而不是铺垫细节。
// 阅读它时可以关注“场景是怎么被搭出来的”。
/// Matches stub `tablecodec` record-key layout used by `decodeIndexID`.
fn example_handle_key() -> Vec<u8> {
    let mut k = vec![0u8; 19];
    k[0] = b't';
    k[10] = b'r';
    k[11..19].copy_from_slice(&22i64.to_be_bytes());
    k
}

const EXAMPLE_INDEX_ID: i64 = 23;

// 自动补充的`example_index_key` 是测试文件用来搭建场景的辅助部件。
// 它通常把样例数据、框架对象或重复断言逻辑收敛起来。
// 这样主测例就能更直接地聚焦契约行为而不是铺垫细节。
// 阅读它时可以关注“场景是怎么被搭出来的”。
/// Matches stub `tablecodec` index-key layout used by `decodeIndexID`.
fn example_index_key() -> Vec<u8> {
    let mut k = vec![0u8; 19];
    k[0] = b't';
    k[10] = b'i';
    k[11..19].copy_from_slice(&EXAMPLE_INDEX_ID.to_be_bytes());
    k
}

// 自动补充的下面的测试围绕 `test_error_on_dup` 对应的契约或边界展开。
// 它会用构造好的样本输入来锁定与 Go 一致的可观测结果。
// 断言优先锁定输出、计数或错误文本，而不是实现细节。
// 这也是本次只加注释不改逻辑时最需要保护的契约部分。
/// Corresponds to Go `TestErrorOnDup`.
#[test]
fn test_error_on_dup() {
    let mut h = errorOnDup::default();
    h.Begin(&example_handle_key()).unwrap();
    h.Append(&[1]).unwrap();
    h.Append(&[2]).unwrap();
    let err = h.End().unwrap_err();
    // 断言说明：从这里开始校验上一段场景对外暴露的结果。
    // 这些断言关注的是计数、文本、输出形状或资源清理状态。
    // 它们用来锁住 Rust 端需要与 Go 保持一致的可观测合约。
    // 因此只要这些断言成立，就说明测试地图仍然守住了原始意图。
    // 这也是本次只加注释不改逻辑时最需要被明示的部分。
    assert!(errors::Is(&err, &ErrDuplicateKey()));
    let cause = errors::Cause(&err);
    // 断言说明：从这里开始校验上一段场景对外暴露的结果。
    // 这些断言关注的是计数、文本、输出形状或资源清理状态。
    // 它们用来锁住 Rust 端需要与 Go 保持一致的可观测合约。
    // 因此只要这些断言成立，就说明测试地图仍然守住了原始意图。
    // 这也是本次只加注释不改逻辑时最需要被明示的部分。
    assert!(
        cause.Error().contains(&CONFLICT_ON_HANDLE.to_string()),
        "handle conflict should carry CONFLICT_ON_HANDLE: {}",
        cause.Error()
    );
    // 断言说明：从这里开始校验上一段场景对外暴露的结果。
    // 这些断言关注的是计数、文本、输出形状或资源清理状态。
    // 它们用来锁住 Rust 端需要与 Go 保持一致的可观测合约。
    // 因此只要这些断言成立，就说明测试地图仍然守住了原始意图。
    // 这也是本次只加注释不改逻辑时最需要被明示的部分。
    assert!(
        cause.Error().contains("[[1], [2]]") || cause.Error().contains("[1]"),
        "handle conflict should carry row ids: {}",
        cause.Error()
    );
    h.Close().unwrap();

    let mut h = errorOnDup::default();
    h.Begin(&example_index_key()).unwrap();
    h.Append(&[11]).unwrap();
    h.Append(&[12]).unwrap();
    let err = h.End().unwrap_err();
    // 断言说明：从这里开始校验上一段场景对外暴露的结果。
    // 这些断言关注的是计数、文本、输出形状或资源清理状态。
    // 它们用来锁住 Rust 端需要与 Go 保持一致的可观测合约。
    // 因此只要这些断言成立，就说明测试地图仍然守住了原始意图。
    // 这也是本次只加注释不改逻辑时最需要被明示的部分。
    assert!(errors::Is(&err, &ErrDuplicateKey()));
    let cause = errors::Cause(&err);
    // 断言说明：从这里开始校验上一段场景对外暴露的结果。
    // 这些断言关注的是计数、文本、输出形状或资源清理状态。
    // 它们用来锁住 Rust 端需要与 Go 保持一致的可观测合约。
    // 因此只要这些断言成立，就说明测试地图仍然守住了原始意图。
    // 这也是本次只加注释不改逻辑时最需要被明示的部分。
    assert!(
        cause.Error().contains(&EXAMPLE_INDEX_ID.to_string()),
        "index conflict should carry index id 23: {}",
        cause.Error()
    );
    // 断言说明：从这里开始校验上一段场景对外暴露的结果。
    // 这些断言关注的是计数、文本、输出形状或资源清理状态。
    // 它们用来锁住 Rust 端需要与 Go 保持一致的可观测合约。
    // 因此只要这些断言成立，就说明测试地图仍然守住了原始意图。
    // 这也是本次只加注释不改逻辑时最需要被明示的部分。
    assert!(
        cause.Error().contains("[[11], [12]]") || cause.Error().contains("[11]"),
        "index conflict should carry row ids: {}",
        cause.Error()
    );
    h.Close().unwrap();
}

// 自动补充的`DupRecord` 是测试文件用来搭建场景的辅助部件。
// 它通常把样例数据、框架对象或重复断言逻辑收敛起来。
// 这样主测例就能更直接地聚焦契约行为而不是铺垫细节。
// 阅读它时可以关注“场景是怎么被搭出来的”。
struct DupRecord {
    key: Vec<u8>,
    row_ids: Vec<Vec<u8>>,
}

// 自动补充的下面的测试围绕 `test_replace_on_dup` 对应的契约或边界展开。
// 它会用构造好的样本输入来锁定与 Go 一致的可观测结果。
// 断言优先锁定输出、计数或错误文本，而不是实现细节。
// 这也是本次只加注释不改逻辑时最需要保护的契约部分。
/// Corresponds to Go `TestReplaceOnDup` / `runDupHandlerTest`.
#[test]
fn test_replace_on_dup() {
    run_dup_handler_test(
        |w| {
            Box::new(replaceOnDup {
                w,
                keyID: Vec::new(),
                idxID: Vec::new(),
            }) as Box<dyn Handler>
        },
        vec![
            DupRecord {
                key: example_handle_key(),
                row_ids: vec![b"01".to_vec(), b"02".to_vec(), b"03".to_vec()],
            },
            DupRecord {
                key: example_index_key(),
                row_ids: vec![b"11".to_vec(), b"12".to_vec(), b"13".to_vec()],
            },
        ],
        HashMap::from([
            (CONFLICT_ON_HANDLE, vec![b"01".to_vec(), b"02".to_vec()]),
            (EXAMPLE_INDEX_ID, vec![b"11".to_vec(), b"12".to_vec()]),
        ]),
    );
}

#[test]
fn test_add_keys_processes_data_chunks_and_skips_index_engine() {
    let mut cfg = config::Config::NewConfig();
    cfg.TikvImporter.Backend = config::BackendLocal.into();
    cfg.App.RegionConcurrency = 2;
    cfg.Conflict.Strategy = config::ErrorOnDup;
    let rc = NewImportController(
        context::Background(),
        &cfg,
        ControllerParam {
            DBMetas: vec![],
            Status: None,
            DumpFileStorage: storeapi::Storage::new("mem://dup-detect"),
            OwnExtStorage: false,
            Pauser: None,
            DB: Some(sql::DB::new_memory()),
            CheckpointStorage: None,
            CheckpointName: String::new(),
            DupIndicator: None,
            KeyspaceName: String::new(),
            ResourceGroupName: String::new(),
            TaskType: String::new(),
        },
    )
    .unwrap();
    let table_info = importdef::TableInfo {
        DB: "db".into(),
        Name: "table".into(),
        Core: model::TableInfo {
            Name: model::CIStr::new("table"),
            ..Default::default()
        },
        ..Default::default()
    };
    let db_info = importdef::DBInfo {
        Name: "db".into(),
        ..Default::default()
    };
    let tr = NewTableImporter(&db_info, &table_info, None, log::Logger::L()).unwrap();
    let missing_chunk = checkpoints::ChunkCheckpoint {
        Key: checkpoints::ChunkCheckpointKey {
            Path: "missing.sql".into(),
            Offset: 0,
        },
        FileMeta: checkpoints::mydump::SourceFileMeta {
            Path: "missing.sql".into(),
            Type: checkpoints::mydump::SourceType(mydump::SourceTypeSQL),
            ..Default::default()
        },
        ..Default::default()
    };
    let mut cp = checkpoints::TableCheckpoint::default();
    cp.Engines.insert(
        -1,
        checkpoints::EngineCheckpoint {
            Chunks: vec![missing_chunk.clone()],
            ..Default::default()
        },
    );
    let detector = dupDetector {
        tr: Arc::new(tr),
        rc: Arc::new(rc),
        cp,
        logger: log::Logger::L(),
    };
    let ignore_rows = Arc::new(extsort::DiskSorter::default());
    detector
        .run(context::Background(), "/tmp", ignore_rows.clone())
        .unwrap();

    let mut cp = detector.cp.clone();
    cp.Engines.insert(
        0,
        checkpoints::EngineCheckpoint {
            Chunks: vec![missing_chunk],
            ..Default::default()
        },
    );
    let detector = dupDetector { cp, ..detector };
    let error = detector
        .run(context::Background(), "/tmp", ignore_rows)
        .unwrap_err();
    assert!(error.Error().contains("missing.sql"), "{}", error.Error());
}

// 自动补充的`run_dup_handler_test` 是测试文件用来搭建场景的辅助部件。
// 它通常把样例数据、框架对象或重复断言逻辑收敛起来。
// 这样主测例就能更直接地聚焦契约行为而不是铺垫细节。
// 阅读它时可以关注“场景是怎么被搭出来的”。
fn run_dup_handler_test<F>(
    make_handler: F,
    input: Vec<DupRecord>,
    ignored_row_ids: HashMap<i64, Vec<Vec<u8>>>,
) where
    F: FnOnce(Box<dyn extsort::Writer>) -> Box<dyn Handler>,
{
    let sorter = extsort::OpenDiskSorter("/tmp", &extsort::DiskSorterOptions::default()).unwrap();
    let ctx = context::Background();
    let writer = sorter.NewWriter(ctx).unwrap();
    let mut handler = make_handler(writer);

    for record in input {
        handler.Begin(&record.key).unwrap();
        for row_id in &record.row_ids {
            handler.Append(row_id).unwrap();
        }
        handler.End().unwrap();
    }
    handler.Close().unwrap();

    // Slim DiskSorter keeps Put records in-memory (no Sort/Iterator).
    let records = sorter.records.lock().unwrap().clone();
    let mut row_ids: HashMap<i64, Vec<Vec<u8>>> = HashMap::new();
    for (key, val) in records {
        let (_, idx_id) = codec::DecodeVarint(&val).unwrap();
        row_ids.entry(idx_id).or_default().push(key);
    }
    // 断言说明：从这里开始校验上一段场景对外暴露的结果。
    // 这些断言关注的是计数、文本、输出形状或资源清理状态。
    // 它们用来锁住 Rust 端需要与 Go 保持一致的可观测合约。
    // 因此只要这些断言成立，就说明测试地图仍然守住了原始意图。
    // 这也是本次只加注释不改逻辑时最需要被明示的部分。
    assert_eq!(row_ids, ignored_row_ids);
}

// 自动补充的`col` 是测试文件用来搭建场景的辅助部件。
// 它通常把样例数据、框架对象或重复断言逻辑收敛起来。
// 这样主测例就能更直接地聚焦契约行为而不是铺垫细节。
// 阅读它时可以关注“场景是怎么被搭出来的”。
fn col(name: &str, offset: i32) -> model::ColumnInfo {
    model::ColumnInfo {
        Name: model::CIStr::new(name),
        Offset: offset,
        ..Default::default()
    }
}

// 自动补充的`idx_col` 是测试文件用来搭建场景的辅助部件。
// 它通常把样例数据、框架对象或重复断言逻辑收敛起来。
// 这样主测例就能更直接地聚焦契约行为而不是铺垫细节。
// 阅读它时可以关注“场景是怎么被搭出来的”。
fn idx_col(name: &str, offset: i32) -> model::IndexColumn {
    model::IndexColumn {
        Name: model::CIStr::new(name),
        Offset: offset,
        Length: -1,
    }
}

// 自动补充的`index_info` 是测试文件用来搭建场景的辅助部件。
// 它通常把样例数据、框架对象或重复断言逻辑收敛起来。
// 这样主测例就能更直接地聚焦契约行为而不是铺垫细节。
// 阅读它时可以关注“场景是怎么被搭出来的”。
fn index_info(
    name: &str,
    unique: bool,
    primary: bool,
    columns: Vec<model::IndexColumn>,
) -> model::IndexInfo {
    model::IndexInfo {
        Name: model::CIStr::new(name),
        Unique: unique,
        Primary: primary,
        Columns: columns,
        ..Default::default()
    }
}

// 自动补充的下面的测试围绕 `test_simplify_table` 对应的契约或边界展开。
// 它会用构造好的样本输入来锁定与 Go 一致的可观测结果。
// 断言优先锁定输出、计数或错误文本，而不是实现细节。
// 这也是本次只加注释不改逻辑时最需要保护的契约部分。
/// Corresponds to Go `TestSimplifyTable` (manual `TableInfo`, no SQL parser).
#[test]
fn test_simplify_table() {
    // Case 1: no PK/unique → empty cols, perm [-1] from [0,1,2,-1].
    {
        let original = model::TableInfo {
            Name: model::CIStr::new("t"),
            Columns: vec![col("a", 0), col("b", 1), col("c", 2)],
            ..Default::default()
        };
        for _ in 0..2 {
            let (actual, actual_perm) = simplifyTable(&original, &[0, 1, 2, -1]);
            // 断言说明：从这里开始校验上一段场景对外暴露的结果。
            // 这些断言关注的是计数、文本、输出形状或资源清理状态。
            // 它们用来锁住 Rust 端需要与 Go 保持一致的可观测合约。
            // 因此只要这些断言成立，就说明测试地图仍然守住了原始意图。
            // 这也是本次只加注释不改逻辑时最需要被明示的部分。
            assert!(actual.Columns.is_empty());
            assert!(actual.Indices.is_empty());
            // 断言说明：从这里开始校验上一段场景对外暴露的结果。
            // 这些断言关注的是计数、文本、输出形状或资源清理状态。
            // 它们用来锁住 Rust 端需要与 Go 保持一致的可观测合约。
            // 因此只要这些断言成立，就说明测试地图仍然守住了原始意图。
            // 这也是本次只加注释不改逻辑时最需要被明示的部分。
            assert_eq!(actual_perm, vec![-1]);
        }
        // 断言说明：从这里开始校验上一段场景对外暴露的结果。
        // 这些断言关注的是计数、文本、输出形状或资源清理状态。
        // 它们用来锁住 Rust 端需要与 Go 保持一致的可观测合约。
        // 因此只要这些断言成立，就说明测试地图仍然守住了原始意图。
        // 这也是本次只加注释不改逻辑时最需要被明示的部分。
        assert_eq!(original.Columns.len(), 3);
    }

    // Case 2: PK on a → keep a, perm [2] from [2,0,1].
    {
        let original = model::TableInfo {
            Name: model::CIStr::new("t"),
            Columns: vec![col("a", 0), col("b", 1), col("c", 2)],
            PKIsHandle: true,
            Indices: vec![index_info("PRIMARY", true, true, vec![idx_col("a", 0)])],
            ..Default::default()
        };
        for _ in 0..2 {
            let (actual, actual_perm) = simplifyTable(&original, &[2, 0, 1]);
            // 断言说明：从这里开始校验上一段场景对外暴露的结果。
            // 这些断言关注的是计数、文本、输出形状或资源清理状态。
            // 它们用来锁住 Rust 端需要与 Go 保持一致的可观测合约。
            // 因此只要这些断言成立，就说明测试地图仍然守住了原始意图。
            // 这也是本次只加注释不改逻辑时最需要被明示的部分。
            assert_eq!(actual.Columns.len(), 1);
            assert_eq!(actual.Columns[0].Name.L, "a");
            // 断言说明：从这里开始校验上一段场景对外暴露的结果。
            // 这些断言关注的是计数、文本、输出形状或资源清理状态。
            // 它们用来锁住 Rust 端需要与 Go 保持一致的可观测合约。
            // 因此只要这些断言成立，就说明测试地图仍然守住了原始意图。
            // 这也是本次只加注释不改逻辑时最需要被明示的部分。
            assert_eq!(actual.Columns[0].Offset, 0);
            assert_eq!(actual.Indices.len(), 1);
            // 断言说明：从这里开始校验上一段场景对外暴露的结果。
            // 这些断言关注的是计数、文本、输出形状或资源清理状态。
            // 它们用来锁住 Rust 端需要与 Go 保持一致的可观测合约。
            // 因此只要这些断言成立，就说明测试地图仍然守住了原始意图。
            // 这也是本次只加注释不改逻辑时最需要被明示的部分。
            assert!(actual.Indices[0].Primary || actual.Indices[0].Unique);
            assert_eq!(actual.Indices[0].Columns[0].Name.L, "a");
            // 断言说明：从这里开始校验上一段场景对外暴露的结果。
            // 这些断言关注的是计数、文本、输出形状或资源清理状态。
            // 它们用来锁住 Rust 端需要与 Go 保持一致的可观测合约。
            // 因此只要这些断言成立，就说明测试地图仍然守住了原始意图。
            // 这也是本次只加注释不改逻辑时最需要被明示的部分。
            assert_eq!(actual_perm, vec![2]);
        }
    }

    // Case 3: unique a + unique bc → keep a,b,c + those indices (drop non-unique idx_b/idx_c).
    {
        let original = model::TableInfo {
            Name: model::CIStr::new("t"),
            Columns: vec![col("a", 0), col("b", 1), col("c", 2), col("d", 3)],
            Indices: vec![
                index_info("a", true, false, vec![idx_col("a", 0)]),
                index_info("idx_b", false, false, vec![idx_col("b", 1)]),
                index_info("idx_c", false, false, vec![idx_col("c", 2)]),
                index_info(
                    "idx_bc",
                    true,
                    false,
                    vec![idx_col("b", 1), idx_col("c", 2)],
                ),
            ],
            ..Default::default()
        };
        for _ in 0..2 {
            let (actual, actual_perm) = simplifyTable(&original, &[0, 1, 2, 3, 10]);
            // 断言说明：从这里开始校验上一段场景对外暴露的结果。
            // 这些断言关注的是计数、文本、输出形状或资源清理状态。
            // 它们用来锁住 Rust 端需要与 Go 保持一致的可观测合约。
            // 因此只要这些断言成立，就说明测试地图仍然守住了原始意图。
            // 这也是本次只加注释不改逻辑时最需要被明示的部分。
            assert_eq!(actual.Columns.len(), 3);
            assert_eq!(actual.Columns[0].Name.L, "a");
            // 断言说明：从这里开始校验上一段场景对外暴露的结果。
            // 这些断言关注的是计数、文本、输出形状或资源清理状态。
            // 它们用来锁住 Rust 端需要与 Go 保持一致的可观测合约。
            // 因此只要这些断言成立，就说明测试地图仍然守住了原始意图。
            // 这也是本次只加注释不改逻辑时最需要被明示的部分。
            assert_eq!(actual.Columns[1].Name.L, "b");
            assert_eq!(actual.Columns[2].Name.L, "c");
            // 断言说明：从这里开始校验上一段场景对外暴露的结果。
            // 这些断言关注的是计数、文本、输出形状或资源清理状态。
            // 它们用来锁住 Rust 端需要与 Go 保持一致的可观测合约。
            // 因此只要这些断言成立，就说明测试地图仍然守住了原始意图。
            // 这也是本次只加注释不改逻辑时最需要被明示的部分。
            assert_eq!(actual.Indices.len(), 2);
            let names: Vec<_> = actual.Indices.iter().map(|i| i.Name.L.clone()).collect();
            // 断言说明：从这里开始校验上一段场景对外暴露的结果。
            // 这些断言关注的是计数、文本、输出形状或资源清理状态。
            // 它们用来锁住 Rust 端需要与 Go 保持一致的可观测合约。
            // 因此只要这些断言成立，就说明测试地图仍然守住了原始意图。
            // 这也是本次只加注释不改逻辑时最需要被明示的部分。
            assert!(names.contains(&"a".to_string()));
            assert!(names.contains(&"idx_bc".to_string()));
            // 断言说明：从这里开始校验上一段场景对外暴露的结果。
            // 这些断言关注的是计数、文本、输出形状或资源清理状态。
            // 它们用来锁住 Rust 端需要与 Go 保持一致的可观测合约。
            // 因此只要这些断言成立，就说明测试地图仍然守住了原始意图。
            // 这也是本次只加注释不改逻辑时最需要被明示的部分。
            assert!(!names.contains(&"idx_b".to_string()));
            assert!(!names.contains(&"idx_c".to_string()));
            // 断言说明：从这里开始校验上一段场景对外暴露的结果。
            // 这些断言关注的是计数、文本、输出形状或资源清理状态。
            // 它们用来锁住 Rust 端需要与 Go 保持一致的可观测合约。
            // 因此只要这些断言成立，就说明测试地图仍然守住了原始意图。
            // 这也是本次只加注释不改逻辑时最需要被明示的部分。
            assert_eq!(actual_perm, vec![0, 1, 2, 10]);
        }
    }

    // Case 4: unique cd only → keep c,d.
    {
        let original = model::TableInfo {
            Name: model::CIStr::new("t"),
            Columns: vec![col("a", 0), col("b", 1), col("c", 2), col("d", 3)],
            Indices: vec![
                index_info("idx_b", false, false, vec![idx_col("b", 1)]),
                index_info("idx_c", false, false, vec![idx_col("c", 2)]),
                index_info(
                    "idx_cd",
                    true,
                    false,
                    vec![idx_col("c", 2), idx_col("d", 3)],
                ),
            ],
            ..Default::default()
        };
        for _ in 0..2 {
            let (actual, actual_perm) = simplifyTable(&original, &[0, 1, 2, 3, 10]);
            // 断言说明：从这里开始校验上一段场景对外暴露的结果。
            // 这些断言关注的是计数、文本、输出形状或资源清理状态。
            // 它们用来锁住 Rust 端需要与 Go 保持一致的可观测合约。
            // 因此只要这些断言成立，就说明测试地图仍然守住了原始意图。
            // 这也是本次只加注释不改逻辑时最需要被明示的部分。
            assert_eq!(actual.Columns.len(), 2);
            assert_eq!(actual.Columns[0].Name.L, "c");
            // 断言说明：从这里开始校验上一段场景对外暴露的结果。
            // 这些断言关注的是计数、文本、输出形状或资源清理状态。
            // 它们用来锁住 Rust 端需要与 Go 保持一致的可观测合约。
            // 因此只要这些断言成立，就说明测试地图仍然守住了原始意图。
            // 这也是本次只加注释不改逻辑时最需要被明示的部分。
            assert_eq!(actual.Columns[1].Name.L, "d");
            assert_eq!(actual.Indices.len(), 1);
            // 断言说明：从这里开始校验上一段场景对外暴露的结果。
            // 这些断言关注的是计数、文本、输出形状或资源清理状态。
            // 它们用来锁住 Rust 端需要与 Go 保持一致的可观测合约。
            // 因此只要这些断言成立，就说明测试地图仍然守住了原始意图。
            // 这也是本次只加注释不改逻辑时最需要被明示的部分。
            assert_eq!(actual.Indices[0].Name.L, "idx_cd");
            assert_eq!(actual.Indices[0].Columns.len(), 2);
            // 断言说明：从这里开始校验上一段场景对外暴露的结果。
            // 这些断言关注的是计数、文本、输出形状或资源清理状态。
            // 它们用来锁住 Rust 端需要与 Go 保持一致的可观测合约。
            // 因此只要这些断言成立，就说明测试地图仍然守住了原始意图。
            // 这也是本次只加注释不改逻辑时最需要被明示的部分。
            assert_eq!(actual.Indices[0].Columns[0].Name.L, "c");
            assert_eq!(actual.Indices[0].Columns[1].Name.L, "d");
            // 断言说明：从这里开始校验上一段场景对外暴露的结果。
            // 这些断言关注的是计数、文本、输出形状或资源清理状态。
            // 它们用来锁住 Rust 端需要与 Go 保持一致的可观测合约。
            // 因此只要这些断言成立，就说明测试地图仍然守住了原始意图。
            // 这也是本次只加注释不改逻辑时最需要被明示的部分。
            assert_eq!(actual_perm, vec![2, 3, 10]);
        }
    }
}
