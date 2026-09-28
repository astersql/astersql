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

//! Go-equivalent tests for `lightning/pkg/importer/mock/mock_test.go`.
//!
//! Mapping:
//! - `TestMockImportSourceBasic` → `test_mock_import_source_basic`
//! - `TestMockTargetInfoBasic` → `test_mock_target_info_basic`
//!
//! Mock/real boundary: exercises real in-memory `MemStorage` WriteFile/ReadFile
//! and pure TargetInfo algorithms; no PD/TiKV/DB/network (same as Go package mock).
//!
//! 因而本文件的测试重点有两条：
//! 一条验证 `NewImportSource` 是否把手工输入正确投影成 mydump 元数据和内存文件；
//! 另一条验证 `TargetInfo` 是否给 importer 预检流程返回稳定、可预期的假响应。

use std::collections::HashMap;

use crate::ast;
use crate::context;
use crate::model;
use crate::*;

// 测试写法整体遵循 Go 原用例的组织方式：
// 大量输入数据直接内联在测试体中，
// 这样可以让断言旁边就看到对应的假 schema 和假数据文件。
// 迁移到 Rust 后仍保留这种铺开式写法，
// 因为这些测试更像可读的契约样例，而不是追求极致复用的单元测试模板。
// 代价是测试函数偏长，
// 但优点是任何一个失败点都能快速回溯到对应的构造输入。

// 这个测试块主要覆盖“源数据侧”：
// 先拼测试输入，再检查元数据和内存存储是否与输入一致。

/// Corresponds to Go `TestMockImportSourceBasic`.
#[test]
fn test_mock_import_source_basic() {
    // `db01` 同时含有纯 schema 表和带数据文件的表，用来覆盖两类常见输入。
    // 其中 `tbl01` 只有 schema，帮助确认“没有 data file”不会破坏构造结果。
    let mut tables_db01 = HashMap::new();
    tables_db01.insert(
        "tbl01".to_string(),
        Box::new(TableSourceData {
            DBName: "db01".to_string(),
            TableName: "tbl01".to_string(),
            SchemaFile: Some(Box::new(SourceFile {
                FileName: "/db01/tbl01/tbl01.schema.sql".to_string(),
                Data: b"CREATE TABLE db01.tbl01(id INTEGER PRIMARY KEY AUTO_INCREMENT, strval VARCHAR(64))"
                    .to_vec(),
                TotalSize: 0,
            })),
            DataFiles: vec![],
        }),
    );
    tables_db01.insert(
        "tbl02".to_string(),
        Box::new(TableSourceData {
            DBName: "db01".to_string(),
            TableName: "tbl02".to_string(),
            SchemaFile: Some(Box::new(SourceFile {
                FileName: "/db01/tbl02/tbl02.schema.sql".to_string(),
                Data: b"CREATE TABLE db01.tbl02(id INTEGER PRIMARY KEY AUTO_INCREMENT, val VARCHAR(64))"
                    .to_vec(),
                TotalSize: 0,
            })),
            DataFiles: vec![
                // 同时放 CSV 和 SQL，验证 mock 能按后缀识别不同文件类型。
                Box::new(SourceFile {
                    FileName: "/db01/tbl02/tbl02.data.csv".to_string(),
                    Data: b"val\naaa\nbbb".to_vec(),
                    TotalSize: 0,
                }),
                Box::new(SourceFile {
                    FileName: "/db01/tbl02/tbl02.data.sql".to_string(),
                    Data: b"INSERT INTO db01.tbl02 (val) VALUES ('ccc');".to_vec(),
                    TotalSize: 0,
                }),
            ],
        }),
    );

    // 第二个数据库只保留一张表，用来验证多 DB 聚合不会互相污染。
    let mut mock_data_map: HashMap<String, Box<DBSourceData>> = HashMap::new();
    mock_data_map.insert(
        "db01".to_string(),
        Box::new(DBSourceData {
            Name: "db01".to_string(),
            Tables: tables_db01,
        }),
    );
    mock_data_map.insert(
        "db02".to_string(),
        Box::new(DBSourceData {
            Name: "db02".to_string(),
            Tables: {
                let mut t = HashMap::new();
                t.insert(
                    "tbl01".to_string(),
                    Box::new(TableSourceData {
                        DBName: "db02".to_string(),
                        TableName: "tbl01".to_string(),
                        SchemaFile: Some(Box::new(SourceFile {
                            FileName: "/db02/tbl01/tbl01.schema.sql".to_string(),
                            Data: b"CREATE TABLE db02.tbl01(id INTEGER PRIMARY KEY AUTO_INCREMENT, strval VARCHAR(64))"
                                .to_vec(),
                            TotalSize: 0,
                        })),
                        DataFiles: vec![],
                    }),
                );
                t
            },
        }),
    );

    // Go: ctx, cancel := context.WithCancel(context.Background()); defer cancel()
    let ctx = context::Background();
    let mock_env = NewImportSource(mock_data_map.clone()).expect("NewImportSource");
    // 数据库数量应先在顶层对齐，避免后面表级遍历建立在错误前提上。
    let db_file_metas = mock_env.GetAllDBFileMetas();
    assert_eq!(mock_data_map.len(), db_file_metas.len(), "compare db count");

    for db_file_meta in &db_file_metas {
        // 逐库检查能确认 `HashMap<String, DBSourceData>` 被正确展开成 mydump 视图。
        let db_mock_data = mock_data_map
            .get(&db_file_meta.Name)
            .unwrap_or_else(|| panic!("get mock data by DB: {}", db_file_meta.Name));
        assert_eq!(
            db_mock_data.Tables.len(),
            db_file_meta.Tables.len(),
            "compare table count: {}",
            db_file_meta.Name
        );
        for tbl_file_meta in &db_file_meta.Tables {
            // 每张表既校验 schema 文件，也校验数据文件个数与字节内容。
            // 这能确保 `NewImportSource` 不只是记录路径，而是真的把字节写进了存储。
            let tbl_mock_data = db_mock_data
                .Tables
                .get(&tbl_file_meta.Name)
                .unwrap_or_else(|| {
                    panic!(
                        "get mock data by Table: {}.{}",
                        db_file_meta.Name, tbl_file_meta.Name
                    )
                });
            let schema_file_meta = &tbl_file_meta.SchemaFile;
            let mock_schema_file = tbl_mock_data.SchemaFile.as_ref().expect("SchemaFile");
            // Go same-package: mockEnv.srcStorage.ReadFile
            // Rust 通过 `src_storage()` 暴露只读句柄，语义等价于 Go 同包字段访问。
            let file_data = mock_env
                .src_storage()
                .ReadFile(&ctx, &schema_file_meta.FileMeta.Path)
                .unwrap_or_else(|_| {
                    panic!(
                        "read schema file: {}.{}",
                        db_file_meta.Name, tbl_file_meta.Name
                    )
                });
            assert_eq!(
                mock_schema_file.Data, file_data,
                "compare schema file: {}.{}",
                db_file_meta.Name, tbl_file_meta.Name
            );
            // schema 文件读回正确后，再检查数据文件列表和内容。
            assert_eq!(
                tbl_mock_data.DataFiles.len(),
                tbl_file_meta.DataFiles.len(),
                "compare data file count: {}.{}",
                db_file_meta.Name,
                tbl_file_meta.Name
            );
            for (i, data_file_meta) in tbl_file_meta.DataFiles.iter().enumerate() {
                // 数据文件按顺序逐个核对，确保写入内存存储时没有丢文件或串内容。
                // 这里不额外断言类型字段，因为这些路径已经在 parity 测试里覆盖。
                let mock_data_file = &tbl_mock_data.DataFiles[i];
                let file_data = mock_env
                    .src_storage()
                    .ReadFile(&ctx, &data_file_meta.FileMeta.Path)
                    .unwrap_or_else(|_| {
                        panic!(
                            "read data file: {}.{}: {}",
                            db_file_meta.Name, tbl_file_meta.Name, data_file_meta.FileMeta.Path
                        )
                    });
                assert_eq!(
                    mock_data_file.Data, file_data,
                    "compare data file: {}.{}: {}",
                    db_file_meta.Name, tbl_file_meta.Name, data_file_meta.FileMeta.Path
                );
            }
        }
    }
}

#[test]
fn test_import_source_strips_only_one_gzip_suffix() {
    let mut tables = HashMap::new();
    tables.insert(
        "tbl".to_string(),
        Box::new(TableSourceData {
            DBName: "db".to_string(),
            TableName: "tbl".to_string(),
            SchemaFile: Some(Box::new(SourceFile {
                FileName: "/db/tbl.schema.sql".to_string(),
                Data: Vec::new(),
                TotalSize: 0,
            })),
            DataFiles: vec![Box::new(SourceFile {
                FileName: "/db/tbl.csv.gz.gz".to_string(),
                Data: Vec::new(),
                TotalSize: 0,
            })],
        }),
    );
    let input = HashMap::from([(
        "db".to_string(),
        Box::new(DBSourceData {
            Name: "db".to_string(),
            Tables: tables,
        }),
    )]);

    let err = NewImportSource(input).expect_err("Go TrimSuffix removes only one .gz suffix");
    assert_eq!(err.Error(), "unsupported file type: /db/tbl.csv.gz.gz");
}

// 这一块转向“目标端侧”：
// 关注系统变量、容量统计、空 region、远端表结构与空表判断这些查询接口。
// 目标是证明 importer 预检读取这些接口时能得到和 Go 测试一致的基础语义。
// 这些断言多数不是为了追求复杂算法覆盖，
// 而是为了锁住 mock 返回值的形状，
// 防止后续精简 stub 时不小心改坏调用契约。

/// Corresponds to Go `TestMockTargetInfoBasic`.
///
/// Go compile-time check `var _ importer.TargetInfoGetter = ti` is omitted here:
/// this crate stays slim (no importer/kv/domain deps); method surface is covered below.
#[test]
fn test_mock_target_info_basic() {
    let ctx = context::Background();
    let mut ti = NewTargetInfo();
    // 常量放在测试体内部，目的是让读者在阅读断言时立刻看到量纲和基线值。
    // 这也和 Go 用例里“局部常量服务当前测试”的风格保持一致。

    const REPLICA_COUNT: isize = 3;
    const EMPTY_REGION_COUNT: isize = 5;
    const S01_TOTAL_SIZE: u64 = 10 << 30;
    const S01_TOTAL_SIZE_STR: &str = "10GiB";
    const S01_USED_SIZE: u64 = (7 << 30) + (500 << 20);
    const S02_TOTAL_SIZE: u64 = 50 << 30;
    const S02_TOTAL_SIZE_STR: &str = "50GiB";
    const S02_USED_SIZE: u64 = (35 << 30) + (700 << 20);

    // 系统变量读取应当是对 `SetSysVar` 写入结果的直接回放。
    ti.SetSysVar("aaa", "111");
    ti.SetSysVar("bbb", "222");
    // 读取返回的是克隆结果，但这里先只验证内容是否完整。
    let sys_vars = ti.GetTargetSysVariablesForImport(&ctx, &[]);
    assert_eq!(sys_vars.get("aaa").map(String::as_str), Some("111"));
    assert_eq!(sys_vars.get("bbb").map(String::as_str), Some("222"));

    // 副本数测试覆盖最简单的标量配置读取路径。
    ti.MaxReplicasPerRegion = REPLICA_COUNT;
    let cnt = ti.GetMaxReplica(&ctx).expect("GetMaxReplica");
    assert_eq!(cnt, REPLICA_COUNT as u64);

    // store 容量会被格式化成 Go 风格字符串，因此既检查数值也检查文案。
    ti.StorageInfos.push(StorageInfo {
        TotalSize: S01_TOTAL_SIZE,
        UsedSize: S01_USED_SIZE,
        AvailableSize: S01_TOTAL_SIZE - S01_USED_SIZE,
        RegionCount: 0,
    });
    ti.StorageInfos.push(StorageInfo {
        TotalSize: S02_TOTAL_SIZE,
        UsedSize: S02_USED_SIZE,
        AvailableSize: S02_TOTAL_SIZE - S02_USED_SIZE,
        RegionCount: 0,
    });
    let si = ti.GetStorageInfo(&ctx).expect("GetStorageInfo");
    assert_eq!(si.Count, 2);
    // 这里顺带验证 store 顺序稳定，便于上层测试按索引断言。
    // 若顺序不稳定，很多基于索引的预检断言都会变得脆弱。
    let store = &si.Stores[0];
    assert_eq!(store.Status.Capacity, S01_TOTAL_SIZE_STR);
    assert_eq!(store.Status.RegionSize as u64, S01_USED_SIZE);
    let store = &si.Stores[1];
    assert_eq!(store.Status.Capacity, S02_TOTAL_SIZE_STR);
    assert_eq!(store.Status.RegionSize as u64, S02_USED_SIZE);

    // 空 region 统计会把计数展开成若干 `RegionInfo`，长度必须精确匹配。
    // 这模拟了 importer 用长度判断“空 region 是否充足”的使用方式。
    ti.EmptyRegionCountMap.insert(1, EMPTY_REGION_COUNT);
    let ri = ti.GetEmptyRegionsInfo(&ctx).expect("GetEmptyRegionsInfo");
    assert_eq!(ri.Count, EMPTY_REGION_COUNT as i64);
    assert_eq!(ri.Regions.len(), EMPTY_REGION_COUNT as usize);

    // 第一张表有结构且行数为 0，用来覆盖“空表但结构存在”的分支。
    // 该分支常用于判断是否允许直接导入到空目标表。
    ti.SetTableInfo(
        "testdb",
        "testtbl1",
        Box::new(TableInfo {
            RowCount: 0,
            TableModel: Some(Box::new(model::TableInfo {
                ID: 1,
                Name: ast::NewCIStr("testtbl1"),
                Columns: vec![
                    model::ColumnInfo {
                        ID: 1,
                        Name: ast::NewCIStr("c_1"),
                        Offset: 0,
                    },
                    model::ColumnInfo {
                        ID: 2,
                        Name: ast::NewCIStr("c_2"),
                        Offset: 1,
                    },
                ],
            })),
        }),
    );
    // 第二张表只有行数，没有结构模型，用来模拟 Go map 中的 nil value。
    // 这样可以验证 Rust `Option` 是否正确表达“表存在，但结构不可用”。
    ti.SetTableInfo(
        "testdb",
        "testtbl2",
        Box::new(TableInfo {
            RowCount: 100,
            TableModel: None,
        }),
    );
    let names = vec!["testtbl1".to_string(), "testtbl2".to_string()];
    let tbl_infos = ti
        .FetchRemoteTableModels(&ctx, "testdb", &names)
        .expect("FetchRemoteTableModels");
    // Go returns map[string]*model.TableInfo; len == 2 (testtbl2 value is nil).
    // Rust 这里用 `Option<Box<TableInfo>>` 承接同样语义。
    assert_eq!(tbl_infos.len(), 2);
    for tbl_info in tbl_infos.values() {
        if let Some(tbl_info) = tbl_info {
            // 对存在结构的表，只关心列信息是否按原样透传。
            assert_eq!(tbl_info.Columns.len(), 2);
        }
    }

    // 空表判断最终只依赖 `RowCount == 0`。
    // 因而第一张表返回 true，第二张表返回 false。
    let is_empty_ptr = ti
        .IsTableEmpty(&ctx, "testdb", "testtbl1")
        .expect("IsTableEmpty testtbl1");
    assert!(*is_empty_ptr);
    let is_empty_ptr = ti
        .IsTableEmpty(&ctx, "testdb", "testtbl2")
        .expect("IsTableEmpty testtbl2");
    assert!(!*is_empty_ptr);
}
