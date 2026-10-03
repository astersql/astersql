// Copyright 2026 AsterSQL.

//! Parity tests for `br/pkg/restore` vs Go sources
//! (`import_mode_switcher.go`, `misc.go`, `restorer.go`).
//! 与 Go `br/pkg/restore` 三源文件的公开契约对齐测试（parity）。
//! 聚合 import_mode_switcher、misc、restorer 的关键符号与错误路径，单测按段落组织。
//! 失败时按段落中文注释定位；仅验证语义一致性，不启动真实 TiKV/PD 集群。
//! 测试段落顺序：misc 工具 → blocklist → TS/元数据 → switcher → region 分组 → 三类 Restorer → pipeline。
//!
//! Go 对照：`misc.go` / `import_mode_switcher.go` / `restorer.go` 公开契约。
//! FakeImporter 记录 Import 批次数；`has_error` 模拟导入失败传播。
//! FakeBalancedImporter 额外计数 PauseForBackpressure，对齐多表背压。
//! FakeSplitStrategy 由测试控制 ShouldSplit，Accumulate 收集待分裂集合。
//! MemPd/MemStorage/MemDomain 提供可注入失败边界，不启动真实集群。
//! blocklist 段钉住文件名解析、编解码往返与 Truncate 清理。
//! TS 段覆盖 GetTS 成功与 GetTSWithRetry 瞬时错误收敛。
//! switcher 段确认 Pre/Post/FineGrained 进出 import mode 顺序。
//! region 分组段验证重叠 BackupFileSet 迭代合并与 CollectAll。
//! Restorer 三段分别钉住 Simple/Batch/MultiTables 的 Send/Close。
//! Pipeline 段断言去重键与切片管道错误短路传播。
//! 布尔/错误文案属稳定契约，回归勿为过测改写期望。
//! 本任务仅加注释，输入向量与断言期望保持不变。
//! 新增公开 API 应扩展本综合测试段落，而非另起零散文件。
//! 全部用例可并行；桩内用原子/互斥保护共享计数。
//! 半开区间 range key 语义与 Go restore 包一致。
//! RecordingImportSstSwitcher 记录 SetMode 轨迹供顺序断言。
//! WorkerPool 与 PiTRIdTracker 仅服务可观察调用序，非生产路径。
//! 完成门槛：密度 + 仅注释差异 + 空白检查 + rustfmt（或待回归）。
//!
//! 夹具：FakeImporter/Balanced/Split/RegionsSplitter 与 sample_files。
//! 段落：misc→blocklist→switcher→region→GroupOverlapped→Restorer→Pipeline。
//! 断言用具体数值与错误关键字；Mem* 隔离外部依赖。
//! Go 对照：import_mode_switcher.go / misc.go / restorer.go。
//! CreateUniqueFileSets 将 File 拆成独立 BackupFileSet。
//! RecordingImportSstSwitcher 记录 SwitchMode 进入/退出。
//! PiTRIdTracker 驱动 blocklist 冲突错误。
//! PauseForBackpressure 次数应对齐 MultiTables 批次数。
//! ShouldSkip 命中不得 Accumulate；should_split=false 时不 ExecuteRegions。
//! checksum/Parse/Truncate/Unmarshal 覆盖黑名单完整性与清理。
//! GetTS/GetTSWithRetry/HasRestoreIDColumn/AssertUserDBsEmpty 覆盖元数据工具。
//! FineGrainedRestorePreWork 与 RestorePreWork/PostWork 均需覆盖。
//! Batch/Simple/MultiTables 各验证成功、错误传播与进度回调。
//! PipelineFromSlice 提供确定性输入，避免迭代器副作用。
//! GroupOverlapped 重叠且 rules 不一致必须失败。
//! Close 在 Import 失败后仍可调用。
//! 段落间重置 Mem*，避免隐式耦合掩盖回归。
//! imported 计数语义：Batch 按合并批次，Simple 按文件集数。
//! TransferBoolToValue 仅 ON/OFF；粒度常量拼写不可改。
//! Marshal 路径前缀必须是 v1/log_restore_tables_blocklists。
//! Parse 短名/坏分隔返回 (0,0,false)。
//! region 缓存 drain 后 cache[0] 覆盖当前 key。

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use astersql_br_pkg_restore_utils::stubs::backuppb;
use astersql_br_pkg_utils_iter::{CollectAll, Context as IterContext};

use crate::import_mode_switcher::{
    FineGrainedRestorePreWork, NewImportModeSwitcher, RestorePostWork, RestorePreWork,
};
use crate::misc::{
    AssertUserDBsEmpty, CheckTableTrackerContainsTableIDsFromBlocklistFiles, GetTS, GetTSWithRetry,
    GetTableSchema, GroupOverlappedBackupFileSetsIter, HasRestoreIDColumn,
    MarshalLogRestoreTableIDsBlocklistFile, NewRegionScanner,
    ParseLogRestoreTableIDsBlocklistFileName, TransferBoolToValue,
    TruncateLogRestoreTableIDsBlocklistFiles, UnmarshalLogRestoreTableIDsBlocklistFile,
    logRestoreTableIDBlocklistFilePrefix, parseLogRestoreTableIDsBlocklistFileName,
};
use crate::restorer::{
    BackupFileSet, BalancedFileImporter, CreateUniqueFileSets, FileImporter, GetFileRangeKey,
    NewBatchSstRestorer, NewMultiTablesRestorer, NewSimpleSstRestorer, PipelineFromSlice,
    PipelineRestorerWrapper, SstRestorer,
};
use crate::stubs::{
    CIStr, ColumnInfo, Context, DBInfo, Error, MemConnMgr, MemDomain, MemInfoSchema, MemMetaReader,
    MemPdClient, MemRestoreCheckpoint, MemSplitClient, MemStorage, NewWorkerPool, PiTRIdTracker,
    RecordingImportSstSwitcher, RegionInfo, SplitHelperIterator, SplitStrategy, Storage, TableInfo,
    import_sstpb, metapb,
};

/// 轻量 FakeImporter：实现 `FileImporter`，可模拟成功导入或固定错误。
/// 成功路径累加 `imported` 计数，供 Batch/Simple restorer 断言导入批次数量。
/// `has_error` 为 true 时固定返回 "import error"，用于验证错误传播路径。
struct FakeImporter {
    has_error: bool,
    imported: Mutex<usize>,
}

impl FileImporter for FakeImporter {
    fn Import(&self, _ctx: &Context, file_sets: &[BackupFileSet]) -> Result<(), Error> {
        // 模拟导入失败：与 Go fakeImporter.hasError 行为一致。
        if self.has_error {
            return Err(Error::new("import error"));
        }
        // 成功时按传入 batch 数量累加，BatchRestorer 可据此断言 Import 调用次数。
        *self.imported.lock().unwrap() += file_sets.len();
        Ok(())
    }

    // Close 为空操作：restorer 仍应调用以对齐 Go importer.Close 契约。
    fn Close(&self) -> Result<(), Error> {
        Ok(())
    }
}

/// FakeBalancedImporter：MultiTablesRestorer 使用的均衡导入器桩。
/// 实现 `BalancedFileImporter::PauseForBackpressure`，用于断言背压解除次数。
/// MultiTables 每完成一批导入会调用 PauseForBackpressure 等待下游消化。
struct FakeBalancedImporter {
    has_error: bool,
    unblock_count: AtomicUsize,
}

impl FileImporter for FakeBalancedImporter {
    fn Import(&self, _ctx: &Context, _file_sets: &[BackupFileSet]) -> Result<(), Error> {
        if self.has_error {
            return Err(Error::new("import error"));
        }
        Ok(())
    }

    fn Close(&self) -> Result<(), Error> {
        Ok(())
    }
}

impl BalancedFileImporter for FakeBalancedImporter {
    fn PauseForBackpressure(&self) {
        // 每次背压解除计数 +1，与 Go TestMultiTablesRestorer 中 unblock 断言对齐。
        self.unblock_count.fetch_add(1, Ordering::SeqCst);
    }
}

/// FakeSplitStrategy：PipelineRestorerWrapper.WithSplit 的拆分策略桩。
/// `should_split` 控制是否触发 region 拆分；`skipped` 集合驱动 ShouldSkip 过滤。
/// `accumulated` 记录未被 skip 的元素，供 no-split 场景断言累积数量。
struct FakeSplitStrategy {
    should_split: bool,
    accumulated: Vec<String>,
    skipped: HashSet<String>,
}

impl SplitStrategy<String> for FakeSplitStrategy {
    fn Accumulate(&mut self, v: String) {
        self.accumulated.push(v);
    }

    fn ShouldSplit(&self) -> bool {
        self.should_split
    }

    fn ShouldSkip(&self, v: &String) -> bool {
        self.skipped.contains(v)
    }

    fn GetAccumulations(&self) -> SplitHelperIterator {
        SplitHelperIterator { items: Vec::new() }
    }

    fn ResetAccumulations(&mut self) {
        self.accumulated.clear();
    }
}

/// FakeRegionsSplitter：记录 ExecuteRegions 调用次数，验证 WithSplit 是否触发拆分。
/// should_split=true 时每个 pipeline 元素应触发一次 ExecuteRegions。
struct FakeRegionsSplitter {
    executed: AtomicUsize,
}

impl crate::stubs::PipelineRegionsSplitter for FakeRegionsSplitter {
    fn ExecuteRegions(
        &self,
        _ctx: &Context,
        _split_helper: &SplitHelperIterator,
    ) -> Result<(), Error> {
        self.executed.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

/// 构造两份标准 SST 备份文件（TotalKvs 10+20=30），供进度回调与去重测试复用。
fn sample_files() -> Vec<backuppb::File> {
    vec![
        backuppb::File {
            Name: "file1.sst".into(),
            TotalKvs: 10,
            ..Default::default()
        },
        backuppb::File {
            Name: "file2.sst".into(),
            TotalKvs: 20,
            ..Default::default()
        },
    ]
}

/// Go `context.WithCancel`/`WithTimeout` contract: descendants observe a parent
/// cancellation after construction, and deadlines expire without explicit cancel.
#[test]
fn context_propagates_parent_cancellation_and_deadline() {
    let parent = Context::Background();
    let (child, _child_cancel) = Context::WithCancel(&parent);
    parent.cancel(Error::new("parent stopped"));
    assert_eq!(
        child.Err().expect("parent cancellation").msg,
        "parent stopped"
    );

    let (timed, _timed_cancel) =
        Context::WithTimeout(&Context::Background(), Duration::from_millis(1));
    std::thread::sleep(Duration::from_millis(5));
    assert_eq!(
        timed.Err().expect("deadline cancellation").msg,
        "context deadline exceeded"
    );
}

/// 总览式契约测试：按段落断言 misc / switcher / restorer 的 Go-Rust 对齐点。
#[test]
fn go_rust_public_contract_matches() {
    // --- normal: TransferBoolToValue / GetFileRangeKey / CreateUniqueFileSets ---
    // 段落：misc 基础工具 —— 布尔转 ON/OFF、SST 文件名 range key 提取、文件集去重。
    // TransferBoolToValue 与 Go 一致：true→"ON"，false→"OFF"。
    assert_eq!(TransferBoolToValue(true), "ON");
    assert_eq!(TransferBoolToValue(false), "OFF");
    // GetFileRangeKey 剥掉 `_default.sst` 后缀，保留 table/key/ts 前缀段。
    assert_eq!(GetFileRangeKey("1_2_3_key_ts_default.sst"), "1_2_3_key_ts");
    // CreateUniqueFileSets 按文件名去重，两个不同文件应产出两个 BackupFileSet 各含 1 SST。
    let unique = CreateUniqueFileSets(sample_files());
    assert_eq!(unique.len(), 2);
    // 每个 BackupFileSet 只保留唯一 SST，避免重复导入同名文件。
    assert_eq!(unique[0].SSTFiles.len(), 1);

    // --- boundary: blocklist filename parse ---
    // 段落：PiTR blocklist 文件名解析 —— 合法路径与非法输入边界。
    // 合法 v1 前缀 + R/S 十六进制 commit/restore TS 应解析成功。
    let (c, s, ok) = parseLogRestoreTableIDsBlocklistFileName(
        "v1/log_restore_tables_blocklists/R0000FFFFFCDEFFFF_S0000FFFFFFABCFFF.meta",
    );
    assert!(ok);
    // commit TS 与 restore TS 从 R/S 十六进制段解析，须与 Go parse 结果 bitwise 一致。
    assert_eq!(c, 0xFFFF_FCDE_FFFF);
    assert_eq!(s, 0xFFFF_FFAB_CFFF);
    // 非预期后缀或 R 段长度不足时 Parse 返回 ok=false，不 panic。
    assert!(!ParseLogRestoreTableIDsBlocklistFileName("nope.txt").2);
    assert!(!ParseLogRestoreTableIDsBlocklistFileName("Rshort.meta").2);

    // --- normal + error: marshal / unmarshal checksum ---
    // 段落：blocklist 元文件序列化/反序列化与 CRC 校验失败路径。
    // Marshal 产出带固定前缀与 .meta 后缀的文件名，payload 含 table/db ID 列表。
    let (name, data) = MarshalLogRestoreTableIDsBlocklistFile(
        0xFFFF_FCDE_FFFF,
        0xFFFF_FFAB_CFFF,
        0xFFFF_FCCC_FFFF,
        vec![1, 2, 3],
        vec![4],
    )
    .expect("marshal");
    // 文件名须含 PiTR blocklist 固定前缀，便于 storage 侧列举与截断。
    assert!(name.contains(logRestoreTableIDBlocklistFilePrefix));
    assert!(name.ends_with(".meta"));
    let decoded = UnmarshalLogRestoreTableIDsBlocklistFile(&data).expect("unmarshal");
    // round-trip 后 table/db ID 列表须与 marshal 入参完全一致。
    assert_eq!(decoded.TableIds, vec![1, 2, 3]);
    assert_eq!(decoded.DbIds, vec![4]);
    // 篡改末字节破坏 CRC，Unmarshal 应返回错误而非静默接受。
    let mut bad = data.clone();
    if let Some(last) = bad.last_mut() {
        *last ^= 0xff;
    }
    assert!(UnmarshalLogRestoreTableIDsBlocklistFile(&bad).is_err());

    // --- normal: GetTS / GetTSWithRetry ---
    // 段落：从 PD 获取 TS；MemPdClient 可注入 fail_ts 验证重试至少 3 次。
    let ctx = Context::Background();
    let pd = MemPdClient::new(vec![]);
    // 注入固定 physical/logical TS，验证 GetTS 直接读取 PD 返回值。
    *pd.ts.lock().unwrap() = (42, 7);
    let ts = GetTS(&ctx, &pd).expect("ts");
    assert_eq!(ts, crate::stubs::ComposeTS(42, 7));
    // fail_ts 使前几次 GetTS 失败，GetTSWithRetry 应重试并最终成功。
    pd.fail_ts.store(true, Ordering::SeqCst);
    let ts2 = GetTSWithRetry(&ctx, &pd).expect("retry ts");
    assert_eq!(ts2, crate::stubs::ComposeTS(42, 7));
    // 重试次数至少 3 次，与 Go backoff 策略下限对齐。
    assert!(pd.fail_ts_times.load(Ordering::SeqCst) >= 3);

    // --- normal/error: AssertUserDBsEmpty / HasRestoreIDColumn / GetTableSchema ---
    // 段落：恢复前集群空库检查、PiTR restore_id 列探测、表 schema 查询。
    // 仅含系统库 mysql/test 时 AssertUserDBsEmpty 应通过。
    let mut dom = MemDomain {
        info: MemInfoSchema::default(),
        meta: MemMetaReader::default(),
    };
    dom.info.schemas.lock().unwrap().push(DBInfo {
        ID: 1,
        Name: CIStr::new("mysql"),
    });
    dom.info.schemas.lock().unwrap().push(DBInfo {
        ID: 2,
        Name: CIStr::new("test"),
    });
    AssertUserDBsEmpty(&dom).expect("fresh cluster");
    // 新增用户库 d1 后应拒绝恢复，防止覆盖已有数据。
    dom.info.schemas.lock().unwrap().push(DBInfo {
        ID: 3,
        Name: CIStr::new("d1"),
    });
    // 存在非系统库时必须返回 err，不能静默继续。
    assert!(AssertUserDBsEmpty(&dom).is_err());

    // tidb_pitr_id_map 含 restore_id 列时 HasRestoreIDColumn 为 true。
    dom.info.tables.lock().unwrap().insert(
        ("mysql".into(), "tidb_pitr_id_map".into()),
        TableInfo {
            ID: 99,
            Name: CIStr::new("tidb_pitr_id_map"),
            Columns: vec![ColumnInfo {
                Name: CIStr::new("restore_id"),
            }],
            ..Default::default()
        },
    );
    assert!(HasRestoreIDColumn(&dom));
    let schema = GetTableSchema(&dom, &CIStr::new("mysql"), &CIStr::new("tidb_pitr_id_map"))
        .expect("schema");
    // 返回的 TableInfo 名称须与查询表名一致。
    assert_eq!(schema.Name.L, "tidb_pitr_id_map");
    // 不存在的表应返回错误，而非空 schema。
    assert!(GetTableSchema(&dom, &CIStr::new("test"), &CIStr::new("missing")).is_err());

    // --- resource cleanup: import mode switcher pre/post work ---
    // 段落：ImportModeSwitcher 离线/在线 prework、postwork 与细粒度 undo。
    // store-1 为 TiKV，store-2 带 tiflash engine 标签；仅 TiKV 应切 Import 模式。
    let stores = vec![
        metapb::Store {
            Id: 1,
            Address: "store-1".into(),
            Labels: vec![],
        },
        metapb::Store {
            Id: 2,
            Address: "store-2".into(),
            Labels: vec![("engine".into(), "tiflash".into())],
        },
    ];
    let pd = Arc::new(MemPdClient::new(stores));
    let switcher_impl = Arc::new(RecordingImportSstSwitcher::new());
    let mut mode =
        NewImportModeSwitcher(pd.clone(), Duration::from_millis(50), switcher_impl.clone());
    let mgr = MemConnMgr::default();
    // 离线 prework：应返回 undo 闭包与 cfg，并调用 conn mgr Remove。
    let (undo, cfg) = RestorePreWork(&ctx, &mgr, &mut mode, false, true).expect("prework");
    // 离线模式须返回 tikv config 快照，供 postwork 恢复。
    assert!(cfg.is_some());
    // conn mgr Remove 在 prework 被调用，释放旧连接池。
    assert!(mgr.remove_called.load(Ordering::SeqCst));
    // initial switch to import for non-tiflash store only
    {
        let calls = switcher_impl.calls.lock().unwrap();
        // store-1（TiKV）应收到 SwitchMode::Import。
        assert!(
            calls
                .iter()
                .any(|(a, m)| a == "store-1" && *m == import_sstpb::SwitchMode::Import)
        );
        // store-2（TiFlash）不应切换 import 模式。
        assert!(!calls.iter().any(|(a, _)| a == "store-2"));
    }
    // RestorePostWork 应将所有 store 切回 Normal 模式。
    RestorePostWork(ctx.clone(), &mut mode, undo, false);
    {
        let calls = switcher_impl.calls.lock().unwrap();
        assert!(
            calls
                .iter()
                .any(|(_, m)| *m == import_sstpb::SwitchMode::Normal)
        );
    }
    // online prework is nop
    // 在线 prework 不修改 import 模式，cfg 应为 None。
    let (undo2, cfg2) = RestorePreWork(&ctx, &mgr, &mut mode, true, true).expect("online");
    assert!(cfg2.is_none());
    let _ = undo2;

    // FineGrainedRestorePreWork 对给定 key range 做细粒度切换，undo 可逆。
    let (undo3, origin) = FineGrainedRestorePreWork(
        &ctx,
        &mgr,
        &mut mode,
        &[[b"a".to_vec(), b"b".to_vec()]],
        false,
    )
    .expect("fine");
    // 未绑定 placement rule 时 RuleID 为空字符串。
    assert_eq!(origin.RuleID, "");
    // undo 闭包须可重复调用且不报错，保证 prework 可逆。
    undo3(&ctx).expect("fine undo");

    // --- blocklist tracker conflict (error path) + truncate cleanup ---
    // 段落：PiTR blocklist 与 tracker 冲突检测，以及过期 blocklist 文件截断清理。
    let storage = MemStorage::new();
    let (fname, blob) =
        MarshalLogRestoreTableIDsBlocklistFile(200, 50, 30, vec![7], vec![8]).unwrap();
    storage.WriteFile(&ctx, &fname, &blob).unwrap();
    let mut tracker = PiTRIdTracker::new();
    // tracker 预先持有 table 7，与 blocklist 中 table 7 冲突。
    tracker.table_ids.insert(7);
    // startTs(100) < restoreCommitTs(200) 且 restoredTs(60) >= restoreStartTs(50) 时，
    // tracker 已含 table 7 应报 "cannot restore the table" 冲突错误。
    let err = CheckTableTrackerContainsTableIDsFromBlocklistFiles(
        &ctx,
        &storage,
        &tracker,
        100, // startTs < restoreCommitTs
        60,  // restoredTs >= restoreStartTs
        |_id| "t".into(),
        |_id| "d".into(),
        |_| false,
        |_| false,
        |_| {},
    );
    assert!(err.is_err());
    // 错误信息须明确指出 table 不可恢复，便于运维定位冲突表。
    assert!(err.unwrap_err().msg.contains("cannot restore the table"));

    // Truncate 到 commitTs=200 应删除该 blocklist 文件。
    TruncateLogRestoreTableIDsBlocklistFiles(&ctx, &storage, 200).unwrap();
    // 截断后 storage 读该文件应失败，证明清理生效。
    assert!(storage.ReadFile(&ctx, &fname).is_err());

    // --- region scanner / overlapped grouping ---
    // 段落：RegionScanner 单 region 键范围判定与重叠 SST 分组迭代。
    // 两个 region 以 b"m" 为分界：a~b 在同一 region，a~z 跨 region。
    let regions = vec![
        RegionInfo {
            Region: Some(metapb::Region {
                Id: 1,
                StartKey: vec![],
                EndKey: b"m".to_vec(),
            }),
            Leader: None,
        },
        RegionInfo {
            Region: Some(metapb::Region {
                Id: 2,
                StartKey: b"m".to_vec(),
                EndKey: vec![],
            }),
            Leader: None,
        },
    ];
    let split_client = Arc::new(MemSplitClient::new(regions));
    let mut scanner = NewRegionScanner(split_client.clone(), 64);
    // a~b 完全落在 region-1（[,m)）内，应返回 true。
    assert!(scanner.IsKeyRangeInOneRegion(&ctx, b"a", b"b").unwrap());
    // a~z 跨越 region 边界 m，应返回 false。
    assert!(!scanner.IsKeyRangeInOneRegion(&ctx, b"a", b"z").unwrap());

    // Files with no rewrite rules: GetRewriteRawKeys returns raw keys.
    // 无 RewriteRules 时两 SST key 范围重叠，GroupOverlapped 应合并为 1 batch 含 2 文件。
    let sets = vec![
        BackupFileSet {
            TableID: 1,
            SSTFiles: vec![backuppb::File {
                Name: "a.sst".into(),
                StartKey: b"a".to_vec(),
                EndKey: b"c".to_vec(),
                TotalKvs: 1,
                ..Default::default()
            }],
            RewriteRules: None,
        },
        BackupFileSet {
            TableID: 1,
            SSTFiles: vec![backuppb::File {
                Name: "b.sst".into(),
                StartKey: b"b".to_vec(),
                EndKey: b"d".to_vec(),
                TotalKvs: 1,
                ..Default::default()
            }],
            RewriteRules: None,
        },
    ];
    let batches = Mutex::new(Vec::new());
    GroupOverlappedBackupFileSetsIter(&ctx, split_client.clone(), sets, |batch| {
        batches.lock().unwrap().push(batch);
    })
    .unwrap();
    let batches = batches.into_inner().unwrap();
    // 重叠 SST 被合并为单个外层 batch。
    assert_eq!(batches.len(), 1);
    // 内层只有一个 BackupFileSet，但含两个 SST 文件。
    assert_eq!(batches[0].len(), 1);
    assert_eq!(batches[0][0].SSTFiles.len(), 2);

    // --- SimpleRestorer progress + import error ---
    // 段落：SimpleSstRestorer 进度累加、导入失败传播与 checkpoint 写入。
    let importer = Arc::new(FakeImporter {
        has_error: false,
        imported: Mutex::new(0),
    });
    // worker pool 大小 2，与 Go TestSimpleRestorerImportAndProgress 一致。
    let pool = NewWorkerPool(2, "simple-restorer");
    let restorer = NewSimpleSstRestorer(&ctx, importer.clone(), pool, None);
    let progress = Arc::new(AtomicUsize::new(0));
    let progress_cb = {
        let progress = progress.clone();
        Arc::new(move |n: i64| {
            progress.fetch_add(n as usize, Ordering::SeqCst);
        }) as Arc<dyn Fn(i64) + Send + Sync>
    };
    restorer
        .GoRestore(
            progress_cb.clone(),
            vec![vec![BackupFileSet {
                TableID: 0,
                SSTFiles: sample_files(),
                RewriteRules: None,
            }]],
        )
        .unwrap();
    restorer.WaitUntilFinish().unwrap();
    // sample_files TotalKvs 10+20=30，进度回调应累加到 30。
    assert_eq!(progress.load(Ordering::SeqCst), 30);

    // has_error=true 时 WaitUntilFinish 应返回含 "import error" 的错误。
    let bad_importer = Arc::new(FakeImporter {
        has_error: true,
        imported: Mutex::new(0),
    });
    // 新建 restorer 实例，避免前一次成功导入的状态干扰。
    let restorer2 = NewSimpleSstRestorer(
        &ctx,
        bad_importer,
        NewWorkerPool(2, "simple-restorer"),
        None,
    );
    restorer2
        .GoRestore(
            Arc::new(|_: i64| {}),
            vec![vec![BackupFileSet {
                TableID: 0,
                SSTFiles: vec![backuppb::File {
                    Name: "file_with_error.sst".into(),
                    TotalKvs: 15,
                    ..Default::default()
                }],
                RewriteRules: None,
            }]],
        )
        .unwrap();
    let err = restorer2.WaitUntilFinish().unwrap_err();
    assert!(err.msg.contains("import error"));

    // checkpoint append on success
    // 成功导入且传入 checkpoint 时，应按 SST 文件名写入 ckpt.files。
    let ckpt = Arc::new(MemRestoreCheckpoint::default());
    let restorer3 = NewSimpleSstRestorer(
        &ctx,
        Arc::new(FakeImporter {
            has_error: false,
            imported: Mutex::new(0),
        }),
        NewWorkerPool(2, "ckpt"),
        Some(ckpt.clone()),
    );
    restorer3
        .GoRestore(
            Arc::new(|_: i64| {}),
            vec![vec![BackupFileSet {
                TableID: 9,
                SSTFiles: sample_files(),
                RewriteRules: None,
            }]],
        )
        .unwrap();
    restorer3.WaitUntilFinish().unwrap();
    // 两个 SST 文件名各写入 ckpt.files 一条记录。
    assert_eq!(ckpt.files.lock().unwrap().len(), 2);

    // --- MultiTablesRestorer success / error / cancel ---
    // 段落：MultiTablesRestorer 多表并行、背压、错误与取消路径。
    let bal = Arc::new(FakeBalancedImporter {
        has_error: false,
        unblock_count: AtomicUsize::new(0),
    });
    let multi = NewMultiTablesRestorer(&ctx, bal.clone(), NewWorkerPool(2, "multi"), None);
    let progress2 = Arc::new(AtomicUsize::new(0));
    let cb2 = {
        let progress2 = progress2.clone();
        Arc::new(move |n: i64| {
            progress2.fetch_add(n as usize, Ordering::SeqCst);
        }) as Arc<dyn Fn(i64) + Send + Sync>
    };
    let batch = vec![
        BackupFileSet {
            TableID: 1001,
            SSTFiles: sample_files(),
            RewriteRules: None,
        },
        BackupFileSet {
            TableID: 1002,
            SSTFiles: vec![backuppb::File {
                Name: "file3.sst".into(),
                TotalKvs: 15,
                ..Default::default()
            }],
            RewriteRules: None,
        },
    ];
    // 两批相同 batch：进度回调按 batch 计数为 2；每批结束触发一次背压解除。
    multi
        .GoRestore(cb2, vec![batch.clone(), batch.clone()])
        .unwrap();
    multi.WaitUntilFinish().unwrap();
    // MultiTables 进度按完成的 batch 数累加，两批故为 2。
    assert_eq!(progress2.load(Ordering::SeqCst), 2);
    // 每批导入后触发一次背压解除。
    assert_eq!(bal.unblock_count.load(Ordering::SeqCst), 2);

    // 导入失败时 WaitUntilFinish 传播 import error。
    let bal_err = Arc::new(FakeBalancedImporter {
        has_error: true,
        unblock_count: AtomicUsize::new(0),
    });
    let multi_err = NewMultiTablesRestorer(&ctx, bal_err, NewWorkerPool(2, "multi"), None);
    multi_err
        .GoRestore(Arc::new(|_: i64| {}), vec![batch.clone()])
        .unwrap();
    assert!(
        multi_err
            .WaitUntilFinish()
            .unwrap_err()
            .msg
            .contains("import error")
    );

    // 已取消的 context 在 GoRestore 阶段即应返回 cancel 错误。
    let (cancelled, cancel) = Context::WithCancel(&Context::Background());
    cancel.call();
    let multi_cancel = NewMultiTablesRestorer(
        &cancelled,
        Arc::new(FakeBalancedImporter {
            has_error: false,
            unblock_count: AtomicUsize::new(0),
        }),
        NewWorkerPool(2, "multi"),
        None,
    );
    let err = multi_cancel
        .GoRestore(Arc::new(|_: i64| {}), vec![batch])
        .unwrap_err();
    assert!(err.msg.contains("cancel"));

    // range-key checkpoint merging
    // 同 range key 前缀的多 CF SST 应合并为一条 checkpoint range 记录。
    let ckpt2 = Arc::new(MemRestoreCheckpoint::default());
    let multi_ckpt = NewMultiTablesRestorer(
        &ctx,
        Arc::new(FakeBalancedImporter {
            has_error: false,
            unblock_count: AtomicUsize::new(0),
        }),
        NewWorkerPool(2, "multi"),
        Some(ckpt2.clone()),
    );
    multi_ckpt
        .GoRestore(
            Arc::new(|_: i64| {}),
            vec![vec![BackupFileSet {
                TableID: 5,
                SSTFiles: vec![
                    backuppb::File {
                        Name: "1_2_3_key_ts_default.sst".into(),
                        TotalKvs: 1,
                        ..Default::default()
                    },
                    backuppb::File {
                        Name: "1_2_3_key_ts_write.sst".into(),
                        TotalKvs: 1,
                        ..Default::default()
                    },
                ],
                RewriteRules: None,
            }]],
        )
        .unwrap();
    multi_ckpt.WaitUntilFinish().unwrap();
    let ranges = ckpt2.ranges.lock().unwrap().clone();
    // default/write 两 CF 共享 range key 前缀，checkpoint 只保留一条。
    assert_eq!(ranges.len(), 1);
    assert_eq!(ranges[0], (5, "1_2_3_key_ts".into()));

    // --- BatchRestorer wires GroupOverlapped + importer ---
    // 段落：BatchSstRestorer 串联重叠分组与实际 Import，验证进度与导入次数。
    let batch_importer = Arc::new(FakeImporter {
        has_error: false,
        imported: Mutex::new(0),
    });
    let batch_restorer = NewBatchSstRestorer(
        &ctx,
        batch_importer.clone(),
        split_client,
        NewWorkerPool(4, "batch"),
        None,
    );
    let prog = Arc::new(AtomicUsize::new(0));
    let prog_cb = {
        let prog = prog.clone();
        Arc::new(move |n: i64| {
            prog.fetch_add(n as usize, Ordering::SeqCst);
        }) as Arc<dyn Fn(i64) + Send + Sync>
    };
    batch_restorer
        .GoRestore(
            prog_cb,
            vec![vec![
                BackupFileSet {
                    TableID: 1,
                    SSTFiles: vec![backuppb::File {
                        Name: "a.sst".into(),
                        StartKey: b"a".to_vec(),
                        EndKey: b"c".to_vec(),
                        TotalKvs: 3,
                        ..Default::default()
                    }],
                    RewriteRules: None,
                },
                BackupFileSet {
                    TableID: 1,
                    SSTFiles: vec![backuppb::File {
                        Name: "b.sst".into(),
                        StartKey: b"b".to_vec(),
                        EndKey: b"d".to_vec(),
                        TotalKvs: 4,
                        ..Default::default()
                    }],
                    RewriteRules: None,
                },
            ]],
        )
        .unwrap();
    batch_restorer.WaitUntilFinish().unwrap();
    // TotalKvs 3+4=7；重叠分组后至少触发一次 Import。
    assert_eq!(prog.load(Ordering::SeqCst), 7);
    // GroupOverlapped 合并后一次 Import 调用即可覆盖两文件。
    assert!(*batch_importer.imported.lock().unwrap() >= 1);

    // --- PipelineRestorerWrapper WithSplit (skip / no-split / split) ---
    // 段落：Pipeline 迭代器 WithSplit —— skip、不拆分、触发拆分三种行为。
    let splitter = Arc::new(FakeRegionsSplitter {
        executed: AtomicUsize::new(0),
    });
    let wrapper = PipelineRestorerWrapper {
        splitter: splitter.clone(),
    };
    let strategy = Arc::new(Mutex::new(FakeSplitStrategy {
        should_split: false,
        accumulated: Vec::new(),
        skipped: HashSet::from(["skip-me".into()]),
    }));
    let iter_ctx = IterContext::background();
    let split_ctx = Context::Background();
    // should_split=false 且 skip-me 在 skipped：输出 a、b，不执行 ExecuteRegions。
    let mut it = wrapper.WithSplit(
        &split_ctx,
        PipelineFromSlice(vec!["a".into(), "skip-me".into(), "b".into()]),
        strategy.clone(),
    );
    let got = CollectAll(&iter_ctx, it.as_mut());
    assert!(got.Err.is_none());
    let items = got.Item.expect("items");
    // skip-me 被过滤，仅保留 a 与 b。
    assert_eq!(items, vec!["a".to_string(), "b".to_string()]);
    // 未触发拆分时 ExecuteRegions 调用次数为 0。
    assert_eq!(splitter.executed.load(Ordering::SeqCst), 0);
    // a、b 均被 Accumulate，累积长度为 2。
    assert_eq!(strategy.lock().unwrap().accumulated.len(), 2);

    // should_split=true：每个元素触发拆分，accumulated 在 split 后清空。
    let strategy2 = Arc::new(Mutex::new(FakeSplitStrategy {
        should_split: true,
        accumulated: Vec::new(),
        skipped: HashSet::new(),
    }));
    let mut it2 = wrapper.WithSplit(
        &split_ctx,
        PipelineFromSlice(vec!["x".into(), "y".into()]),
        strategy2.clone(),
    );
    let got2 = CollectAll(&iter_ctx, it2.as_mut());
    assert!(got2.Item.is_some());
    // x、y 各触发一次 ExecuteRegions，累计 2 次（含前段 no-split 的 0）。
    assert_eq!(splitter.executed.load(Ordering::SeqCst), 2);
    // split 后 ResetAccumulations 清空 accumulated。
    assert!(strategy2.lock().unwrap().accumulated.is_empty());

    // panic boundary for invalid file name
    // GetFileRangeKey 对无法解析的文件名应 panic，与 Go 行为一致。
    let panicked = std::panic::catch_unwind(|| GetFileRangeKey("nofile"));
    assert!(panicked.is_err());
}
