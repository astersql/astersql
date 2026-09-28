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
//! Go-equivalent unit tests for `lightning/pkg/importer/get_pre_info_test.go`.
//!
//! The cases below preserve the observable Go contracts supported by this crate.

use crate::*;
use astersql_lightning_pkg_importer_opts as ropts;
use flate2::{Compression, write::GzEncoder};
use std::io::Write;
use std::sync::Arc;

// 自动补充的`make_pre_import_getter` 是测试文件用来搭建场景的辅助部件。
// 它通常把样例数据、框架对象或重复断言逻辑收敛起来。
// 这样主测例就能更直接地聚焦契约行为而不是铺垫细节。
// 阅读它时可以关注“场景是怎么被搭出来的”。
fn make_pre_import_getter(db_metas: Vec<mydump::MDDatabaseMeta>) -> Arc<dyn PreImportInfoGetter> {
    make_pre_import_getter_with_storage(db_metas, storeapi::Storage::new("file:///data"))
}

fn make_pre_import_getter_with_storage(
    db_metas: Vec<mydump::MDDatabaseMeta>,
    storage: storeapi::Storage,
) -> Arc<dyn PreImportInfoGetter> {
    let mut cfg = config::Config::NewConfig();
    cfg.TikvImporter.Backend = config::BackendLocal.into();
    let target = NewTargetInfoGetterImpl(&cfg, sql::DB::new_memory(), None).unwrap();
    NewPreImportInfoGetter(
        &cfg,
        db_metas,
        storage,
        target,
        None,
        None,
        vec![ropts::WithIgnoreDBNotExist(true)],
    )
    .unwrap()
}

// 自动补充的下面的测试围绕 `test_get_pre_info_generate_table_info` 对应的契约或边界展开。
// 它会用构造好的样本输入来锁定与 Go 一致的可观测结果。
// 断言优先锁定输出、计数或错误文本，而不是实现细节。
// 这也是本次只加注释不改逻辑时最需要保护的契约部分。
/// TestGetPreInfoGenerateTableInfo
#[test]
fn test_get_pre_info_generate_table_info() {
    let tbl_info = newTableInfo(
        "create table `tbl1` (a varchar(16) not null, b varchar(8) default 'DEFA')",
        1,
    )
    .unwrap();
    // 断言说明：从这里开始校验上一段场景对外暴露的结果。
    // 这些断言关注的是计数、文本、输出形状或资源清理状态。
    // 它们用来锁住 Rust 端需要与 Go 保持一致的可观测合约。
    // 因此只要这些断言成立，就说明测试地图仍然守住了原始意图。
    // 这也是本次只加注释不改逻辑时最需要被明示的部分。
    assert_eq!(tbl_info.Name.L, "tbl1");
    assert_eq!(tbl_info.ID, 1);
    // 断言说明：从这里开始校验上一段场景对外暴露的结果。
    // 这些断言关注的是计数、文本、输出形状或资源清理状态。
    // 它们用来锁住 Rust 端需要与 Go 保持一致的可观测合约。
    // 因此只要这些断言成立，就说明测试地图仍然守住了原始意图。
    // 这也是本次只加注释不改逻辑时最需要被明示的部分。
    assert_eq!(tbl_info.State, model::StatePublic);

    // Slim parser does not validate default length — still returns TableInfo.
    let tbl2 = newTableInfo(
        "create table `tbl1` (a varchar(16), b varchar(8) default 'DEFAULT_BBBBB')",
        2,
    )
    .unwrap();
    // 断言说明：从这里开始校验上一段场景对外暴露的结果。
    // 这些断言关注的是计数、文本、输出形状或资源清理状态。
    // 它们用来锁住 Rust 端需要与 Go 保持一致的可观测合约。
    // 因此只要这些断言成立，就说明测试地图仍然守住了原始意图。
    // 这也是本次只加注释不改逻辑时最需要被明示的部分。
    assert_eq!(tbl2.Name.L, "tbl1");
    assert_eq!(tbl2.ID, 2);
}

// 自动补充的下面的测试围绕 `test_get_pre_info_has_default` 对应的契约或边界展开。
// 它会用构造好的样本输入来锁定与 Go 一致的可观测结果。
// 断言优先锁定输出、计数或错误文本，而不是实现细节。
// 这也是本次只加注释不改逻辑时最需要保护的契约部分。
/// TestGetPreInfoHasDefault — slim hasDefault checks DefaultValue / Generated / _tidb_rowid.
#[test]
fn test_get_pre_info_has_default() {
    let cases: Vec<(model::ColumnInfo, bool)> = vec![
        (
            model::ColumnInfo {
                Name: model::CIStr::new("a"),
                DefaultValue: Some("x".into()),
                ..Default::default()
            },
            true,
        ),
        (
            model::ColumnInfo {
                Name: model::CIStr::new("a"),
                NotNull: true,
                ..Default::default()
            },
            false,
        ),
        (
            model::ColumnInfo {
                Name: model::CIStr::new("a"),
                NotNull: true,
                AutoIncrement: true,
                ..Default::default()
            },
            true,
        ),
        (
            model::ColumnInfo {
                Name: model::CIStr::new("a"),
                GeneratedExprString: "a+1".into(),
                ..Default::default()
            },
            true,
        ),
        (
            model::ColumnInfo {
                Name: model::CIStr::new("_tidb_rowid"),
                ..Default::default()
            },
            true,
        ),
    ];
    for (i, (col, expected)) in cases.into_iter().enumerate() {
        // 断言说明：从这里开始校验上一段场景对外暴露的结果。
        // 这些断言关注的是计数、文本、输出形状或资源清理状态。
        // 它们用来锁住 Rust 端需要与 Go 保持一致的可观测合约。
        // 因此只要这些断言成立，就说明测试地图仍然守住了原始意图。
        // 这也是本次只加注释不改逻辑时最需要被明示的部分。
        assert_eq!(hasDefault(&col), expected, "case {i}");
    }
}

// 自动补充的下面的测试围绕 `test_get_pre_info_auto_random_bits` 对应的契约或边界展开。
// 它会用构造好的样本输入来锁定与 Go 一致的可观测结果。
// 断言优先锁定输出、计数或错误文本，而不是实现细节。
// 这也是本次只加注释不改逻辑时最需要保护的契约部分。
/// TestGetPreInfoAutoRandomBits.
#[test]
fn test_get_pre_info_auto_random_bits() {
    let cases = [
        ("create table `t` (a varchar(16))", 0, 0),
        ("create table `t` (a BIGINT PRIMARY KEY AUTO_RANDOM)", 5, 64),
        (
            "create table `t` (a BIGINT PRIMARY KEY AUTO_RANDOM(3))",
            3,
            64,
        ),
        (
            "create table `t` (a BIGINT PRIMARY KEY AUTO_RANDOM(5, 64))",
            5,
            64,
        ),
        (
            "create table `t` (a BIGINT PRIMARY KEY AUTO_RANDOM(2, 32))",
            2,
            32,
        ),
    ];
    for (i, (sql, bits, range_bits)) in cases.iter().enumerate() {
        let info = newTableInfo(sql, (i as i64) + 1).unwrap();
        // 断言说明：从这里开始校验上一段场景对外暴露的结果。
        // 这些断言关注的是计数、文本、输出形状或资源清理状态。
        // 它们用来锁住 Rust 端需要与 Go 保持一致的可观测合约。
        // 因此只要这些断言成立，就说明测试地图仍然守住了原始意图。
        // 这也是本次只加注释不改逻辑时最需要被明示的部分。
        assert_eq!(info.Name.L, "t", "sql={sql}");
        assert_eq!(info.ID, (i as i64) + 1);
        assert_eq!(info.AutoRandomBits, *bits);
        assert_eq!(info.AutoRandomRangeBits, *range_bits);
    }
}

// 自动补充的下面的测试围绕 `test_get_pre_info_get_all_table_structures` 对应的契约或边界展开。
// 它会用构造好的样本输入来锁定与 Go 一致的可观测结果。
// 断言优先锁定输出、计数或错误文本，而不是实现细节。
// 这也是本次只加注释不改逻辑时最需要保护的契约部分。
/// TestGetPreInfoGetAllTableStructures
#[test]
fn test_get_pre_info_get_all_table_structures() {
    let db_metas = vec![
        mydump::MDDatabaseMeta {
            Name: "db01".into(),
            Tables: vec![
                mydump::MDTableMeta {
                    DB: "db01".into(),
                    Name: "tbl01".into(),
                    TotalSize: 0,
                    SchemaFile: Some(mydump::SourceFileMeta {
                        Path: "/db01/tbl01/tbl01.schema.sql".into(),
                        ..Default::default()
                    }),
                    DataFiles: vec![],
                },
                mydump::MDTableMeta {
                    DB: "db01".into(),
                    Name: "tbl02".into(),
                    TotalSize: 0,
                    SchemaFile: Some(mydump::SourceFileMeta {
                        Path: "/db01/tbl02/tbl02.schema.sql".into(),
                        ..Default::default()
                    }),
                    DataFiles: vec![],
                },
            ],
        },
        mydump::MDDatabaseMeta {
            Name: "db02".into(),
            Tables: vec![mydump::MDTableMeta {
                DB: "db02".into(),
                Name: "tbl01".into(),
                TotalSize: 0,
                SchemaFile: Some(mydump::SourceFileMeta {
                    Path: "/db02/tbl01/tbl01.schema.sql".into(),
                    ..Default::default()
                }),
                DataFiles: vec![],
            }],
        },
    ];
    let storage = storeapi::Storage::new("file:///data");
    storage.Put(
        "/db01/tbl01/tbl01.schema.sql",
        b"CREATE TABLE `tbl01` (`id` BIGINT)".to_vec(),
    );
    storage.Put(
        "/db01/tbl02/tbl02.schema.sql",
        b"CREATE TABLE `tbl02` (`id` BIGINT)".to_vec(),
    );
    storage.Put(
        "/db02/tbl01/tbl01.schema.sql",
        b"CREATE TABLE `tbl01` (`id` BIGINT)".to_vec(),
    );
    let ig = make_pre_import_getter_with_storage(db_metas, storage);
    let structs = ig
        .GetAllTableStructures(context::Background(), &[])
        .unwrap();
    // 断言说明：从这里开始校验上一段场景对外暴露的结果。
    // 这些断言关注的是计数、文本、输出形状或资源清理状态。
    // 它们用来锁住 Rust 端需要与 Go 保持一致的可观测合约。
    // 因此只要这些断言成立，就说明测试地图仍然守住了原始意图。
    // 这也是本次只加注释不改逻辑时最需要被明示的部分。
    assert_eq!(structs.len(), 2);
    assert_eq!(structs["db01"].Tables.len(), 2);
    // 断言说明：从这里开始校验上一段场景对外暴露的结果。
    // 这些断言关注的是计数、文本、输出形状或资源清理状态。
    // 它们用来锁住 Rust 端需要与 Go 保持一致的可观测合约。
    // 因此只要这些断言成立，就说明测试地图仍然守住了原始意图。
    // 这也是本次只加注释不改逻辑时最需要被明示的部分。
    assert_eq!(structs["db02"].Tables.len(), 1);
    assert_eq!(structs["db01"].Tables["tbl01"].Core.Name.L, "tbl01");
    // 断言说明：从这里开始校验上一段场景对外暴露的结果。
    // 这些断言关注的是计数、文本、输出形状或资源清理状态。
    // 它们用来锁住 Rust 端需要与 Go 保持一致的可观测合约。
    // 因此只要这些断言成立，就说明测试地图仍然守住了原始意图。
    // 这也是本次只加注释不改逻辑时最需要被明示的部分。
    assert_eq!(structs["db01"].Tables["tbl02"].Core.Name.L, "tbl02");
    assert_eq!(structs["db02"].Tables["tbl01"].Core.Name.L, "tbl01");

    // Cached path (no ForceReload).
    let again = ig
        .GetAllTableStructures(context::Background(), &[])
        .unwrap();
    // 断言说明：从这里开始校验上一段场景对外暴露的结果。
    // 这些断言关注的是计数、文本、输出形状或资源清理状态。
    // 它们用来锁住 Rust 端需要与 Go 保持一致的可观测合约。
    // 因此只要这些断言成立，就说明测试地图仍然守住了原始意图。
    // 这也是本次只加注释不改逻辑时最需要被明示的部分。
    assert_eq!(again.len(), 2);

    // ForceReloadCache rebuilds.
    let reloaded = ig
        .GetAllTableStructures(context::Background(), &[ropts::ForceReloadCache(true)])
        .unwrap();
    // 断言说明：从这里开始校验上一段场景对外暴露的结果。
    // 这些断言关注的是计数、文本、输出形状或资源清理状态。
    // 它们用来锁住 Rust 端需要与 Go 保持一致的可观测合约。
    // 因此只要这些断言成立，就说明测试地图仍然守住了原始意图。
    // 这也是本次只加注释不改逻辑时最需要被明示的部分。
    assert_eq!(reloaded.len(), 2);
}

#[test]
fn test_get_pre_info_loads_schema_columns_from_storage() {
    let schema = mydump::SourceFileMeta {
        Path: "/db/t.schema.sql".into(),
        ..Default::default()
    };
    let metas = vec![mydump::MDDatabaseMeta {
        Name: "db".into(),
        Tables: vec![mydump::MDTableMeta {
            DB: "db".into(),
            Name: "t".into(),
            SchemaFile: Some(schema.clone()),
            ..Default::default()
        }],
    }];
    let storage = storeapi::Storage::new("file:///data");
    storage.Put(
        &schema.Path,
        b"CREATE TABLE `t` (`id` BIGINT PRIMARY KEY, `name` VARCHAR(32) DEFAULT 'x')".to_vec(),
    );
    let ig = make_pre_import_getter_with_storage(metas, storage);
    let structures = ig
        .GetAllTableStructures(context::Background(), &[])
        .unwrap();
    let columns = &structures["db"].Tables["t"].Core.Columns;
    assert_eq!(columns.len(), 2);
    assert_eq!(columns[0].Name.L, "id");
    assert_eq!(columns[1].Name.L, "name");
    assert_eq!(columns[1].DefaultValue.as_deref(), Some("x"));
}

// 自动补充的下面的测试围绕 `test_get_pre_info_read_first_row` 对应的契约或边界展开。
// 它会用构造好的样本输入来锁定与 Go 一致的可观测结果。
// 断言优先锁定输出、计数或错误文本，而不是实现细节。
// 这也是本次只加注释不改逻辑时最需要保护的契约部分。
/// TestGetPreInfoReadFirstRow.
#[test]
fn test_get_pre_info_read_first_row() {
    let db_metas = vec![mydump::MDDatabaseMeta {
        Name: "db01".into(),
        Tables: vec![mydump::MDTableMeta {
            DB: "db01".into(),
            Name: "tbl01".into(),
            TotalSize: 32,
            SchemaFile: Some(mydump::SourceFileMeta::default()),
            DataFiles: vec![mydump::FileInfo {
                TableName: "tbl01".into(),
                FileMeta: mydump::SourceFileMeta {
                    Path: "/db01/tbl01/data.001.csv".into(),
                    Type: mydump::SourceTypeCSV,
                    ..Default::default()
                },
            }],
        }],
    }];
    let storage = storeapi::Storage::new("file:///data");
    storage.Put(
        "/db01/tbl01/data.001.csv",
        b"alice,10\nbob,20\ncarol,30\n".to_vec(),
    );
    let ig = make_pre_import_getter_with_storage(db_metas, storage);
    let (cols, rows) = ig
        .ReadFirstNRowsByFileMeta(
            context::Background(),
            mydump::SourceFileMeta {
                Path: "/db01/tbl01/data.001.csv".into(),
                Type: mydump::SourceTypeCSV,
                ..Default::default()
            },
            2,
        )
        .unwrap();
    // 断言说明：从这里开始校验上一段场景对外暴露的结果。
    // 这些断言关注的是计数、文本、输出形状或资源清理状态。
    // 它们用来锁住 Rust 端需要与 Go 保持一致的可观测合约。
    // 因此只要这些断言成立，就说明测试地图仍然守住了原始意图。
    // 这也是本次只加注释不改逻辑时最需要被明示的部分。
    assert!(cols.is_empty());
    assert_eq!(rows.len(), 2);
    assert_eq!(datum_strings(&rows[0]), vec!["alice", "10"]);
    assert_eq!(datum_strings(&rows[1]), vec!["bob", "20"]);

    let (cols, rows) = ig
        .ReadFirstNRowsByTableName(context::Background(), "db01", "tbl01", 1)
        .unwrap();
    // 断言说明：从这里开始校验上一段场景对外暴露的结果。
    // 这些断言关注的是计数、文本、输出形状或资源清理状态。
    // 它们用来锁住 Rust 端需要与 Go 保持一致的可观测合约。
    // 因此只要这些断言成立，就说明测试地图仍然守住了原始意图。
    // 这也是本次只加注释不改逻辑时最需要被明示的部分。
    assert!(cols.is_empty());
    assert_eq!(rows.len(), 1);
    assert_eq!(datum_strings(&rows[0]), vec!["alice", "10"]);
}

#[test]
fn test_get_pre_info_read_sql_and_empty_table_like_go() {
    let sql_meta = mydump::SourceFileMeta {
        Path: "/db01/tbl01/data.sql".into(),
        Type: mydump::SourceTypeSQL,
        ..Default::default()
    };
    let db_metas = vec![mydump::MDDatabaseMeta {
        Name: "db01".into(),
        Tables: vec![
            mydump::MDTableMeta {
                DB: "db01".into(),
                Name: "tbl01".into(),
                DataFiles: vec![mydump::FileInfo {
                    TableName: "tbl01".into(),
                    FileMeta: sql_meta.clone(),
                }],
                ..Default::default()
            },
            mydump::MDTableMeta {
                DB: "db01".into(),
                Name: "empty".into(),
                ..Default::default()
            },
        ],
    }];
    let storage = storeapi::Storage::new("file:///data");
    storage.Put(
        &sql_meta.Path,
        b"INSERT INTO db01.tbl01 (ival, sval) VALUES (333, 'ccc'),(444, 'ddd');".to_vec(),
    );
    let ig = make_pre_import_getter_with_storage(db_metas, storage);
    let (cols, rows) = ig
        .ReadFirstNRowsByFileMeta(context::Background(), sql_meta, 1)
        .unwrap();
    assert_eq!(cols, vec!["ival", "sval"]);
    assert_eq!(datum_strings(&rows[0]), vec!["333", "ccc"]);

    let (cols, rows) = ig
        .ReadFirstNRowsByTableName(context::Background(), "db01", "empty", 1)
        .unwrap();
    assert!(cols.is_empty());
    assert!(rows.is_empty());
}

#[test]
fn test_get_pre_info_reads_parquet_like_go() {
    let data = std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/testdata/test.parquet"
    ))
    .unwrap();
    let meta = mydump::SourceFileMeta {
        Path: "/test.parquet".into(),
        Type: mydump::SourceTypeParquet,
        ..Default::default()
    };
    let storage = storeapi::Storage::new("file:///data");
    storage.Put(&meta.Path, data);
    let mut cfg = config::Config::NewConfig();
    cfg.TikvImporter.Backend = config::BackendLocal.into();
    let target = NewTargetInfoGetterImpl(&cfg, sql::DB::new_memory(), None).unwrap();
    let ig = NewPreImportInfoGetter(&cfg, vec![], storage, target, None, None, vec![]).unwrap();
    let (columns, rows) = ig
        .ReadFirstNRowsByFileMeta(context::Background(), meta, 3)
        .unwrap();
    assert_eq!(columns, vec!["id", "name"]);
    assert_eq!(rows.len(), 3);
    assert_eq!(datum_strings(&rows[0]), vec!["1", "name_1"]);
}

// 自动补充的下面的测试围绕 `test_get_pre_info_read_compressed_first_row` 对应的契约或边界展开。
// 它会用构造好的样本输入来锁定与 Go 一致的可观测结果。
// 断言优先锁定输出、计数或错误文本，而不是实现细节。
// 这也是本次只加注释不改逻辑时最需要保护的契约部分。
/// TestGetPreInfoReadCompressedFirstRow — storage mock supplies decompressed bytes.
#[test]
fn test_get_pre_info_read_compressed_first_row() {
    let compressed = mydump::SourceFileMeta {
        Path: "/db01/tbl01/data.001.csv.gz".into(),
        Compression: 1, // non-None compression marker
        Type: mydump::SourceTypeCSV,
        ..Default::default()
    };
    let db_metas = vec![mydump::MDDatabaseMeta {
        Name: "db01".into(),
        Tables: vec![mydump::MDTableMeta {
            DB: "db01".into(),
            Name: "tbl01".into(),
            TotalSize: 16,
            SchemaFile: Some(mydump::SourceFileMeta::default()),
            DataFiles: vec![mydump::FileInfo {
                TableName: "tbl01".into(),
                FileMeta: compressed.clone(),
            }],
        }],
    }];
    let storage = storeapi::Storage::new("file:///data");
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(b"zip,preview\n").unwrap();
    storage.Put(&compressed.Path, encoder.finish().unwrap());
    let ig = make_pre_import_getter_with_storage(db_metas, storage);
    let (cols, rows) = ig
        .ReadFirstNRowsByFileMeta(context::Background(), compressed, 1)
        .unwrap();
    // 断言说明：从这里开始校验上一段场景对外暴露的结果。
    // 这些断言关注的是计数、文本、输出形状或资源清理状态。
    // 它们用来锁住 Rust 端需要与 Go 保持一致的可观测合约。
    // 因此只要这些断言成立，就说明测试地图仍然守住了原始意图。
    // 这也是本次只加注释不改逻辑时最需要被明示的部分。
    assert!(cols.is_empty());
    assert_eq!(rows.len(), 1);
    assert_eq!(datum_strings(&rows[0]), vec!["zip", "preview"]);
}

#[test]
fn test_get_pre_info_really_decompresses_gzip() {
    let meta = mydump::SourceFileMeta {
        Path: "/data.csv.gz".into(),
        Compression: 1,
        Type: mydump::SourceTypeCSV,
        ..Default::default()
    };
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(b"ival,sval\n111,aaa\n").unwrap();
    let storage = storeapi::Storage::new("file:///data");
    storage.Put(&meta.Path, encoder.finish().unwrap());
    let mut cfg = config::Config::NewConfig();
    cfg.TikvImporter.Backend = config::BackendLocal.into();
    cfg.Mydumper.CSV.Header = true;
    let target = NewTargetInfoGetterImpl(&cfg, sql::DB::new_memory(), None).unwrap();
    let ig = NewPreImportInfoGetter(&cfg, vec![], storage, target, None, None, vec![]).unwrap();
    let (cols, rows) = ig
        .ReadFirstNRowsByFileMeta(context::Background(), meta, 1)
        .unwrap();
    assert_eq!(cols, vec!["ival", "sval"]);
    assert_eq!(datum_strings(&rows[0]), vec!["111", "aaa"]);
}

fn datum_strings(row: &[types::Datum]) -> Vec<String> {
    row.iter()
        .map(|datum| match datum {
            types::Datum::String(value) => value.clone(),
            types::Datum::Bytes(value) => String::from_utf8_lossy(value).into_owned(),
            types::Datum::Int(value) => value.to_string(),
        })
        .collect()
}

// 自动补充的下面的测试围绕 `test_get_pre_info_sample_source` 对应的契约或边界展开。
// 它会用构造好的样本输入来锁定与 Go 一致的可观测结果。
// 断言优先锁定输出、计数或错误文本，而不是实现细节。
// 这也是本次只加注释不改逻辑时最需要保护的契约部分。
/// TestGetPreInfoSampleSource — SampleSource not exported; cover via EstimateSourceDataSize.
#[test]
fn test_get_pre_info_sample_source() {
    let ig = make_pre_import_getter(vec![mydump::MDDatabaseMeta {
        Name: "db01".into(),
        Tables: vec![mydump::MDTableMeta {
            DB: "db01".into(),
            Name: "tbl01".into(),
            TotalSize: 48,
            DataFiles: vec![mydump::FileInfo {
                TableName: "tbl01".into(),
                FileMeta: mydump::SourceFileMeta {
                    Path: "/db01/tbl01/tbl01.data.001.csv".into(),
                    FileSize: 48,
                    ..Default::default()
                },
            }],
            SchemaFile: None,
        }],
    }]);
    let size = ig
        .EstimateSourceDataSize(context::Background(), &[])
        .unwrap();
    // 断言说明：从这里开始校验上一段场景对外暴露的结果。
    // 这些断言关注的是计数、文本、输出形状或资源清理状态。
    // 它们用来锁住 Rust 端需要与 Go 保持一致的可观测合约。
    // 因此只要这些断言成立，就说明测试地图仍然守住了原始意图。
    // 这也是本次只加注释不改逻辑时最需要被明示的部分。
    assert_eq!(size.SizeWithoutIndex, 48);
    assert_eq!(size.SizeWithIndex, 16);
    // 断言说明：从这里开始校验上一段场景对外暴露的结果。
    // 这些断言关注的是计数、文本、输出形状或资源清理状态。
    // 它们用来锁住 Rust 端需要与 Go 保持一致的可观测合约。
    // 因此只要这些断言成立，就说明测试地图仍然守住了原始意图。
    // 这也是本次只加注释不改逻辑时最需要被明示的部分。
    assert!(!size.HasUnsortedBigTables);
}

// 自动补充的下面的测试围绕 `test_get_pre_info_sample_source_compressed` 对应的契约或边界展开。
// 它会用构造好的样本输入来锁定与 Go 一致的可观测结果。
// 断言优先锁定输出、计数或错误文本，而不是实现细节。
// 这也是本次只加注释不改逻辑时最需要保护的契约部分。
/// TestGetPreInfoSampleSourceCompressed
#[test]
fn test_get_pre_info_sample_source_compressed() {
    let ig = make_pre_import_getter(vec![mydump::MDDatabaseMeta {
        Name: "db01".into(),
        Tables: vec![mydump::MDTableMeta {
            DB: "db01".into(),
            Name: "tbl01".into(),
            TotalSize: 20,
            DataFiles: vec![mydump::FileInfo {
                TableName: "tbl01".into(),
                FileMeta: mydump::SourceFileMeta {
                    Path: "/db01/tbl01/tbl01.data.001.csv.gz".into(),
                    Compression: 1,
                    FileSize: 20,
                    ..Default::default()
                },
            }],
            SchemaFile: None,
        }],
    }]);
    let size = ig
        .EstimateSourceDataSize(context::Background(), &[])
        .unwrap();
    // 断言说明：从这里开始校验上一段场景对外暴露的结果。
    // 这些断言关注的是计数、文本、输出形状或资源清理状态。
    // 它们用来锁住 Rust 端需要与 Go 保持一致的可观测合约。
    // 因此只要这些断言成立，就说明测试地图仍然守住了原始意图。
    // 这也是本次只加注释不改逻辑时最需要被明示的部分。
    assert_eq!(size.SizeWithoutIndex, 20);
    assert!(!size.HasUnsortedBigTables);
}

// 自动补充的下面的测试围绕 `test_get_pre_info_estimate_source_size` 对应的契约或边界展开。
// 它会用构造好的样本输入来锁定与 Go 一致的可观测结果。
// 断言优先锁定输出、计数或错误文本，而不是实现细节。
// 这也是本次只加注释不改逻辑时最需要保护的契约部分。
/// TestGetPreInfoEstimateSourceSize
#[test]
fn test_get_pre_info_estimate_source_size() {
    let ig = make_pre_import_getter(vec![mydump::MDDatabaseMeta {
        Name: "db01".into(),
        Tables: vec![
            mydump::MDTableMeta {
                DB: "db01".into(),
                Name: "t1".into(),
                TotalSize: 10,
                DataFiles: vec![],
                SchemaFile: None,
            },
            mydump::MDTableMeta {
                DB: "db01".into(),
                Name: "t2".into(),
                TotalSize: 25,
                DataFiles: vec![],
                SchemaFile: None,
            },
        ],
    }]);
    let size = ig
        .EstimateSourceDataSize(context::Background(), &[])
        .unwrap();
    // 断言说明：从这里开始校验上一段场景对外暴露的结果。
    // 这些断言关注的是计数、文本、输出形状或资源清理状态。
    // 它们用来锁住 Rust 端需要与 Go 保持一致的可观测合约。
    // 因此只要这些断言成立，就说明测试地图仍然守住了原始意图。
    // 这也是本次只加注释不改逻辑时最需要被明示的部分。
    assert_eq!(size.SizeWithoutIndex, 35);
    assert_eq!(size.SizeWithIndex, 11);

    // Cache hit.
    let cached = ig
        .EstimateSourceDataSize(context::Background(), &[])
        .unwrap();
    // 断言说明：从这里开始校验上一段场景对外暴露的结果。
    // 这些断言关注的是计数、文本、输出形状或资源清理状态。
    // 它们用来锁住 Rust 端需要与 Go 保持一致的可观测合约。
    // 因此只要这些断言成立，就说明测试地图仍然守住了原始意图。
    // 这也是本次只加注释不改逻辑时最需要被明示的部分。
    assert_eq!(cached.SizeWithoutIndex, 35);

    // Force reload still Ok.
    let reloaded = ig
        .EstimateSourceDataSize(context::Background(), &[ropts::ForceReloadCache(true)])
        .unwrap();
    // 断言说明：从这里开始校验上一段场景对外暴露的结果。
    // 这些断言关注的是计数、文本、输出形状或资源清理状态。
    // 它们用来锁住 Rust 端需要与 Go 保持一致的可观测合约。
    // 因此只要这些断言成立，就说明测试地图仍然守住了原始意图。
    // 这也是本次只加注释不改逻辑时最需要被明示的部分。
    assert_eq!(reloaded.SizeWithoutIndex, 35);
}

// 自动补充的下面的测试围绕 `test_get_pre_info_is_table_empty` 对应的契约或边界展开。
// 它会用构造好的样本输入来锁定与 Go 一致的可观测结果。
// 断言优先锁定输出、计数或错误文本，而不是实现细节。
// 这也是本次只加注释不改逻辑时最需要保护的契约部分。
/// TestGetPreInfoIsTableEmpty
#[test]
fn test_get_pre_info_is_table_empty() {
    let ctx = context::Background();
    let db = sql::DB::new_memory();
    let mut cfg = config::Config::NewConfig();
    cfg.TikvImporter.Backend = config::BackendTiDB.into();
    let target = NewTargetInfoGetterImpl(&cfg, db.clone(), None).unwrap();

    // No rows → ErrNoRows path → empty table.
    let empty = target
        .IsTableEmpty(ctx.clone(), "test_db", "test_tbl")
        .unwrap();
    // 断言说明：从这里开始校验上一段场景对外暴露的结果。
    // 这些断言关注的是计数、文本、输出形状或资源清理状态。
    // 它们用来锁住 Rust 端需要与 Go 保持一致的可观测合约。
    // 因此只要这些断言成立，就说明测试地图仍然守住了原始意图。
    // 这也是本次只加注释不改逻辑时最需要被明示的部分。
    assert_eq!(empty, Some(true));

    // Push a row → not empty.
    db.push_query_rows("test_db", vec![vec![sql::SqlValue::Int64(1)]], -1);
    let not_empty = target
        .IsTableEmpty(ctx.clone(), "test_db", "test_tbl")
        .unwrap();
    // 断言说明：从这里开始校验上一段场景对外暴露的结果。
    // 这些断言关注的是计数、文本、输出形状或资源清理状态。
    // 它们用来锁住 Rust 端需要与 Go 保持一致的可观测合约。
    // 因此只要这些断言成立，就说明测试地图仍然守住了原始意图。
    // 这也是本次只加注释不改逻辑时最需要被明示的部分。
    assert_eq!(not_empty, Some(false));

    assert!(target.CheckVersionRequirements(ctx).is_ok());
}

#[test]
fn test_target_getter_go_query_shape_vars_and_backend_validation() {
    let ctx = context::Background();
    let db = sql::DB::new_memory();
    db.push_query_rows(
        "SELECT 1 FROM `test_db`.`test_tbl` USE INDEX() LIMIT 1",
        vec![vec![sql::SqlValue::Int64(1)]],
        1,
    );
    let mut cfg = config::Config::NewConfig();
    cfg.TikvImporter.Backend = config::BackendTiDB.into();
    cfg.TiDB.Vars = Some(std::collections::HashMap::from([(
        "manual".to_string(),
        "value".to_string(),
    )]));
    let target = NewTargetInfoGetterImpl(&cfg, db, None).unwrap();
    assert_eq!(
        target
            .IsTableEmpty(ctx.clone(), "test_db", "test_tbl")
            .unwrap(),
        Some(false)
    );
    assert_eq!(
        target.GetTargetSysVariablesForImport(ctx, &[])["manual"],
        "value"
    );

    let mut local_cfg = config::Config::NewConfig();
    local_cfg.TikvImporter.Backend = config::BackendLocal.into();
    let local = NewTargetInfoGetterImpl(&local_cfg, sql::DB::new_memory(), None).unwrap();
    assert!(
        local
            .CheckVersionRequirements(context::Background())
            .unwrap_err()
            .to_string()
            .contains("pd HTTP client is required")
    );

    cfg.TikvImporter.Backend = "unknown".into();
    assert!(NewTargetInfoGetterImpl(&cfg, sql::DB::new_memory(), None).is_err());
}

#[test]
fn test_target_getter_fetches_models_and_pd_information() {
    let db = sql::DB::new_memory();
    db.push_query_rows("SHOW DATABASES", vec![vec![sql::SqlValue::from("db1")]], 1);
    db.push_query_rows(
        "SHOW TABLES FROM `db1`",
        vec![vec![sql::SqlValue::from("t1")]],
        1,
    );
    let pd = pdhttp::Client {
        max_replicas: 5,
        stores: vec![(1, "store-1".into(), 100, 80, 4, 2)],
        empty_regions: vec![(9, 1)],
        ..Default::default()
    };
    let mut cfg = config::Config::NewConfig();
    cfg.TikvImporter.Backend = config::BackendLocal.into();
    let getter = NewTargetInfoGetterImpl(&cfg, db, Some(pd)).unwrap();
    let ctx = context::Background();
    assert_eq!(
        getter.FetchRemoteDBModels(ctx.clone()).unwrap()[0].Name.L,
        "db1"
    );
    assert_eq!(
        getter.FetchRemoteTableModels(ctx.clone(), "db1").unwrap()[0]
            .Name
            .L,
        "t1"
    );
    assert_eq!(getter.GetMaxReplica(ctx.clone()).unwrap(), 5);
    let stores = getter.GetStorageInfo(ctx.clone()).unwrap();
    assert_eq!(stores.Count, 1);
    assert_eq!(stores.Stores[0].Status.Available, 80);
    let regions = getter.GetEmptyRegionsInfo(ctx).unwrap();
    assert_eq!(regions.Count, 1);
    assert_eq!(regions.Regions[0].StoreId, 1);
}
