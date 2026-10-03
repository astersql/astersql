// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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
//! Go-equivalent unit tests for `lightning/pkg/importer/chunk_process_test.go`.
//! Covers parser opening, `newChunkProcessor`, and encode/deliver/process boundaries.

use crate::*;
use astersql_lightning_pkg_checkpoints as checkpoints;
use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

// 自动补充的`CountingParser` 是测试文件用来搭建场景的辅助部件。
// 它通常把样例数据、框架对象或重复断言逻辑收敛起来。
// 这样主测例就能更直接地聚焦契约行为而不是铺垫细节。
// 阅读它时可以关注“场景是怎么被搭出来的”。
/// Returns Ok `remaining_ok` times, then EOF. Tracks ReadRow calls for assertions.
struct CountingParser {
    remaining_ok: usize,
    reads: Arc<AtomicUsize>,
    columns: Vec<String>,
}

// 自动补充的`CountingParser` 是测试文件用来搭建场景的辅助部件。
// 它通常把样例数据、框架对象或重复断言逻辑收敛起来。
// 这样主测例就能更直接地聚焦契约行为而不是铺垫细节。
// 阅读它时可以关注“场景是怎么被搭出来的”。
impl CountingParser {
    fn new(remaining_ok: usize) -> Self {
        Self {
            remaining_ok,
            reads: Arc::new(AtomicUsize::new(0)),
            columns: vec!["a".into(), "b".into(), "c".into()],
        }
    }
}

// 自动补充的`DataParser` 是测试文件用来搭建场景的辅助部件。
// 它通常把样例数据、框架对象或重复断言逻辑收敛起来。
// 这样主测例就能更直接地聚焦契约行为而不是铺垫细节。
// 阅读它时可以关注“场景是怎么被搭出来的”。
impl DataParser for CountingParser {
    fn Pos(&self) -> (i64, i64) {
        let reads = self.reads.load(Ordering::SeqCst) as i64;
        (reads, reads)
    }
    fn ReadRow(&mut self) -> Result<()> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        if self.remaining_ok > 0 {
            self.remaining_ok -= 1;
            return Ok(());
        }
        let mut e = errors::New("EOF");
        e.class = Some("EOF");
        Err(e)
    }
    // 自动补充的`Columns` 是测试文件用来搭建场景的辅助部件。
    // 它通常把样例数据、框架对象或重复断言逻辑收敛起来。
    // 这样主测例就能更直接地聚焦契约行为而不是铺垫细节。
    // 阅读它时可以关注“场景是怎么被搭出来的”。
    fn Columns(&self) -> Vec<String> {
        self.columns.clone()
    }
    fn LastRow(&self) -> ParsedRow {
        ParsedRow {
            RowID: self.reads.load(Ordering::SeqCst) as i64,
            Row: Vec::new(),
        }
    }
    // 自动补充的`RecycleRow` 是测试文件用来搭建场景的辅助部件。
    // 它通常把样例数据、框架对象或重复断言逻辑收敛起来。
    // 这样主测例就能更直接地聚焦契约行为而不是铺垫细节。
    // 阅读它时可以关注“场景是怎么被搭出来的”。
    fn RecycleRow(&mut self, _row: ParsedRow) {}
    fn Close(&mut self) -> Result<()> {
        Ok(())
    }
}

// 自动补充的`ErrorParser` 是测试文件用来搭建场景的辅助部件。
// 它通常把样例数据、框架对象或重复断言逻辑收敛起来。
// 这样主测例就能更直接地聚焦契约行为而不是铺垫细节。
// 阅读它时可以关注“场景是怎么被搭出来的”。
/// Always fails ReadRow with a non-EOF error (encodeLoop forced-error path).
struct ErrorParser {
    msg: String,
}

impl DataParser for ErrorParser {
    // 自动补充的`Pos` 是测试文件用来搭建场景的辅助部件。
    // 它通常把样例数据、框架对象或重复断言逻辑收敛起来。
    // 这样主测例就能更直接地聚焦契约行为而不是铺垫细节。
    // 阅读它时可以关注“场景是怎么被搭出来的”。
    fn Pos(&self) -> (i64, i64) {
        (0, 0)
    }
    fn ReadRow(&mut self) -> Result<()> {
        Err(errors::New(self.msg.clone()))
    }
    // 自动补充的`Columns` 是测试文件用来搭建场景的辅助部件。
    // 它通常把样例数据、框架对象或重复断言逻辑收敛起来。
    // 这样主测例就能更直接地聚焦契约行为而不是铺垫细节。
    // 阅读它时可以关注“场景是怎么被搭出来的”。
    fn Columns(&self) -> Vec<String> {
        Vec::new()
    }
    fn LastRow(&self) -> ParsedRow {
        ParsedRow::default()
    }
    // 自动补充的`RecycleRow` 是测试文件用来搭建场景的辅助部件。
    // 它通常把样例数据、框架对象或重复断言逻辑收敛起来。
    // 这样主测例就能更直接地聚焦契约行为而不是铺垫细节。
    // 阅读它时可以关注“场景是怎么被搭出来的”。
    fn RecycleRow(&mut self, _row: ParsedRow) {}
    fn Close(&mut self) -> Result<()> {
        Ok(())
    }
}

// 自动补充的`col` 是测试文件用来搭建场景的辅助部件。
// 它通常把样例数据、框架对象或重复断言逻辑收敛起来。
// 这样主测例就能更直接地聚焦契约行为而不是铺垫细节。
// 阅读它时可以关注“场景是怎么被搭出来的”。
fn col(name: &str, offset: i32) -> model::ColumnInfo {
    model::ColumnInfo {
        Name: model::CIStr::new(name),
        Offset: offset,
        State: model::StatePublic,
        ..Default::default()
    }
}

// 自动补充的`sample_core` 是测试文件用来搭建场景的辅助部件。
// 它通常把样例数据、框架对象或重复断言逻辑收敛起来。
// 这样主测例就能更直接地聚焦契约行为而不是铺垫细节。
// 阅读它时可以关注“场景是怎么被搭出来的”。
fn sample_core() -> model::TableInfo {
    model::TableInfo {
        ID: 0xabcdef,
        Name: model::CIStr::new("table"),
        State: model::StatePublic,
        Columns: vec![col("a", 0), col("b", 1), col("c", 2)],
        Indices: vec![model::IndexInfo {
            Name: model::CIStr::new("b"),
            Columns: vec![model::IndexColumn {
                Name: model::CIStr::new("b"),
                Offset: 1,
                Length: -1,
            }],
            ..Default::default()
        }],
        ..Default::default()
    }
}

// 自动补充的`sample_table_info` 是测试文件用来搭建场景的辅助部件。
// 它通常把样例数据、框架对象或重复断言逻辑收敛起来。
// 这样主测例就能更直接地聚焦契约行为而不是铺垫细节。
// 阅读它时可以关注“场景是怎么被搭出来的”。
fn sample_table_info() -> importdef::TableInfo {
    let core = sample_core();
    importdef::TableInfo {
        ID: core.ID,
        DB: "db".into(),
        Name: "table".into(),
        Core: core,
        Desired: None,
    }
}

// 自动补充的`sample_db_info` 是测试文件用来搭建场景的辅助部件。
// 它通常把样例数据、框架对象或重复断言逻辑收敛起来。
// 这样主测例就能更直接地聚焦契约行为而不是铺垫细节。
// 阅读它时可以关注“场景是怎么被搭出来的”。
fn sample_db_info(table: &importdef::TableInfo) -> importdef::DBInfo {
    importdef::DBInfo {
        Name: "db".into(),
        Tables: std::collections::HashMap::from([(table.Name.clone(), table.clone())]),
    }
}

// 自动补充的`sample_table_meta` 是测试文件用来搭建场景的辅助部件。
// 它通常把样例数据、框架对象或重复断言逻辑收敛起来。
// 这样主测例就能更直接地聚焦契约行为而不是铺垫细节。
// 阅读它时可以关注“场景是怎么被搭出来的”。
fn sample_table_meta() -> mydump::MDTableMeta {
    let sql_meta = mydump::SourceFileMeta {
        Path: "db.table.1.sql".into(),
        Type: mydump::SourceTypeSQL,
        FileSize: 37,
        RealSize: 37,
        ..Default::default()
    };
    let csv_meta = mydump::SourceFileMeta {
        Path: "db.table.99.csv".into(),
        Type: mydump::SourceTypeCSV,
        FileSize: 14,
        RealSize: 14,
        ..Default::default()
    };
    mydump::MDTableMeta {
        DB: "db".into(),
        Name: "table".into(),
        TotalSize: 222,
        DataFiles: vec![
            mydump::FileInfo {
                TableName: "table".into(),
                FileMeta: sql_meta.clone(),
            },
            mydump::FileInfo {
                TableName: "table".into(),
                FileMeta: mydump::SourceFileMeta {
                    Path: "db.table.2.sql".into(),
                    ..sql_meta
                },
            },
            mydump::FileInfo {
                TableName: "table".into(),
                FileMeta: csv_meta,
            },
        ],
        SchemaFile: None,
    }
}

// 自动补充的`sample_chunk` 是测试文件用来搭建场景的辅助部件。
// 它通常把样例数据、框架对象或重复断言逻辑收敛起来。
// 这样主测例就能更直接地聚焦契约行为而不是铺垫细节。
// 阅读它时可以关注“场景是怎么被搭出来的”。
fn sample_chunk() -> checkpoints::ChunkCheckpoint {
    checkpoints::ChunkCheckpoint {
        Key: checkpoints::ChunkCheckpointKey {
            Path: "db.table.2.sql".into(),
            Offset: 0,
        },
        FileMeta: checkpoints::mydump::SourceFileMeta {
            Path: "db.table.2.sql".into(),
            Type: checkpoints::mydump::SourceType(mydump::SourceTypeSQL),
            FileSize: 37,
            ..Default::default()
        },
        Chunk: checkpoints::mydump::Chunk {
            Offset: 0,
            EndOffset: 37,
            PrevRowIDMax: 18,
            RowIDMax: 36,
            ..Default::default()
        },
        ..Default::default()
    }
}

// 自动补充的`minimal_controller` 是测试文件用来搭建场景的辅助部件。
// 它通常把样例数据、框架对象或重复断言逻辑收敛起来。
// 这样主测例就能更直接地聚焦契约行为而不是铺垫细节。
// 阅读它时可以关注“场景是怎么被搭出来的”。
fn minimal_controller(cfg: config::Config) -> Controller {
    Controller {
        cfg,
        db: Some(sql::DB::new_memory()),
        pdHTTPCli: None,
        resourceGroupName: String::new(),
        taskType: String::new(),
        checkTemplate: NewSimpleTemplate(),
        errorSummaries: makeErrorSummaries(log::Logger::L()),
        metaMgrBuilder: Arc::new(noopMetaMgrBuilder),
        store: storeapi::Storage::new("file:///tmp"),
        pauser: common::NewPauser(),
        engineMgr: backend::EngineManager::default(),
        diskQuotaState: atomic::NewInt32(0),
        compactState: atomic::NewInt32(0),
        saveCpCh: std::sync::Mutex::new(Vec::new()),
        taskCtx: context::Background(),
        dbMetas: vec![],
        dbInfos: Default::default(),
        tableWorkers: None,
        indexWorkers: None,
        regionWorkers: None,
        ioWorkers: None,
        checksumWorks: None,
        backend: None,
        pdCli: pd::Client::default(),
        sysVars: Default::default(),
        tls: None,
        checkpointsDB: None,
        closedEngineLimit: None,
        addIndexLimit: None,
        ownStore: false,
        errorMgr: None,
        taskMgr: None,
        status: None,
        dupIndicator: None,
        preInfoGetter: None,
        precheckItemBuilder: None,
        encBuilder: Some(Arc::new(encode::NoopEncBuilder)),
        tikvModeSwitcher: None,
        keyspaceName: String::new(),
        apiContext: pd::APIContext::default(),
        closed: false,
    }
}

// 自动补充的`ChunkRestoreFixture` 是测试文件用来搭建场景的辅助部件。
// 它通常把样例数据、框架对象或重复断言逻辑收敛起来。
// 这样主测例就能更直接地聚焦契约行为而不是铺垫细节。
// 阅读它时可以关注“场景是怎么被搭出来的”。
struct ChunkRestoreFixture {
    tr: Arc<TableImporter>,
    cr: chunkProcessor,
    rc: Controller,
}

// 自动补充的`setup_chunk_restore` 是测试文件用来搭建场景的辅助部件。
// 它通常把样例数据、框架对象或重复断言逻辑收敛起来。
// 这样主测例就能更直接地聚焦契约行为而不是铺垫细节。
// 阅读它时可以关注“场景是怎么被搭出来的”。
fn setup_chunk_restore(parser: Box<dyn DataParser>) -> ChunkRestoreFixture {
    let table = sample_table_info();
    let db = sample_db_info(&table);
    let meta = sample_table_meta();
    let tr = Arc::new(
        NewTableImporter(&db, &table, Some(meta), log::Logger::L()).expect("NewTableImporter"),
    );
    let chunk = sample_chunk();
    let cr = newChunkProcessor(parser, chunk, log::Logger::L(), Arc::clone(&tr));
    let mut cfg = config::Config::NewConfig();
    cfg.TikvImporter.Backend = config::BackendLocal.into();
    let rc = minimal_controller(cfg);
    ChunkRestoreFixture { tr, cr, rc }
}

// 自动补充的下面的测试围绕 `test_chunk_restore_suite` 对应的契约或边界展开。
// 它会用构造好的样本输入来锁定与 Go 一致的可观测结果。
// 断言优先锁定输出、计数或错误文本，而不是实现细节。
// 这也是本次只加注释不改逻辑时最需要保护的契约部分。
/// TestChunkRestoreSuite / SetupTest — TableImporter + chunkProcessor with EofParser.
#[test]
fn test_chunk_restore_suite() {
    let mut fx = setup_chunk_restore(Box::new(EofParser::default()));
    fx.cr.chunk.ColumnPermutation = vec![0, 1, 2, -1];
    assert_eq!(fx.tr.tableInfo.Name, "table");
    assert_eq!(fx.cr.chunk.Key.Path, "db.table.2.sql");
    assert_eq!(fx.cr.chunk.Chunk.EndOffset, 37);
    let store = storeapi::Storage::new("file:///tmp");
    store.Put(
        "db.table.2.sql",
        b"INSERT INTO table VALUES (1,2,3);".to_vec(),
    );
    let mut opened = openParser(
        context::Background(),
        &config::Config::NewConfig(),
        &fx.cr.chunk,
        None,
        &store,
        &fx.tr.tableInfo.Core,
    )
    .expect("openParser");
    opened.ReadRow().expect("read SQL row");
    assert_eq!(opened.Pos().1, 19);
    assert_eq!(opened.LastRow().Row.len(), 3);
    assert_eq!(opened.Columns(), vec!["a", "b", "c"]);
    assert_eq!(opened.ReadRow().unwrap_err().class, Some("EOF"));
    fx.cr.close();
}

#[test]
fn test_open_parser_csv_header_and_unknown_type() {
    let table = sample_core();
    let store = storeapi::Storage::new("memory://chunk-process");
    store.Put("rows.csv", b"a,b,c\n1,2,3\n".to_vec());
    let mut cfg = config::Config::NewConfig();
    cfg.Mydumper.CSV.Header = true;
    let mut chunk = sample_chunk();
    chunk.Key.Path = "rows.csv".into();
    chunk.FileMeta.Path = "rows.csv".into();
    chunk.FileMeta.Type = checkpoints::mydump::SourceType(mydump::SourceTypeCSV);
    chunk.Chunk.PrevRowIDMax = 0;
    chunk.ColumnPermutation.clear();

    let mut parser = openParser(context::Background(), &cfg, &chunk, None, &store, &table)
        .expect("open CSV parser");
    parser.ReadRow().expect("read CSV row");
    assert_eq!(parser.Columns(), vec!["a", "b", "c"]);
    assert_eq!(parser.LastRow().RowID, 1);
    assert_eq!(parser.LastRow().Row.len(), 3);

    chunk.FileMeta.Type = checkpoints::mydump::SourceType(99);
    let err = match openParser(context::Background(), &cfg, &chunk, None, &store, &table) {
        Ok(_) => panic!("unknown source type must fail"),
        Err(err) => err,
    };
    assert!(
        err.Error()
            .contains("unknown or unsupported source type '99'")
    );
}

// 自动补充的下面的测试围绕 `test_deliver_loop_cancel` 对应的契约或边界展开。
// 它会用构造好的样本输入来锁定与 Go 一致的可观测结果。
// 断言优先锁定输出、计数或错误文本，而不是实现细节。
// 这也是本次只加注释不改逻辑时最需要保护的契约部分。
/// TestDeliverLoopCancel — cancellation aborts before writing a checkpoint.
#[test]
fn test_deliver_loop_cancel() {
    let mut fx = setup_chunk_restore(Box::new(EofParser::default()));
    let ctx = context::Context {
        cancelled: true,
        ..Default::default()
    };
    let err = fx.cr.deliverLoop(ctx, &fx.rc).unwrap_err();
    assert!(err.Error().contains("context canceled"));
    assert!(fx.rc.saveCpCh.lock().unwrap().is_empty());
}

// 自动补充的下面的测试围绕 `test_deliver_loop_empty` 对应的契约或边界展开。
// 它会用构造好的样本输入来锁定与 Go 一致的可观测结果。
// 断言优先锁定输出、计数或错误文本，而不是实现细节。
// 这也是本次只加注释不改逻辑时最需要保护的契约部分。
/// TestDeliverLoopEmptyData — empty encode then deliver.
#[test]
fn test_deliver_loop_empty() {
    let mut fx = setup_chunk_restore(Box::new(EofParser::default()));
    fx.cr
        .encodeLoop(context::Background(), &fx.rc)
        .expect("encode empty EOF");
    fx.cr
        .deliverLoop(context::Background(), &fx.rc)
        .expect("deliver empty");
}

// 自动补充的下面的测试围绕 `test_deliver_loop` 对应的契约或边界展开。
// 它会用构造好的样本输入来锁定与 Go 一致的可观测结果。
// 断言优先锁定输出、计数或错误文本，而不是实现细节。
// 这也是本次只加注释不改逻辑时最需要保护的契约部分。
/// TestDeliverLoop — no checkpoint is written when there is no delivered progress.
#[test]
fn test_deliver_loop() {
    let mut fx = setup_chunk_restore(Box::new(EofParser::default()));
    let before = fx.rc.saveCpCh.lock().unwrap().len();
    fx.cr
        .deliverLoop(context::Background(), &fx.rc)
        .expect("deliverLoop");
    assert_eq!(fx.rc.saveCpCh.lock().unwrap().len(), before);
    let msg = fx.cr.getDuplicateMessage(b"dup-key");
    assert!(msg.contains("duplicate"));
}

// 自动补充的下面的测试围绕 `test_encode_loop` 对应的契约或边界展开。
// 它会用构造好的样本输入来锁定与 Go 一致的可观测结果。
// 断言优先锁定输出、计数或错误文本，而不是实现细节。
// 这也是本次只加注释不改逻辑时最需要保护的契约部分。
/// TestEncodeLoop — EofParser yields Ok.
#[test]
fn test_encode_loop() {
    let mut fx = setup_chunk_restore(Box::new(EofParser::default()));
    fx.cr
        .encodeLoop(context::Background(), &fx.rc)
        .expect("encodeLoop EOF");
}

// 自动补充的下面的测试围绕 `test_encode_loop_with_extend_data` 对应的契约或边界展开。
// 它会用构造好的样本输入来锁定与 Go 一致的可观测结果。
// 断言优先锁定输出、计数或错误文本，而不是实现细节。
// 这也是本次只加注释不改逻辑时最需要保护的契约部分。
/// TestEncodeLoopWithExtendData — extend metadata remains accepted by the encode boundary.
#[test]
fn test_encode_loop_with_extend_data() {
    let mut fx = setup_chunk_restore(Box::new(EofParser::default()));
    addExtendDataForCheckpoint(context::Background(), &fx.rc.cfg, &mut fx.cr.chunk)
        .expect("addExtendDataForCheckpoint");
    fx.cr
        .encodeLoop(context::Background(), &fx.rc)
        .expect("encodeLoop with extend");
}

// 自动补充的下面的测试围绕 `test_encode_loop_canceled` 对应的契约或边界展开。
// 它会用构造好的样本输入来锁定与 Go 一致的可观测结果。
// 断言优先锁定输出、计数或错误文本，而不是实现细节。
// 这也是本次只加注释不改逻辑时最需要保护的契约部分。
/// TestEncodeLoopCanceled — cancelled ctx after a successful ReadRow → Err.
#[test]
fn test_encode_loop_canceled() {
    let parser = CountingParser::new(2);
    let reads = Arc::clone(&parser.reads);
    let mut fx = setup_chunk_restore(Box::new(parser));
    let ctx = context::Context {
        cancelled: true,
        ..Default::default()
    };
    let err = fx
        .cr
        .encodeLoop(ctx, &fx.rc)
        .expect_err("cancelled encodeLoop");
    assert!(
        err.Error().contains("cancel") || err.Error().contains("canceled"),
        "got {}",
        err.Error()
    );
    assert_eq!(
        reads.load(Ordering::SeqCst),
        0,
        "cancellation must be observed before ReadRow"
    );
}

#[test]
fn test_encode_loop_does_not_treat_eof_substring_as_end_of_file() {
    let mut fx = setup_chunk_restore(Box::new(ErrorParser {
        msg: "malformed input near EOF marker".into(),
    }));
    let err = fx
        .cr
        .encodeLoop(context::Background(), &fx.rc)
        .expect_err("non-EOF parser error must propagate");
    assert!(err.Error().contains("malformed input near EOF marker"));
}

// 自动补充的下面的测试围绕 `test_encode_loop_forced_error` 对应的契约或边界展开。
// 它会用构造好的样本输入来锁定与 Go 一致的可观测结果。
// 断言优先锁定输出、计数或错误文本，而不是实现细节。
// 这也是本次只加注释不改逻辑时最需要保护的契约部分。
/// TestEncodeLoopForcedError — custom ErrorParser → Err.
#[test]
fn test_encode_loop_forced_error() {
    let mut fx = setup_chunk_restore(Box::new(ErrorParser {
        msg: "forced encode error".into(),
    }));
    let err = fx
        .cr
        .encodeLoop(context::Background(), &fx.rc)
        .expect_err("forced error");
    assert!(
        err.Error().contains("forced encode error"),
        "got {}",
        err.Error()
    );
}

// 自动补充的下面的测试围绕 `test_encode_loop_deliver_limit` 对应的契约或边界展开。
// 它会用构造好的样本输入来锁定与 Go 一致的可观测结果。
// 断言优先锁定输出、计数或错误文本，而不是实现细节。
// 这也是本次只加注释不改逻辑时最需要保护的契约部分。
/// TestEncodeLoopDeliverLimit — CountingParser drains then process Ok.
#[test]
fn test_encode_loop_deliver_limit() {
    let mut fx = setup_chunk_restore(Box::new(CountingParser::new(3)));
    fx.cr
        .process(context::Background(), &fx.rc)
        .expect("process with limit rows");
    assert_eq!(fx.cr.chunk.Chunk.Offset, 3);
    assert_eq!(fx.cr.chunk.Chunk.PrevRowIDMax, 3);
    assert_eq!(fx.rc.saveCpCh.lock().unwrap().len(), 1);
    // Column filtering still works for deliver-limit scenario.
    let names = getColumnNames(&fx.tr.tableInfo.Core, &[0, 1, -1, -1]);
    assert_eq!(names, vec!["a".to_string(), "b".to_string()]);
}

// 自动补充的下面的测试围绕 `test_encode_loop_deliver_errored` 对应的契约或边界展开。
// 它会用构造好的样本输入来锁定与 Go 一致的可观测结果。
// 断言优先锁定输出、计数或错误文本，而不是实现细节。
// 这也是本次只加注释不改逻辑时最需要保护的契约部分。
/// TestEncodeLoopDeliverErrored — ErrorParser through process → Err.
#[test]
fn test_encode_loop_deliver_errored() {
    let mut fx = setup_chunk_restore(Box::new(ErrorParser {
        msg: "deliver encode boom".into(),
    }));
    let err = fx
        .cr
        .process(context::Background(), &fx.rc)
        .expect_err("process errored");
    assert!(err.Error().contains("deliver encode boom"));
}

// 自动补充的下面的测试围绕 `test_encode_loop_columns_mismatch` 对应的契约或边界展开。
// 它会用构造好的样本输入来锁定与 Go 一致的可观测结果。
// 断言优先锁定输出、计数或错误文本，而不是实现细节。
// 这也是本次只加注释不改逻辑时最需要保护的契约部分。
/// TestEncodeLoopColumnsMismatch — getColumnNames + process with EofParser.
#[test]
fn test_encode_loop_columns_mismatch() {
    let mut fx = setup_chunk_restore(Box::new(EofParser::default()));
    assert_eq!(
        getColumnNames(&fx.tr.tableInfo.Core, &[1, -1, 0, -1]),
        vec!["c".to_string(), "a".to_string()]
    );
    // Unknown header columns fail via parseColumnPermutations (Go mismatch path).
    let err = parseColumnPermutations(
        &fx.tr.tableInfo.Core,
        &["a".into(), "x".into()],
        &HashSet::new(),
        &log::Logger::L(),
    )
    .expect_err("unknown column");
    assert!(
        err.Error().contains("unknown columns") || err.class == Some("ErrUnknownColumns"),
        "got {}",
        err.Error()
    );
    fx.cr
        .process(context::Background(), &fx.rc)
        .expect("process after column check");
}

// 自动补充的下面的测试围绕 `test_encode_loop_ignore_columns` 对应的契约或边界展开。
// 它会用构造好的样本输入来锁定与 Go 一致的可观测结果。
// 断言优先锁定输出、计数或错误文本，而不是实现细节。
// 这也是本次只加注释不改逻辑时最需要保护的契约部分。
/// TestEncodeLoopIgnoreColumnsCSV — ignore via createColumnPermutation + process.
#[test]
fn test_encode_loop_ignore_columns() {
    let mut fx = setup_chunk_restore(Box::new(EofParser::default()));
    let ignore = HashSet::from(["b".to_string()]);
    let perm =
        createColumnPermutation(&[], &ignore, &fx.tr.tableInfo.Core, &log::Logger::L()).unwrap();
    assert_eq!(perm, vec![0, -1, 2, -1]);
    let names = getColumnNames(&fx.tr.tableInfo.Core, &perm);
    assert_eq!(names, vec!["a".to_string(), "c".to_string()]);
    fx.cr
        .process(context::Background(), &fx.rc)
        .expect("process ignore columns");
}

// 自动补充的下面的测试围绕 `test_restore` 对应的契约或边界展开。
// 它会用构造好的样本输入来锁定与 Go 一致的可观测结果。
// 断言优先锁定输出、计数或错误文本，而不是实现细节。
// 这也是本次只加注释不改逻辑时最需要保护的契约部分。
/// TestRestore — process() Ok with EofParser.
#[test]
fn test_restore() {
    let mut fx = setup_chunk_restore(Box::new(EofParser::default()));
    fx.cr
        .process(context::Background(), &fx.rc)
        .expect("restore process");
    fx.cr.close();
}

// 自动补充的下面的测试围绕 `test_compress_chunk_restore` 对应的契约或边界展开。
// 它会用构造好的样本输入来锁定与 Go 一致的可观测结果。
// 断言优先锁定输出、计数或错误文本，而不是实现细节。
// 这也是本次只加注释不改逻辑时最需要保护的契约部分。
/// TestCompressChunkRestore — compressed chunk FileMeta + process Ok.
#[test]
fn test_compress_chunk_restore() {
    let table = sample_table_info();
    let db = sample_db_info(&table);
    let meta = sample_table_meta();
    let tr = Arc::new(NewTableImporter(&db, &table, Some(meta), log::Logger::L()).unwrap());
    let chunk = checkpoints::ChunkCheckpoint {
        Key: checkpoints::ChunkCheckpointKey {
            Path: "db.table.99.csv.gz".into(),
            Offset: 0,
        },
        FileMeta: checkpoints::mydump::SourceFileMeta {
            Path: "db.table.99.csv.gz".into(),
            Type: checkpoints::mydump::SourceType(mydump::SourceTypeCSV),
            Compression: checkpoints::mydump::Compression(1),
            FileSize: 14,
            ..Default::default()
        },
        Chunk: checkpoints::mydump::Chunk {
            Offset: 0,
            EndOffset: 14,
            PrevRowIDMax: 0,
            RowIDMax: 2,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut cr = newChunkProcessor(
        Box::new(EofParser::default()),
        chunk.clone(),
        log::Logger::L(),
        Arc::clone(&tr),
    );
    let rc = minimal_controller(config::Config::NewConfig());
    cr.process(context::Background(), &rc)
        .expect("compress chunk restore");
    cr.close();

    use std::io::Write;
    let mut gzip = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    gzip.write_all(b"1,2,3\n4,5,6\n").unwrap();
    let store = storeapi::Storage::new("memory://compressed-chunk");
    store.Put(&chunk.FileMeta.Path, gzip.finish().unwrap());
    let mut parser = openParser(
        context::Background(),
        &config::Config::NewConfig(),
        &chunk,
        None,
        &store,
        &tr.tableInfo.Core,
    )
    .expect("open gzip CSV");
    parser.ReadRow().expect("read first compressed row");
    assert_eq!(parser.LastRow().Row.len(), 3);
}

// 自动补充的下面的测试围绕 `test_get_columns_names` 对应的契约或边界展开。
// 它会用构造好的样本输入来锁定与 Go 一致的可观测结果。
// 断言优先锁定输出、计数或错误文本，而不是实现细节。
// 这也是本次只加注释不改逻辑时最需要保护的契约部分。
/// TestGetColumnsNames — output follows source-field order encoded by the permutation.
#[test]
fn test_get_columns_names() {
    let table = sample_core();
    assert_eq!(getColumnNames(&table, &[0, 1, 2, -1]), vec!["a", "b", "c"]);
    assert_eq!(getColumnNames(&table, &[1, 0, 2, -1]), vec!["b", "a", "c"]);
    assert_eq!(getColumnNames(&table, &[-1, 0, 1, -1]), vec!["b", "c"]);
    assert_eq!(getColumnNames(&table, &[0, 1, -1, -1]), vec!["a", "b"]);
    assert_eq!(getColumnNames(&table, &[1, -1, 0, -1]), vec!["c", "a"]);
    assert_eq!(getColumnNames(&table, &[-1, 0, -1, -1]), vec!["b"]);
    assert_eq!(
        getColumnNames(&table, &[1, 2, 3, 0]),
        vec!["_tidb_rowid", "a", "b", "c"]
    );
    assert_eq!(
        getColumnNames(&table, &[1, 0, 2, 3]),
        vec!["b", "a", "c", "_tidb_rowid"]
    );
    assert_eq!(
        getColumnNames(&table, &[-1, 0, 2, 1]),
        vec!["b", "_tidb_rowid", "c"]
    );
    assert_eq!(
        getColumnNames(&table, &[2, -1, 0, 1]),
        vec!["c", "_tidb_rowid", "a"]
    );
    assert_eq!(
        getColumnNames(&table, &[-1, 1, -1, 0]),
        vec!["_tidb_rowid", "b"]
    );
}

#[test]
fn parquet_chunk_reads_real_rows_and_restores_checkpoint_across_groups() {
    use parquet::{
        data_type::Int64Type,
        file::{properties::WriterProperties, writer::SerializedFileWriter},
        schema::parser::parse_message_type,
    };
    let schema = Arc::new(parse_message_type("message schema { OPTIONAL INT64 v; }").unwrap());
    let mut writer = SerializedFileWriter::new(
        Vec::new(),
        schema,
        Arc::new(WriterProperties::builder().build()),
    )
    .unwrap();
    for base in [0, 32] {
        let mut group = writer.next_row_group().unwrap();
        let mut column = group.next_column().unwrap().unwrap();
        column
            .typed::<Int64Type>()
            .write_batch(&(base..base + 32).collect::<Vec<_>>(), Some(&[1; 32]), None)
            .unwrap();
        column.close().unwrap();
        group.close().unwrap();
    }
    let bytes = writer.into_inner().unwrap();
    let size = bytes.len() as i64;
    let store = storeapi::Storage::new("memory://parquet-chunk");
    store.Put("rows.parquet", bytes);
    for file_size in [0, size] {
        let mut chunk = sample_chunk();
        chunk.Key.Path = "rows.parquet".into();
        chunk.FileMeta.Path = "rows.parquet".into();
        chunk.FileMeta.Type = checkpoints::mydump::SourceType(mydump::SourceTypeParquet);
        chunk.FileMeta.FileSize = file_size;
        chunk.Chunk.Offset = 30;
        chunk.Chunk.PrevRowIDMax = 130;
        chunk.ColumnPermutation.clear();
        let mut parser = openParser(
            context::Background(),
            &config::Config::NewConfig(),
            &chunk,
            None,
            &store,
            &sample_core(),
        )
        .unwrap();
        for value in 30..64 {
            parser.ReadRow().unwrap();
            assert_eq!(parser.LastRow().RowID, value + 101);
            assert!(matches!(&parser.LastRow().Row[0],types::Datum::Int(v) if *v==value));
        }
        assert_eq!(parser.ReadRow().unwrap_err().class, Some("EOF"));
        parser.Close().unwrap();
    }
}
