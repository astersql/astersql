// Copyright 2026 AsterSQL.

//! Parity tests for `lightning/pkg/importer/mock` public contracts vs Go.
//!
//! 与 `mock_test.rs` 相比，这个文件更强调“契约覆盖”而不是逐个照搬 Go 用例名称。
//! 它把对外行为拆成四类：
//! 正常路径、边界条件、错误返回以及资源/克隆语义，
//! 从而在保持测试体可读的同时，锁住 mock 包给 importer 暴露的关键保证。

use std::collections::HashMap;

use crate::ast;
use crate::context;
use crate::model;
use crate::mydump::SourceType;
use crate::*;

// 这类 parity 测试的价值不在于“重新证明算法正确”，
// 而在于把 Go 侧已经依赖的外部行为拆成稳定片段。
// 一旦未来有人为了精简 mock 包而调整返回值形状，
// 这里就会第一时间提示是否破坏了 importer 上层的历史假设。
// 由于 mock 包承担测试基建职责，
// 契约回归往往会放大成大量用例一起失败，
// 所以提前在这里把语义边界讲清楚很有必要。
// 这也是本文件比普通单元测试更偏“说明书测试”的原因。
// 四个 `contract_*` 的划分也有刻意设计：
// `normal` 锁住主路径装配，
// `boundary` 锁住默认值和后缀解析，
// `error` 锁住失败时机与文本信号，
// `resource_cleanup` 锁住共享句柄与返回值克隆语义。
// 这样的分组让排查失败时可以直接定位到是哪一类承诺被打破。

#[test]
fn go_rust_public_contract_matches() {
    // 入口测试只负责串起四类契约，不在这里堆放断言细节。
    contract_normal();
    contract_boundary();
    contract_error();
    contract_resource_cleanup();
}

/// Go uses `int` for these public mock fields. On the supported 64-bit
/// targets, Rust must not narrow them to `i32` or make `TotalSize` unsigned.
#[test]
fn go_int_fields_keep_signed_pointer_width() {
    let declared_size: isize = -7;
    let source_file = SourceFile {
        TotalSize: declared_size,
        ..Default::default()
    };
    assert_eq!(source_file.TotalSize, -7);

    let wide_value: isize = i32::MAX as isize + 1;
    let mut target = NewTargetInfo();
    target.MaxReplicasPerRegion = wide_value;
    assert_eq!(
        target.GetMaxReplica(&context::Background()).unwrap(),
        wide_value as u64
    );

    let table = TableInfo {
        RowCount: wide_value,
        TableModel: None,
    };
    assert_eq!(table.RowCount, wide_value);

    let storage = StorageInfo {
        RegionCount: wide_value,
        ..Default::default()
    };
    assert_eq!(storage.RegionCount, wide_value);

    target.EmptyRegionCountMap.insert(7, wide_value);
    assert_eq!(target.EmptyRegionCountMap.get(&7), Some(&wide_value));

    let mut tables = HashMap::new();
    tables.insert(
        "t".to_string(),
        Box::new(TableSourceData {
            DBName: "db".to_string(),
            TableName: "t".to_string(),
            SchemaFile: Some(Box::new(SourceFile {
                FileName: "/db/t/t.schema.sql".to_string(),
                Data: b"CREATE TABLE t(id INT)".to_vec(),
                TotalSize: 0,
            })),
            DataFiles: vec![Box::new(SourceFile {
                FileName: "/db/t/t.data.csv".to_string(),
                Data: b"1\n".to_vec(),
                TotalSize: declared_size,
            })],
        }),
    );
    let mut databases = HashMap::new();
    databases.insert(
        "db".to_string(),
        Box::new(DBSourceData {
            Name: "db".to_string(),
            Tables: tables,
        }),
    );
    let source = NewImportSource(databases).unwrap();
    let table_meta = &source.GetDBMetaMap()["db"].Tables[0];
    assert_eq!(table_meta.TotalSize, declared_size as i64);
    assert_eq!(
        table_meta.DataFiles[0].FileMeta.FileSize,
        declared_size as i64
    );
    assert_eq!(
        table_meta.DataFiles[0].FileMeta.RealSize,
        declared_size as i64
    );
}

/// Go's `make([]pdhttp.RegionInfo, count)` panics for a negative `int` count.
#[test]
#[should_panic(expected = "makeslice: len out of range")]
fn negative_empty_region_count_panics_like_go() {
    let mut target = NewTargetInfo();
    target.EmptyRegionCountMap.insert(1, -1);
    let _ = target.GetEmptyRegionsInfo(&context::Background());
}

// `contract_normal` 覆盖日常最常见的成功路径：
// 有多库、多表、多文件的导入源，
// 以及带系统变量、store 容量和表结构的目标端。
// 如果这一块失败，通常意味着 mock 基础装配或 getter 语义整体漂移。
fn contract_normal() {
    // Build ImportSource like TestMockImportSourceBasic and verify meta + storage bytes.
    // 这一段验证“标准输入”是否能被完整投影为 mydump 元数据与可回读文件。
    // 它相当于整个 mock 包最核心的一次端到端冒烟：
    // 从手写输入，
    // 到构造元数据，
    // 再到通过对外 getter 回读结果。
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
                // 同时放 CSV 与 SQL，确认不同文件类型都能进入元数据列表。
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
    let mut mock_data_map = HashMap::new();
    // 第二个数据库只保留一张表，用来证明跨库聚合不会串数据。
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

    let ctx = context::Background();
    let mock_env = NewImportSource(mock_data_map.clone()).expect("NewImportSource");
    let db_file_metas = mock_env.GetAllDBFileMetas();
    // 顶层先比对数据库个数，保证后续表级断言建立在正确前提上。
    assert_eq!(mock_data_map.len(), db_file_metas.len(), "compare db count");

    let storage = mock_env.GetStorage();
    // 用克隆后的 storage 句柄读文件，顺带覆盖 `GetStorage` 的共享语义。
    for db_file_meta in &db_file_metas {
        let db_mock = mock_data_map
            .get(&db_file_meta.Name)
            .unwrap_or_else(|| panic!("get mock data by DB: {}", db_file_meta.Name));
        assert_eq!(
            db_mock.Tables.len(),
            db_file_meta.Tables.len(),
            "compare table count: {}",
            db_file_meta.Name
        );
        for tbl_file_meta in &db_file_meta.Tables {
            // schema 与 data file 都逐个回读，验证 `NewImportSource` 不是只记录路径。
            let tbl_mock = db_mock.Tables.get(&tbl_file_meta.Name).unwrap_or_else(|| {
                panic!(
                    "get mock data by Table: {}.{}",
                    db_file_meta.Name, tbl_file_meta.Name
                )
            });
            let schema_path = &tbl_file_meta.SchemaFile.FileMeta.Path;
            // schema 文件必须能按路径读回原始字节。
            let file_data = storage.ReadFile(&ctx, schema_path).unwrap_or_else(|_| {
                panic!(
                    "read schema file: {}.{}",
                    db_file_meta.Name, tbl_file_meta.Name
                )
            });
            assert_eq!(
                tbl_mock.SchemaFile.as_ref().unwrap().Data,
                file_data,
                "compare schema file: {}.{}",
                db_file_meta.Name,
                tbl_file_meta.Name
            );
            assert_eq!(
                tbl_mock.DataFiles.len(),
                tbl_file_meta.DataFiles.len(),
                "compare data file count: {}.{}",
                db_file_meta.Name,
                tbl_file_meta.Name
            );
            for (i, data_file_meta) in tbl_file_meta.DataFiles.iter().enumerate() {
                // 数据文件顺序也需要稳定，因为很多上层逻辑会按 mydump 列表顺序消费。
                let mock_data_file = &tbl_mock.DataFiles[i];
                let file_data = storage
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

    // TargetInfo normal path mirrors TestMockTargetInfoBasic sysvars / replica / stores.
    // 下半段切到目标端 mock，验证 importer 预检常用查询的 happy path。
    let mut ti = NewTargetInfo();
    ti.SetSysVar("aaa", "111");
    ti.SetSysVar("bbb", "222");
    // 系统变量读取应该是纯复制，不带额外计算。
    let sys_vars = ti.GetTargetSysVariablesForImport(&ctx, &[]);
    assert_eq!(sys_vars.get("aaa").map(String::as_str), Some("111"));
    assert_eq!(sys_vars.get("bbb").map(String::as_str), Some("222"));

    const REPLICA_COUNT: isize = 3;
    ti.MaxReplicasPerRegion = REPLICA_COUNT;
    // 正常配置下，副本数应原样返回。
    assert_eq!(ti.GetMaxReplica(&ctx).unwrap(), REPLICA_COUNT as u64);

    const S01_TOTAL: u64 = 10 << 30;
    const S01_USED: u64 = (7 << 30) + (500 << 20);
    const S02_TOTAL: u64 = 50 << 30;
    const S02_USED: u64 = (35 << 30) + (700 << 20);
    ti.StorageInfos.push(StorageInfo {
        TotalSize: S01_TOTAL,
        UsedSize: S01_USED,
        AvailableSize: S01_TOTAL - S01_USED,
        RegionCount: 0,
    });
    ti.StorageInfos.push(StorageInfo {
        TotalSize: S02_TOTAL,
        UsedSize: S02_USED,
        AvailableSize: S02_TOTAL - S02_USED,
        RegionCount: 0,
    });
    let si = ti.GetStorageInfo(&ctx).unwrap();
    // 这里同时检查容量文案与已用空间数值，避免格式化和数值投影任一侧回退。
    assert_eq!(si.Count, 2);
    assert_eq!(si.Stores[0].Status.Capacity, "10GiB");
    assert_eq!(si.Stores[0].Status.RegionSize, S01_USED as i64);
    assert_eq!(si.Stores[1].Status.Capacity, "50GiB");
    assert_eq!(si.Stores[1].Status.RegionSize, S02_USED as i64);
    assert_eq!(si.Stores[0].Store.ID, 1);
    assert_eq!(si.Stores[0].Store.StateName, "Up");

    // 远端表信息一张有结构、一张只有行数，用来覆盖 `Some`/`None` 两条路径。
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
    // Rust 用 `Option` 表达 Go 的 nil 指针，这里检查两种值都能共存于同一 map。
    assert_eq!(tbl_infos.len(), 2);
    for tbl in tbl_infos.values() {
        if let Some(t) = tbl {
            assert_eq!(t.Columns.len(), 2);
        }
    }
    assert!(*ti.IsTableEmpty(&ctx, "testdb", "testtbl1").unwrap());
    assert!(!*ti.IsTableEmpty(&ctx, "testdb", "testtbl2").unwrap());
}

// `contract_boundary` 专门收纳“输入合法但接近边界”的情况。
// 它关注默认值、回退值、压缩后缀和空对象处理，
// 这些都是实现里最容易被重构顺手改掉，却又不会在 happy path 立刻暴露的问题。
fn contract_boundary() {
    let ctx = context::Background();
    let mut ti = NewTargetInfo();

    // MaxReplicasPerRegion <= 0 falls back to 1 (Go GetMaxReplica).
    // 这是 importer 预检里非常依赖的防御性分支，避免非法配置导致后续除零或阈值错误。
    ti.MaxReplicasPerRegion = 0;
    assert_eq!(ti.GetMaxReplica(&ctx).unwrap(), 1);
    ti.MaxReplicasPerRegion = -3;
    assert_eq!(ti.GetMaxReplica(&ctx).unwrap(), 1);

    // EmptyRegionCountMap expands peers per store.
    // 断言长度和 store ID，确保“按 store 展开 region”这一投影关系稳定。
    const EMPTY_REGION_COUNT: isize = 5;
    ti.EmptyRegionCountMap.insert(1, EMPTY_REGION_COUNT);
    let ri = ti.GetEmptyRegionsInfo(&ctx).unwrap();
    assert_eq!(ri.Count, EMPTY_REGION_COUNT as i64);
    assert_eq!(ri.Regions.len(), EMPTY_REGION_COUNT as usize);
    assert_eq!(ri.Regions[0].Peers[0].StoreID, 1);

    // Missing schema/table => empty.
    // 这里约定不存在目标对象时返回 true，而不是错误，方便预检把它当成“可导入空目标”。
    assert!(*ti.IsTableEmpty(&ctx, "missing_db", "t").unwrap());
    ti.SetTableInfo(
        "onlydb",
        "onlytbl",
        Box::new(TableInfo {
            RowCount: 1,
            TableModel: None,
        }),
    );
    // 只有库存在但表不存在时，仍然应该返回“空表”，而不是错误或 false。
    assert!(*ti.IsTableEmpty(&ctx, "onlydb", "missing_tbl").unwrap());

    // TotalSize override when non-zero; gzip suffix sets compression + type from stripped name.
    // 这组输入覆盖两个细节：
    // 一是 `TotalSize` 显式值应覆盖字节长度，
    // 二是 `.gz` 压缩后缀不应影响去掉压缩层后的类型识别。
    let mut tables = HashMap::new();
    tables.insert(
        "t".to_string(),
        Box::new(TableSourceData {
            DBName: "db".to_string(),
            TableName: "t".to_string(),
            SchemaFile: Some(Box::new(SourceFile {
                FileName: "/db/t/t.schema.sql.gz".to_string(),
                Data: b"CREATE TABLE t(id INT)".to_vec(),
                TotalSize: 0,
            })),
            DataFiles: vec![Box::new(SourceFile {
                FileName: "/db/t/t.data.csv.gz".to_string(),
                Data: b"1\n2\n".to_vec(),
                TotalSize: 999,
            })],
        }),
    );
    let mut map = HashMap::new();
    map.insert(
        "db".to_string(),
        Box::new(DBSourceData {
            Name: "db".to_string(),
            Tables: tables,
        }),
    );
    let src = NewImportSource(map).unwrap();
    let metas = src.GetAllDBFileMetas();
    // 单表场景下，最关心的是 size、compression、type 三个元数据字段。
    // 它们直接影响 importer 后续如何估算体积、识别格式并决定读取分支。
    assert_eq!(metas.len(), 1);
    let tbl = &metas[0].Tables[0];
    assert_eq!(tbl.TotalSize, 999);
    assert_eq!(
        tbl.SchemaFile.FileMeta.Compression,
        crate::mydump::Compression::GZ
    );
    assert_eq!(tbl.DataFiles[0].FileMeta.Type, SourceType::CSV);
    assert_eq!(
        tbl.DataFiles[0].FileMeta.Compression,
        crate::mydump::Compression::GZ
    );
    assert_eq!(tbl.DataFiles[0].FileMeta.FileSize, 999);
    assert_eq!(tbl.DataFiles[0].FileMeta.RealSize, 999);

    // CheckVersionRequirements is a no-op success.
    // 只要它持续返回成功，上层就能把版本检查视为已满足。
    assert!(ti.CheckVersionRequirements(&ctx).is_ok());

    // FetchRemoteDBModels returns one entry per schema key.
    // 这说明 `TargetInfo` 不会自行过滤 schema 条目。
    let dbs = ti.FetchRemoteDBModels(&ctx).unwrap();
    assert_eq!(dbs.len(), 1);
    // 返回名字也要保持原始 schema 文本，不能被意外 lower-case 或清洗。
    assert_eq!(dbs[0].Name.O, "onlydb");
}

// `contract_error` 锁住失败文案和失败时机。
// 对 mock 包来说，错误文本本身也是对外契约的一部分，
// 因为上层测试经常直接匹配其中的关键片段来断言错误类型。
fn contract_error() {
    // Unsupported data file extension.
    // 错误路径首先验证 `NewImportSource` 会拒绝未知扩展名，而不是默默生成错误元数据。
    let mut tables = HashMap::new();
    tables.insert(
        "t".to_string(),
        Box::new(TableSourceData {
            DBName: "db".to_string(),
            TableName: "t".to_string(),
            SchemaFile: Some(Box::new(SourceFile {
                FileName: "/db/t/t.schema.sql".to_string(),
                Data: b"CREATE TABLE t(id INT)".to_vec(),
                TotalSize: 0,
            })),
            DataFiles: vec![Box::new(SourceFile {
                FileName: "/db/t/t.data.bin".to_string(),
                Data: b"xxx".to_vec(),
                TotalSize: 0,
            })],
        }),
    );
    let mut map = HashMap::new();
    map.insert(
        "db".to_string(),
        Box::new(DBSourceData {
            Name: "db".to_string(),
            Tables: tables,
        }),
    );
    let err = NewImportSource(map).unwrap_err();
    // 错误文本直接包含原始文件名，方便调用方把失败定位回具体输入文件。
    // 这也能防止未来把错误过度包装后丢掉最有用的定位信息。
    assert!(
        err.Error()
            .contains("unsupported file type: /db/t/t.data.bin"),
        "got {}",
        err.Error()
    );

    // Missing remote DB yields HTTP-shaped BadDB error.
    // 这里锁住的是“外层 HTTP 失败包装 + 内层 Unknown database”这类历史兼容文案。
    let ctx = context::Background();
    let ti = NewTargetInfo();
    let err = ti
        .FetchRemoteTableModels(&ctx, "no_such_db", &["t".to_string()])
        .unwrap_err();
    // 这里不去比较整段完整字符串，而是锁定多个关键信号，
    // 以便在允许少量文案调整的同时仍能防住语义退化。
    let msg = err.Error();
    assert!(
        msg.contains("get xxxxxx http status code != 200, message"),
        "got {msg}"
    );
    assert!(msg.contains("Unknown database"), "got {msg}");
    assert!(msg.contains("no_such_db"), "got {msg}");
    // 末尾再要求包含错误码或 schema 关键字，避免错误对象被过度简化。
    assert!(msg.contains("1049") || msg.contains("schema"), "got {msg}");
}

// `contract_resource_cleanup` 看似零散，实际上收束了两个容易被忽略的生命周期保证：
// 一个是 storage 克隆出来后仍然共享底层内容，
// 另一个是系统变量 getter 返回独立副本而非内部可变引用。
// 这两点都不是业务逻辑本身，
// 但非常影响测试基建在复杂场景下是否稳定。
fn contract_resource_cleanup() {
    // MemStorage is shared via Arc; GetStorage clone still reads written files;
    // dropping ImportSource must not invalidate an outstanding storage handle mid-read.
    // 这条契约保证测试拿到的 storage 句柄具有独立生命周期，不会随着 `ImportSource` 提前失效。
    let mut tables = HashMap::new();
    tables.insert(
        "t".to_string(),
        Box::new(TableSourceData {
            DBName: "db".to_string(),
            TableName: "t".to_string(),
            SchemaFile: Some(Box::new(SourceFile {
                FileName: "/db/t/t.schema.sql".to_string(),
                Data: b"CREATE TABLE t(id INT)".to_vec(),
                TotalSize: 0,
            })),
            DataFiles: vec![],
        }),
    );
    let mut map = HashMap::new();
    map.insert(
        "db".to_string(),
        Box::new(DBSourceData {
            Name: "db".to_string(),
            Tables: tables,
        }),
    );
    let src = NewImportSource(map).unwrap();
    let storage = src.GetStorage();
    let ctx = context::Background();
    // 先在源对象存在时读一次，建立基线。
    // 如果这里失败，说明 `GetStorage` 本身就没有拿到正确底层句柄。
    let data = storage
        .ReadFile(&ctx, "/db/t/t.schema.sql")
        .expect("read before drop");
    assert_eq!(data, b"CREATE TABLE t(id INT)");
    drop(src);
    // 再在源对象释放后读一次，确认底层 Arc 仍然持有文件内容。
    // 这一步主要防回归“storage 意外持有弱引用或借用”的实现。
    let data2 = storage
        .ReadFile(&ctx, "/db/t/t.schema.sql")
        .expect("read after ImportSource drop");
    assert_eq!(data2, b"CREATE TABLE t(id INT)");

    // SysVar map clone is independent (maps.Clone semantics).
    // 返回值必须是独立副本，否则调用方修改结果会污染内部状态，导致后续测试串扰。
    let mut ti = NewTargetInfo();
    ti.SetSysVar("k", "v");
    let mut cloned = ti.GetTargetSysVariablesForImport(&ctx, &[]);
    // 改写返回值副本后，原对象中的值应保持不变。
    cloned.insert("k".to_string(), "mutated".to_string());
    assert_eq!(
        ti.GetTargetSysVariablesForImport(&ctx, &[])
            .get("k")
            .map(String::as_str),
        Some("v")
    );
}
