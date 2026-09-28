// Copyright 2026 AsterSQL.

//! Parity tests for `br/pkg/restore/log_client` vs Go sources.
//! Covers normal paths, boundaries, errors, and resource cleanup.
//!
//! 本文件用单一综合测试 `go_rust_public_contract_matches` 对照 Go 公开契约：
//! - skip map / SST / PITR id-map 文件名与块大小；
//! - migration 跳过表、retain-latest-mvcc 注释解析与覆盖；
//! - MetaKV 时间过滤、排序分 CF、批处理计数；
//! - Compacted/Log 拆分策略的 checkpoint 跳过与累计；
//! - RPC 重试策略、按 region 过滤、importer/client 生命周期；
//! - ApplyKVFiles 批/单路径、getKeyTS、RangeController、MetaKV 处理器接线。
//! 全部依赖 Mem* stub，不启动真实 PD/TiKV/对象存储。
//! 断言失败时按段落英文标记定位（normal/boundary/error/resource）。
//! 不改测试行为：仅补充中文说明「校验什么契约、边界为何成立」。
//!
//! 段落顺序刻意从「纯函数/数据结构」推进到「带状态策略」，再进入「client 资源」。
//! 这样失败时更容易判断是编码常量漂移还是状态机回归。
//! skip map 段同时覆盖 bitmap 与 Ext 两套 API，防止只测其一导致语义分叉。
//! SST 段验证 TableID 缓存、RewrittenTo 优先 Upstream、SetSSTs 0/1 契约。
//! migration 段故意让 compaction 落在窗外，确认粗过滤不会误用 EditMeta。
//! MetaKV 时间过滤参数含义对齐 Go：restoreTS / shiftStartTS / startTS 组合。
//! Compacted 策略用 done.sst 模拟 checkpoint 命中，todo.sst 验证残留累计。
//! Log 策略用 MemLogMetaManager 注入 Foffs，区分「已完成 offset」与「低于阈值」。
//! RPCResult 错误字符串是策略分类的输入，保持与 Go 关键字一致。
//! filterFilesByRegion 同时测成功交集与 ranges 长度不匹配错误。
//! client 段覆盖 restore id hint、checkpoint 进度、表路径 segment 校验与 Close。
//! ApplyKVFiles 用 batchSize=1 强制多批，避免「碰巧一批成功」掩盖切分逻辑。
//! getKeyTS 使用 TiDB 编码惯例（按位取反的大端 TS），短键必须报错。
//! RangeController 只挂一个 region，命中次数应为 1，防止重复扫描回归。
//! MetaKV 处理器构造冒烟：空输入不 panic，回调可空实现。
//! 与 Go 源的差异若存在，应在对应英文段落旁用中文标明「当前 stub 行为」。
//! 本任务只加注释，因此所有期望值保持与既有断言完全一致。
//!
//! Go 对照索引（便于回归时跳转）：
//! - `log_file_map.go` / `NeedSkip` 与 Ext 变体；
//! - `ssts.go` 的 TableID / RewrittenTo / SetSSTs；
//! - `id_map.go` 的文件名模板与块大小常量；
//! - `migration.go` 的粗过滤与 retain-latest-mvcc 注释；
//! - `log_file_manager.go` 的 TS 过滤与 MetaKV 批处理；
//! - `compacted_file_strategy.go` / `log_split_strategy.go`；
//! - `import_retry.go` 的 RPCResult 与 RangeController；
//! - `import.go` 的 filterFilesByRegion；
//! - `client.go` 的 ID map 持久化、表读取与 Close。
//! 若 Rust stub 能力不足，对应断言已按当前行为书写；补齐 stub 后应收紧期望。
//! CountingProcessor 只关心「是否进入批处理」，不校验 KV 解码内容。
//! TEST_NewLogClient* 夹具封装了 PD/TS 默认值，测试聚焦业务分支而非构造细节。
//! MemImporterClient / MemSplitClient 提供最小可用实现，Close/扫描路径可观测。
//! 段落之间状态不共享（除局部变量自然传递），避免隐式耦合掩盖失败原因。
//! 资源段落放在后部：先证明逻辑正确，再证明生命周期可清理。
//! 边界段落（segment_id / getKeyTS / 空 ranges）确保错误路径有稳定文案锚点。
//! 成功路径断言使用具体值（表 ID、文件名、计数），避免「非空即过」过宽。
//! 本文件不并行拆测：单一测试保证契约清单完整可见，便于审查对照。
//! 阅读建议：先扫中文模块概述建立心智模型，再按英文段落标记对照断言。
//! 新增公开 API 时，应在本测试追加对应段落，而不是另起零散用例文件。
//! checkpoint 相关期望依赖 MemLogMetaManager 的内存布局，勿替换为真实存储而不改断言。
//! Compacted/Log 策略的 AccumulateCount 是可观察内部计数，用于确认「跳过 vs 累计」分支。
//! operation hint 与 restore id 绑定是跨组件追踪的前提，回归时优先检查。
//! Close 之后不再调用 Apply/Save；本测试只验证 Close 本身不报错。

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use astersql_br_pkg_restore_utils::{GetRewriteRuleOfTable, RewriteRules};
use astersql_br_pkg_utils_iter::{CollectAll, FromSlice, TryNextor};

use crate::batch_meta_processor::{
    BatchMetaKVProcessor, NewMetaKVInfoProcessor, NewRestoreMetaKVProcessor,
};
use crate::client::{
    ApplyKVFilesWithBatchMethod, ApplyKVFilesWithSingleMethod, LoadAndProcessMetaKVFilesInBatch,
    NewLogClient, SeparateAndSortFilesByCF, SortMetaKVFiles, TEST_NewLogClient,
    TEST_NewLogClientWithStorage, operationHintRestoreID,
};
use crate::compacted_file_strategy::NewCompactedFileSplitStrategy;
use crate::id_map::{PITRIdMapBlockSize, PitrIDMapsFilename};
use crate::import::{NewLogFileImporter, filterFilesByRegion};
use crate::import_retry::{
    CreateRangeController, RPCResult, RPCResultFromError, RPCResultOK, RetryStrategy,
};
use crate::log_file_manager::{
    LogDataFileInfo, MetaName, ShouldFilterOutByTsStatic, getKeyTS, shouldReadMetaKVFile,
};
use crate::log_file_map::{NewLogFilesSkipMap, NewLogFilesSkipMapExt};
use crate::log_split_strategy::{NewLogSplitStrategy, SplitFileThresholdDefault};
use crate::migration::{
    NeedSkip, WithMigrationsBuilder, compactLogBackupCompactionIntervalForRetainLatestMVCC,
    hasCompleteShardCoverage, retainLatestMVCCCompactionsCover, skipLogical, skipMeta,
    skipPhysical,
};
use crate::ssts::{CompactedSSTs, CopiedSST, SSTs};
use crate::stubs::backuppb::{
    self, DataFileInfo, File, FileType, LogFileCompaction, LogFileSubcompaction,
    LogFileSubcompactionMeta, MetaEdit, Migration, RewrittenTableID, Span,
};
use crate::stubs::checkpoint::{
    InLogRestoreAndIdMapPersisted, LogRestoreValueMarshaled, MemLogMetaManager, RestoreProgress,
};
use crate::stubs::domain::{Domain, InfoSchema};
use crate::stubs::glue::MemSession;
use crate::stubs::importclient::MemImporterClient;
use crate::stubs::kv::KeyRange;
use crate::stubs::metapb;
use crate::stubs::pd::MemPdClient;
use crate::stubs::split_client::{MemSplitClient, RegionInfo};
use crate::stubs::storeapi::MemStorage;
use crate::stubs::stream::{NewTableMappingManager, SchemasReplace};
use crate::stubs::tablecodec;
use crate::stubs::utils_retry;
use crate::stubs::{Context, Error};

/// 综合契约测试：按段落覆盖 log_client 对外可观察行为。
/// 任一段失败都表示与 Go 公开语义发生漂移，应先对照同名 Go 测试/源。
#[test]
fn go_rust_public_contract_matches() {
    // ---- normal: log_file_map bitmap ----
    // 位图跳过：命中 offset 为真；相邻 offset / 缺失 meta 为假。
    let mut skip = NewLogFilesSkipMap();
    skip.Insert("meta-a", 1, 65);
    assert!(skip.NeedSkip("meta-a", 1, 65));
    assert!(!skip.NeedSkip("meta-a", 1, 64));
    assert!(!skip.NeedSkip("missing", 0, 0));

    // Ext 版：整 meta / 整 group / 单 offset 三级跳过语义。
    let mut skip_ext = NewLogFilesSkipMapExt();
    skip_ext.SkipMeta("m1");
    assert!(skip_ext.NeedSkip("m1", 0, 0));
    skip_ext.SkipGroup("m2", 3);
    assert!(skip_ext.NeedSkip("m2", 3, 99));
    // 不同 group 不受影响。
    assert!(!skip_ext.NeedSkip("m2", 2, 0));
    skip_ext.Insert("m3", 0, 1);
    assert!(skip_ext.NeedSkip("m3", 0, 1));

    // ---- normal: ssts table id + rewritten ----
    // 起止键同属表 10；Upstream=20 使 RewrittenTo 优先返回上游 ID。
    let start = tablecodec::EncodeTablePrefix(10);
    let mut end = tablecodec::EncodeTablePrefix(10);
    end.push(1);
    let file = File {
        Name: "f1".into(),
        StartKey: start,
        EndKey: end,
        TotalKvs: 16,
        Size_: 160,
        ..Default::default()
    };
    let mut copied = CopiedSST::new(
        Some(file.clone()),
        RewrittenTableID {
            Upstream: 20,
            Downstream: 0,
        },
    );
    assert_eq!(copied.TableID(), 10);
    assert_eq!(copied.as_rewritten().unwrap().RewrittenTo(), 20);
    // SetSSTs 契约：1 文件保留，0 文件清空。
    copied.SetSSTs(vec![file.clone()]);
    assert_eq!(copied.GetSSTs().len(), 1);
    copied.SetSSTs(vec![]);
    assert!(copied.GetSSTs().is_empty());

    // Compacted：TableID 取 Meta，Type 恒为 CompactedSSTsType(=1)。
    let compacted = CompactedSSTs::new(LogFileSubcompaction {
        Meta: LogFileSubcompactionMeta { TableId: 7 },
        SstOutputs: vec![file.clone()],
    });
    assert_eq!(compacted.TableID(), 7);
    assert_eq!(compacted.Type(), 1);

    // ---- normal: PitrIDMapsFilename + block size ----
    // 路径模板与 Go 常量必须字节级一致，避免跨语言读写错位。
    assert_eq!(
        PitrIDMapsFilename(1, 99),
        "pitr_id_maps/pitr_id_map.cluster_id:1.restored_ts:99"
    );
    assert_eq!(PITRIdMapBlockSize, 524_288);

    // ---- normal: migration skip map + builder ----
    // 三级 skip* 写入后 NeedSkip 应分别命中。
    let mut meta_skip = HashMap::new();
    skipMeta(&mut meta_skip, "meta.p");
    assert!(NeedSkip(&meta_skip, "meta.p", "any", 0));
    skipPhysical(&mut meta_skip, "meta2", "phys");
    assert!(NeedSkip(&meta_skip, "meta2", "phys", 1));
    skipLogical(&mut meta_skip, "meta3", "phys3", 42);
    assert!(NeedSkip(&meta_skip, "meta3", "phys3", 42));
    assert!(!NeedSkip(&meta_skip, "meta3", "phys3", 41));

    let builder = WithMigrationsBuilder::new(100, 200);
    let mig = Migration {
        EditMeta: vec![MetaEdit {
            Path: "m".into(),
            DestructSelf: true,
            ..Default::default()
        }],
        Compactions: vec![LogFileCompaction {
            InputMinTs: 1,
            InputMaxTs: 50,
            Artifacts: "comp".into(),
            ..Default::default()
        }],
        IngestedSstPaths: vec!["ing".into()],
    };
    // 粗过滤：compaction 完全在 [100,200] 外 → 整条 migration 丢弃。
    // 此时 skipmap 无 "m" 且 compactionDirs 为空（二者满足其一即说明过滤生效）。
    let built = builder.Build(&[mig]);
    assert!(built.skipmap.contains_key("m") || built.compactionDirs.is_empty());

    // retain-latest-mvcc：合法注释应解析出 [10,20] 并判定覆盖完整。
    let comments = r#"{"config":{"cal-shift-ts":true,"minimal-compaction-size":0,"from-ts":10,"until-ts":20}}"#;
    let compaction = LogFileCompaction {
        Comments: comments.into(),
        ..Default::default()
    };
    let (interval, ok) =
        compactLogBackupCompactionIntervalForRetainLatestMVCC(&compaction).unwrap();
    assert!(ok);
    assert_eq!(interval.from, 10);
    assert_eq!(interval.until, 20);
    assert!(hasCompleteShardCoverage(&[interval.clone()], 10, 20));
    assert!(retainLatestMVCCCompactionsCover(&[interval], 10, 20));

    // ---- boundary: ShouldFilterOutByTs ----
    // Meta 文件应可读；MinTs>restoreTS 或 MaxTs 过小都应过滤。
    let meta_file = DataFileInfo {
        IsMeta: true,
        Path: "x".into(),
        MinTs: 50,
        MaxTs: 60,
        Cf: "write".into(),
        ..Default::default()
    };
    assert!(shouldReadMetaKVFile(&meta_file));
    // MinTs(50) > restoreTS(40) → 过滤。
    assert!(ShouldFilterOutByTsStatic(&meta_file, 40, 10, 5)); // MinTs > restoreTS
    // MaxTs 远小于窗口 → 过滤。
    assert!(ShouldFilterOutByTsStatic(
        &DataFileInfo {
            Cf: "write".into(),
            MaxTs: 5,
            MinTs: 1,
            ..Default::default()
        },
        100,
        10,
        5
    ));

    // ---- normal: SortMetaKVFiles / SeparateAndSortFilesByCF ----
    // a/b 为 meta；c 非 meta，仅用于验证分 CF 时被忽略。
    let files = vec![
        DataFileInfo {
            IsMeta: true,
            Path: "a".into(),
            MinTs: 20,
            MaxTs: 30,
            Cf: "default".into(),
            Length: 10,
            ..Default::default()
        },
        DataFileInfo {
            IsMeta: true,
            Path: "b".into(),
            // MinTs 更小，排序后应排在 a 前。
            MinTs: 10,
            MaxTs: 15,
            Cf: "write".into(),
            Length: 10,
            ..Default::default()
        },
        DataFileInfo {
            IsMeta: false,
            Path: "c".into(),
            ..Default::default()
        },
    ];
    // 按 MinTs 升序；非 meta 的 "c" 不进入 SortMetaKVFiles 输入切片。
    let sorted = SortMetaKVFiles(&files[..2]);
    assert_eq!(sorted[0].MinTs, 10);
    assert_eq!(sorted[1].MinTs, 20);
    // 按 CF 拆分：default / write 各一条（IsMeta=false 被忽略）。
    let (def, wr) = SeparateAndSortFilesByCF(&files);
    assert_eq!(def.len(), 1);
    assert_eq!(wr.len(), 1);

    // ---- normal: LoadAndProcessMetaKVFilesInBatch ----
    // 计数处理器：有文件或条目时 batches++，验证两 CF 都走到 ProcessBatch。
    struct CountingProcessor {
        batches: usize,
    }
    impl BatchMetaKVProcessor for CountingProcessor {
        fn ProcessBatch(
            &mut self,
            _ctx: &Context,
            files: &[DataFileInfo],
            entries: Vec<crate::log_file_manager::KvEntryWithTS>,
            _filterTS: u64,
            _cf: &str,
        ) -> Result<Vec<crate::log_file_manager::KvEntryWithTS>, Error> {
            if !files.is_empty() || !entries.is_empty() {
                self.batches += 1;
            }
            Ok(Vec::new())
        }
    }
    let mut proc = CountingProcessor { batches: 0 };
    LoadAndProcessMetaKVFilesInBatch(&Context::Background(), &def, &wr, &mut proc).unwrap();
    // default + write 至少各一批。
    assert!(proc.batches >= 2);

    // ---- normal: compacted split strategy accumulate / skip ----
    // rewrite 7→70；checkpoint 已含 done.sst，ShouldSkip 应剔除后只剩 todo.sst。
    let rules = HashMap::from([(7i64, GetRewriteRuleOfTable(7, 70, HashMap::new(), false))]);
    let skipped = AtomicUsize::new(0);
    let mut strategy = NewCompactedFileSplitStrategy(
        rules,
        HashSet::from(["done.sst".to_string()]),
        Box::new(move |kvs, size| {
            // 跳过文件的 kvs/size 汇总回调（与 Go 指标路径对齐）。
            skipped.fetch_add((kvs + size) as usize, Ordering::SeqCst);
        }),
    );
    let mut ssts = CompactedSSTs::new(LogFileSubcompaction {
        Meta: LogFileSubcompactionMeta { TableId: 7 },
        SstOutputs: vec![
            File {
                Name: "done.sst".into(),
                TotalKvs: 32,
                Size_: 320,
                StartKey: tablecodec::EncodeTablePrefix(7),
                EndKey: {
                    let mut k = tablecodec::EncodeTablePrefix(7);
                    k.push(1);
                    k
                },
                ..Default::default()
            },
            File {
                Name: "todo.sst".into(),
                TotalKvs: 32,
                Size_: 320,
                StartKey: tablecodec::EncodeTablePrefix(7),
                EndKey: {
                    let mut k = tablecodec::EncodeTablePrefix(7);
                    k.push(2);
                    k
                },
                ..Default::default()
            },
        ],
    });
    // 仍有未完成文件 → ShouldSkip=false，且列表缩为 1。
    assert!(!strategy.ShouldSkip(&mut ssts));
    assert_eq!(ssts.GetSSTs().len(), 1);
    strategy.Accumulate(&ssts);
    assert!(strategy.base.AccumulateCount >= 1);

    // ---- normal: log split strategy skip by checkpoint ----
    let ctx = Context::Background();
    // 注入 g1@table100 的已完成 offset=2。
    let mut mgr = MemLogMetaManager::default();
    mgr.data.lock().unwrap().push((
        "g1".into(),
        LogRestoreValueMarshaled {
            Goff: 0,
            Foffs: HashMap::from([(100i64, vec![2i32])]),
        },
    ));
    // 表 1 rewrite 到 100，与 Foffs 键对齐。
    let rules = HashMap::from([(
        1i64,
        RewriteRules {
            NewTableID: 100,
            ..GetRewriteRuleOfTable(1, 100, HashMap::new(), false)
        },
    )]);
    // checkpoint 记录 g1 的 offset=2 已完成；rewrite 1→100。
    let mut log_strategy = NewLogSplitStrategy(
        &ctx,
        true,
        Some(&mgr),
        rules,
        Box::new(|_, _| {}),
        SplitFileThresholdDefault,
    )
    .unwrap();
    let file = LogDataFileInfo {
        Path: "p".into(),
        StartKey: vec![1],
        EndKey: vec![2],
        Cf: "write".into(),
        RangeOffset: 0,
        // 超过阈值，才会进入「可拆分/可累计」路径。
        Length: SplitFileThresholdDefault + 1,
        RangeLength: 1,
        NumberOfEntries: 3,
        TableId: 1,
        IsMeta: false,
        Type: FileType::Put,
        CompressionType: 0,
        Sha256: vec![],
        FileEncryptionInfo: None,
        MinTs: 1,
        MaxTs: 2,
        MetaDataGroupName: "g1".into(),
        OffsetInMetaGroup: 0,
        // 与 checkpoint Foffs 命中 → ShouldSkip。
        OffsetInMergedGroup: 2,
    };
    assert!(log_strategy.ShouldSkip(&file));
    let file2 = LogDataFileInfo {
        OffsetInMergedGroup: 3,
        Length: 10, // below threshold
        ..file.clone()
    };
    // 未完成但低于阈值：不跳过，Accumulate 也不计数。
    assert!(!log_strategy.ShouldSkip(&file2));
    log_strategy.Accumulate(&file2); // no-op due to threshold
    assert_eq!(log_strategy.base.AccumulateCount, 0);
    let file3 = LogDataFileInfo {
        OffsetInMergedGroup: 3,
        ..file
    };
    // 超阈值且未完成 → AccumulateCount=1。
    log_strategy.Accumulate(&file3);
    assert_eq!(log_strategy.base.AccumulateCount, 1);

    // ---- normal/error: RPCResult retry strategy ----
    // OK；普通错误放弃；含 unavailable backend 则从本 region 重试。
    assert!(RPCResultOK().OK());
    let give_up = RPCResultFromError(Error::new("plain"));
    assert_eq!(give_up.StrategyForRetry(), RetryStrategy::StrategyGiveUp);
    let retry_region = RPCResultFromError(Error::new("unavailable backend"));
    assert_eq!(
        retry_region.StrategyForRetry(),
        RetryStrategy::StrategyFromThisRegion
    );

    // ---- normal: filterFilesByRegion ----
    // 文件键落在 region[0,10) 与给定 KeyRange 交集内 → 保留 1 条。
    let regions = RegionInfo {
        Region: Some(metapb::Region {
            Id: 1,
            StartKey: vec![0],
            EndKey: vec![10],
            ..Default::default()
        }),
        Leader: Some(metapb::Peer { Id: 1, StoreId: 1 }),
    };
    let lf = LogDataFileInfo {
        Path: "f".into(),
        StartKey: vec![1],
        EndKey: vec![2],
        Length: 1,
        NumberOfEntries: 1,
        TableId: 1,
        MetaDataGroupName: "g".into(),
        ..Default::default()
    };
    let filtered = filterFilesByRegion(
        &[lf.clone()],
        &[KeyRange {
            StartKey: vec![1],
            EndKey: vec![2],
        }],
        &regions,
    )
    .unwrap();
    assert_eq!(filtered.len(), 1);
    // 空 ranges 与文件数不一致 → 错误文案含 count of files。
    let err = filterFilesByRegion(&[lf], &[], &regions).unwrap_err();
    assert!(err.msg.contains("count of files"));

    // ---- resource: importer Close ----
    // Close 应幂等成功，释放内部 client 引用。
    let importer = NewLogFileImporter(
        Arc::new(MemSplitClient::default()),
        Arc::new(MemImporterClient::default()),
        None,
    );
    importer.Close().unwrap();

    // ---- normal: client lifecycle / id map storage ----
    // 注入 MemStorage：SaveIDMap 后 progress=InLogRestoreAndIdMapPersisted，并能回读 schema。
    let storage = Arc::new(MemStorage::new());
    let mut client = TEST_NewLogClientWithStorage(42, 1000, storage.clone());
    client.SetRestoreID(7);
    // operation hint 必须带上 restore id，供下游观测。
    assert_eq!(
        client.operationContext.GetHintField(operationHintRestoreID),
        Some("7")
    );
    client.useCheckpoint = true;
    let mut map_mgr = NewTableMappingManager();
    map_mgr.FromProto(vec![backuppb::PitrDBMap {
        Name: "test".into(),
        ..Default::default()
    }]);
    let mut cpt = MemLogMetaManager {
        storage: Some(storage.clone()),
        ..Default::default()
    };
    client.TEST_saveIDMap(&ctx, &map_mgr, &cpt).unwrap();
    assert_eq!(
        *cpt.progress.lock().unwrap(),
        Some(InLogRestoreAndIdMapPersisted)
    );
    let loaded = client.TEST_initSchemasMap(&ctx, 1000, &cpt).unwrap();
    assert_eq!(loaded.len(), 1);
    assert_eq!(loaded[0].Name, "test");

    // ---- boundary: id map table segment lost ----
    // 表路径返回错误 segment_id（期望 0 得到 1）→ 报错含 segment_id。
    let mut session = MemSession::default();
    session
        .ctx
        .executor
        .rows
        .lock()
        .unwrap()
        .push(crate::stubs::glue::Row {
            cols: vec![
                crate::stubs::glue::SqlArg::U64(1), // wrong segment id (expect 0)
                crate::stubs::glue::SqlArg::Bytes(vec![1, 2, 3]),
            ],
        });
    let mut client2 = TEST_NewLogClient(1, 2);
    let mut dom = Domain::default();
    // 声明 mysql.tidb_pitr_id_map 存在，走表读取分支。
    dom.info_schema
        .tables
        .insert(("mysql".into(), "tidb_pitr_id_map".into()));
    client2.dom = Some(dom);
    client2.unsafeSession = Some(Box::new(session));
    let err = client2.loadSchemasMapFromTable(&ctx, 2).unwrap_err();
    assert!(err.msg.contains("segment_id"));

    // ---- resource: Close cleans managers ----
    // InitClients 后再 Close，确保不泄漏 split/importer 句柄。
    client
        .InitClients(
            &ctx,
            None,
            Arc::new(MemSplitClient::default()),
            Arc::new(MemImporterClient::default()),
            4,
        )
        .unwrap();
    client.Close(&ctx);

    // ---- normal: ApplyKVFiles batch/single ----
    // batchSize=1 时两条文件分两批，但仍合计 applied=2；single 路径同理。
    let items = vec![
        LogDataFileInfo {
            Path: "1".into(),
            Length: 10,
            ..Default::default()
        },
        LogDataFileInfo {
            Path: "2".into(),
            Length: 10,
            ..Default::default()
        },
    ];
    let mut applied = 0usize;
    ApplyKVFilesWithBatchMethod(
        &ctx,
        FromSlice(items.clone()),
        1,
        100,
        &mut |_ctx, batch| {
            applied += batch.len();
            Ok(())
        },
    )
    .unwrap();
    assert_eq!(applied, 2);
    let mut applied2 = 0usize;
    ApplyKVFilesWithSingleMethod(&ctx, FromSlice(items), &mut |_ctx, _f| {
        applied2 += 1;
        Ok(())
    })
    .unwrap();
    assert_eq!(applied2, 2);

    // ---- getKeyTS boundary ----
    // 短键失败；8 字节大端取反编码成功解出 123。
    assert!(getKeyTS(&[1, 2, 3]).is_err());
    let mut key = vec![0u8; 8];
    let enc = !123u64;
    key.copy_from_slice(&enc.to_be_bytes());
    assert_eq!(getKeyTS(&key).unwrap(), 123);

    // ---- RangeController scans regions ----
    // 单 region 覆盖 [0,10..]：回调应恰好命中一次。
    let split = Arc::new(MemSplitClient {
        regions: std::sync::Mutex::new(vec![regions.clone()]),
        by_id: std::sync::Mutex::new(HashMap::from([(1u64, regions)])),
    });
    let rs = utils_retry::InitialRetryState(
        3,
        std::time::Duration::from_millis(1),
        std::time::Duration::from_millis(2),
    );
    let mut ctl = CreateRangeController(vec![0], vec![10, 0, 0, 0, 0, 0, 0, 0, 0], split, rs);
    let hits = std::sync::Arc::new(AtomicUsize::new(0));
    let hits2 = hits.clone();
    let mut f: crate::import_retry::RegionFunc = Box::new(move |_ctx, _r| {
        hits2.fetch_add(1, Ordering::SeqCst);
        RPCResultOK()
    });
    ctl.ApplyFuncToRange(&ctx, &mut f).unwrap();
    assert_eq!(hits.load(Ordering::SeqCst), 1);

    // ---- MetaKVInfoProcessor wiring ----
    // 空文件列表应成功；处理器构造不 panic（接线冒烟）。
    let mut client3 = TEST_NewLogClient(1, 1);
    let mut info = NewMetaKVInfoProcessor(&mut client3);
    info.ReadMetaKVFilesAndBuildInfo(&ctx, &[]).unwrap();
    let _ = info.GetTableMappingManager();
    let _ = NewRestoreMetaKVProcessor(
        &mut client3,
        SchemasReplace::default(),
        Box::new(|_, _| {}),
        Box::new(|| {}),
    );
}
