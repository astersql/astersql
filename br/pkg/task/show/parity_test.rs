// Copyright 2026 AsterSQL.

//! Parity tests for `br/pkg/task/show` public contracts vs Go.
//!
//! show 子命令 parity 契约测试：Rust 公开 API 与 Go `br/pkg/task/show` 行为对齐。
//! 覆盖 `convertBasic` / `convertTable` / `convertRawRange` 类型投影、MetaReader 读取路径、
//! `CreateExec` 经 ReadBackupMeta hook 组装执行器、`collectResult` 通道聚合与错误传播。
//! 不访问真实对象存储、kvproto 或 TiKV；依赖 stubs 中 MemStorage、MetaReader 与 hook 注入。
//! 断言依据：公开结构体字段逐值相等；错误文案用 `contains` 匹配关键片段（容忍 Annotate 前缀）。
//! 样例 ClusterId、TSO、版本号等常量来自 Go 单测，便于跨语言对照回归。
//! hook 用例结束必须 `set_read_backup_meta_hook(None)`，避免 thread_local 污染并行测试。
//! 单测入口 `go_rust_public_contract_matches` 串联正常、边界、错误与资源清理四类场景。
//! 对照 Go 源：`br/pkg/task/show/show_test.go` 与同包 convert/Read 相关单测。
//! 不覆盖 CLI 输出格式与终端着色；仅验证公开数据结构与错误语义。
//! MemStorage 仅作 IO 替身，不断言真实 S3/local 路径解析行为。

use std::sync::Arc;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use crate::cmd::{
    CmdExecutor, Config, CreateExec, RawRange, ShowResult, Table, TimeStamp, collectResult,
    convertBasic, convertRawRange, convertTable,
};
use crate::stubs::backuppb::{BackupMeta, CipherInfo, RawRange as PbRawRange, StorageBackend};
use crate::stubs::encryptionpb::EncryptionMethod;
use crate::stubs::{
    CIStr, Context, DBInfo, Error, HexBytes, MemStorage, MetaFile, MetaTable, NewMetaReader,
    Result, TableInfo, set_read_backup_meta_hook,
};

/// 构造 `metautil.Table` 投影样例，对应 Go 单测中注入的 schema 行。
/// `table=None` 模拟库级 schema（Info 为空 → 输出表名为空字符串）。
/// kvs/bytes/replicas 直接映射 TotalKvs、TotalBytes、TiFlashReplicas 展示字段。
fn sample_meta_table(
    db: &str,
    table: Option<&str>,
    kvs: u64,
    bytes: u64,
    replicas: i32,
) -> MetaTable {
    MetaTable {
        DB: DBInfo {
            Name: CIStr::new(db),
        },
        Info: table.map(|n| TableInfo {
            Name: CIStr::new(n),
        }),
        TotalKvs: kvs,
        TotalBytes: bytes,
        TiFlashReplicas: replicas,
    }
}

#[test]
/// 总入口：串联 show 包公开契约的正常、边界、错误与资源清理断言。
/// 单函数集中断言便于与 Go 单测一一对照，减少多 test 间的 hook 状态耦合。
fn go_rust_public_contract_matches() {
    // --- Normal: convertBasic / convertTable / convertRawRange ---
    // 正常路径：BackupMeta → ShowResult 基础字段投影，与 Go convertBasic 逐字段对齐。
    let basic = BackupMeta {
        ClusterId: 7211076907329653533,
        ClusterVersion: "\"7.1.0-alpha\"\n".into(),
        BrVersion: "BR/test".into(),
        Version: 1,
        StartVersion: 100,
        EndVersion: 440689413714870273,
        IsRawKv: false,
        RawRanges: vec![],
        RawRangeIndex: None,
    };
    let res = convertBasic(basic.clone());
    // convertBasic 只做字段拷贝与 TimeStamp 包装，不触发 storage 或 schema 读取。
    // 集群标识、版本串、BR 版本、备份版本号须与 protobuf 源一致。
    assert_eq!(res.ClusterID, 7211076907329653533);
    assert_eq!(res.ClusterVersion, "\"7.1.0-alpha\"\n");
    assert_eq!(res.BRVersion, "BR/test");
    assert_eq!(res.Version, 1);
    // StartVersion / EndVersion 包装为 TimeStamp，非 RawKV 时 Tables/RawRanges 初始为空。
    assert_eq!(res.StartVersion, TimeStamp(100));
    assert_eq!(res.EndVersion, TimeStamp(440689413714870273));
    assert!(!res.IsRawKV);
    assert!(res.Tables.is_empty());
    assert!(res.RawRanges.is_empty());

    // convertTable：MetaTable 行 → Table 展示结构（库名、表名、KV 统计、TiFlash 副本数）。
    // tpcc/customer 样例与 Go 单测 schema 命名保持一致，便于 diff 对照。
    let tbl = convertTable(sample_meta_table("tpcc", Some("customer"), 10, 20, 2));
    assert_eq!(
        tbl,
        Table {
            DBName: "tpcc".into(),
            TableName: "customer".into(),
            KVCount: 10,
            KVSize: 20,
            TiFlashReplica: 2,
        }
    );

    // convertRawRange：protobuf RawRange → 展示用 ColumnFamily + HexBytes 起止键。
    let rr = convertRawRange(&PbRawRange {
        Cf: "default".into(),
        StartKey: vec![0x01, 0xab],
        EndKey: vec![0xff],
    });
    assert_eq!(rr.ColumnFamily, "default");
    // HexBytes Display 为小写十六进制拼接，与 Go logutil.HexBytes 一致。
    assert_eq!(rr.StartKey.to_string(), "01ab");
    assert_eq!(rr.EndKey.to_string(), "ff");

    // TimeStamp display matches Go Format("Y06M01D02,15:03:04") for known TSO.
    // 已知 TSO 的 Display 须含物理时间括号段，格式与 Go oracle 布局一致。
    // TSO 440689413714870273 为 Go 单测固定样例，按 Go time.Local 格式化。
    let ts = TimeStamp(440689413714870273);
    #[cfg(unix)]
    assert_eq!(
        ts.to_string(),
        format!(
            "{}({})",
            ts.0,
            crate::cmd_test::go_local_timestamp_string(ts.0)
        )
    );
    #[cfg(not(unix))]
    assert_eq!(ts.to_string(), "440689413714870273(Y23M04D10,03:03:18)");

    // Read path: non-raw with injected schemas (storage IO mocked).
    // 非 RawKV 读取：预注入两张 schema 表，验证 MetaReader → CmdExecutor::Read 聚合路径。
    let reader = NewMetaReader(
        basic.clone(),
        MemStorage::new("mem"),
        &CipherInfo::default(),
    )
    .with_tables(vec![
        // customer/warehouse 顺序用于断言 Tables 切片保序，与 Go 注入顺序一致。
        sample_meta_table("tpcc", Some("customer"), 0, 0, 0),
        sample_meta_table("tpcc", Some("warehouse"), 1, 2, 0),
    ]);
    let exec = CmdExecutor::from_reader(reader);
    let ctx = Context::Background();
    // CmdExecutor::Read 内部会 spawn collectResult 聚合 schema 通道与 basic 投影。
    let items = exec.Read(&ctx).expect("Read ok");
    assert_eq!(items.ClusterID, 7211076907329653533);
    // Tables 顺序与注入顺序一致；非 RawKV 备份 RawRanges 为空。
    assert_eq!(items.Tables.len(), 2);
    assert_eq!(items.Tables[0].TableName, "customer");
    assert_eq!(items.Tables[1].TableName, "warehouse");
    assert!(items.RawRanges.is_empty());

    // CreateExec wires ReadBackupMeta hook → MetaReader (annotate on failure tested below).
    // CreateExec 经 hook 模拟 ReadBackupMeta：校验传入 MetaFile 与 Storage URI，返回 RawKV meta。
    set_read_backup_meta_hook(Some(Box::new(|_ctx, file, cfg| {
        assert_eq!(file, MetaFile);
        assert_eq!(cfg.Storage, "local:///tmp/show-parity");
        Ok((
            StorageBackend {
                Scheme: "local".into(),
                Path: "/tmp/show-parity".into(),
            },
            MemStorage::new("hook-store") as Arc<dyn crate::stubs::Storage>,
            BackupMeta {
                ClusterId: 42,
                ClusterVersion: "v".into(),
                BrVersion: "br".into(),
                Version: 0,
                StartVersion: 1,
                EndVersion: 2,
                IsRawKv: true,
                RawRanges: vec![PbRawRange {
                    Cf: "write".into(),
                    StartKey: vec![0xaa],
                    EndKey: vec![0xbb],
                }],
                RawRangeIndex: None,
            },
        ))
    })));
    // Config.Storage 与 hook 内 assert 一致，验证 CreateExec 把配置传入 ReadBackupMeta。
    let cfg = Config {
        Storage: "local:///tmp/show-parity".into(),
        BackendCfg: Default::default(),
        Cipher: CipherInfo {
            CipherType: EncryptionMethod::PLAINTEXT,
            CipherKey: vec![],
        },
    };
    let exec = CreateExec(&ctx, cfg).expect("CreateExec");
    let raw_res = exec.Read(&ctx).expect("raw Read");
    // hook 返回的 RawKV meta 应完整投影到 ShowResult（ClusterID、IsRawKV、RawRanges）。
    assert_eq!(raw_res.ClusterID, 42);
    assert!(raw_res.IsRawKV);
    assert_eq!(raw_res.RawRanges.len(), 1);
    assert_eq!(raw_res.RawRanges[0].ColumnFamily, "write");
    set_read_backup_meta_hook(None);

    // --- Boundary: DB-only schema (nil Info → empty table name); equal versions OK ---
    // 边界：仅库级 schema（无表 Info）→ TableName 为空串，DBName 仍保留。
    let db_only = convertTable(sample_meta_table("tpcc", None, 0, 0, 0));
    assert_eq!(db_only.TableName, "");
    assert_eq!(db_only.DBName, "tpcc");

    // StartVersion == EndVersion 为合法快照点；无预注入表时 Tables 为空。
    let equal_meta = BackupMeta {
        StartVersion: 7,
        EndVersion: 7,
        IsRawKv: false,
        ..basic.clone()
    };
    let reader = NewMetaReader(equal_meta, MemStorage::new("eq"), &CipherInfo::default());
    let items = CmdExecutor::from_reader(reader)
        .Read(&ctx)
        .expect("equal versions ok");
    assert_eq!(items.StartVersion, items.EndVersion);
    assert!(items.Tables.is_empty());

    // Empty raw ranges list.
    // RawKV 备份但 RawRanges 为空列表：Read 成功且 RawRanges 切片为空。
    let raw_meta = BackupMeta {
        IsRawKv: true,
        RawRanges: vec![],
        RawRangeIndex: None,
        StartVersion: 1,
        EndVersion: 2,
        ..Default::default()
    };
    let items = CmdExecutor::from_reader(NewMetaReader(
        raw_meta,
        MemStorage::new("raw-empty"),
        &CipherInfo::default(),
    ))
    .Read(&ctx)
    .expect("empty raw");
    assert!(items.RawRanges.is_empty());

    // collectResult: normal drain + close.
    // collectResult 正常路径：后台线程发送两项后经 err 通道关闭，映射函数逐元素变换。
    let (tx, rx) = mpsc::sync_channel(16);
    let (etx, erx) = mpsc::channel();
    thread::spawn(move || {
        // 模拟 schema goroutine：先发数据再 err 通道 Ok(())，最后 drop tx 关闭数据通道。
        tx.send(1).unwrap();
        tx.send(2).unwrap();
        let _ = etx.send(Ok(()));
        drop(tx);
    });
    let got = collectResult(&ctx, rx, erx, |x| x * 10).expect("collect");
    // 映射闭包逐元素乘 10，验证 collectResult 保序且不丢项。
    assert_eq!(got, vec![10, 20]);

    // --- Error: start > end (log backup meta) ---
    // 错误路径：StartVersion > EndVersion 触发 invalid metafile（log backup 语义）。
    let bad = BackupMeta {
        StartVersion: 200,
        EndVersion: 100,
        IsRawKv: false,
        ..Default::default()
    };
    let err = CmdExecutor::from_reader(NewMetaReader(
        bad,
        MemStorage::new("bad"),
        &CipherInfo::default(),
    ))
    .Read(&ctx)
    .expect_err("start>end");
    // 错误文案须同时提及 invalid metafile、start version、log backup 三处关键字。
    assert!(
        err.msg.contains("invalid metafile")
            && err.msg.contains("start version")
            && err.msg.contains("log backup"),
        "unexpected err: {}",
        err.msg
    );

    // Error: raw kv meta v2 unsupported.
    // RawKV meta v2（RawRangeIndex 非空）当前不支持，须返回明确错误文案。
    let v2raw = BackupMeta {
        IsRawKv: true,
        StartVersion: 1,
        EndVersion: 2,
        RawRangeIndex: Some(vec![1, 2, 3]),
        ..Default::default()
    };
    let err = CmdExecutor::from_reader(NewMetaReader(
        v2raw,
        MemStorage::new("v2"),
        &CipherInfo::default(),
    ))
    .Read(&ctx)
    .expect_err("v2 raw");
    // v2 拒绝路径只检查 RawRangeIndex 指针非空，不解析 index 字节内容。
    assert!(
        err.msg.contains("backup meta v2 isn't supported"),
        "{}",
        err.msg
    );

    // Error: ReadSchemasFiles failure surfaced via collectResult.
    // ReadSchemasFiles 失败应沿 collectResult 向上冒泡，错误信息含注入的 schema 原因。
    let reader = NewMetaReader(
        BackupMeta {
            StartVersion: 1,
            EndVersion: 2,
            IsRawKv: false,
            ..Default::default()
        },
        MemStorage::new("err"),
        &CipherInfo::default(),
    )
    .with_read_err(Error::new("schema boom"));
    // with_read_err 令 MetaReader.ReadSchemasFiles 首行即返回，不经 collect 通道。
    let err = CmdExecutor::from_reader(reader)
        .Read(&ctx)
        .expect_err("schema err");
    assert!(err.msg.contains("schema boom"), "{}", err.msg);

    // Error: CreateExec annotates ReadBackupMeta failure.
    // 无 hook 时 CreateExec 应失败并带 "failed to create execution" 注解前缀。
    set_read_backup_meta_hook(None);
    // 清 hook 后 CreateExec 无法 ReadBackupMeta，应被 Annotate 为 create execution 失败。
    let err = CreateExec(
        &ctx,
        Config {
            Storage: "local:///missing".into(),
            ..Default::default()
        },
    )
    .expect_err("no hook");
    assert!(
        err.msg.contains("failed to create execution"),
        "{}",
        err.msg
    );

    // Error: context cancel during collectResult.
    // 已取消的 Context 在 collectResult 等待通道时应立即返回 context canceled。
    let (ctx_c, cancel) = Context::WithCancel(&Context::Background());
    let (_tx, rx) = mpsc::sync_channel::<i32>(1);
    let (_etx, erx) = mpsc::channel::<Result<()>>();
    cancel.cancel();
    // 空通道 + 已取消 ctx：collectResult 应在阻塞 recv 前检测 Done。
    let err = collectResult(&ctx_c, rx, erx, |x| x).expect_err("canceled");
    assert!(err.msg.contains("context canceled"), "{}", err.msg);

    // --- Resource: executor / channels drop cleanly after Read ---
    // 资源清理：Read 完成后 drop executor 不应 panic；后台 collect 线程可正常结束。
    let reader = NewMetaReader(
        BackupMeta {
            StartVersion: 1,
            EndVersion: 2,
            IsRawKv: false,
            ..Default::default()
        },
        MemStorage::new("drop"),
        &CipherInfo::default(),
    )
    .with_tables(vec![sample_meta_table("db", Some("t"), 0, 0, 0)]);
    let exec = CmdExecutor::from_reader(reader);
    let _ = exec.Read(&Context::Background()).expect("read");
    // drop(exec) 释放 reader 与内部 join handle，不应触发 double-free 或 panic。
    drop(exec);

    // HexBytes / ShowResult ownership is plain data — no external handles.
    // ShowResult / HexBytes 为纯值类型，无外部句柄；构造即拥有，drop 无额外副作用。
    let _owned: ShowResult = ShowResult {
        // RawRanges 内 HexBytes 为 owned Vec，验证 show 结果可独立持有与释放。
        ClusterID: 1,
        RawRanges: vec![RawRange {
            ColumnFamily: "default".into(),
            StartKey: HexBytes(vec![1]),
            EndKey: HexBytes(vec![2]),
        }],
        ..Default::default()
    };
    // Allow background collect threads to finish.
    // 短暂 sleep 让 collectResult 后台线程收尾，避免测试进程过早退出产生 flaky。
    // 20ms 为经验值，仅 parity 单测使用，不对齐 Go 测试中的 sleep 时长。
    thread::sleep(Duration::from_millis(20));
}

#[test]
fn child_context_observes_late_parent_cancellation() {
    let (parent, cancel_parent) = Context::WithCancel(&Context::Background());
    let (child, _cancel_child) = Context::WithCancel(&parent);

    assert!(!child.Done());
    cancel_parent.cancel();

    assert!(
        child.Done(),
        "child must observe cancellation after creation"
    );
    assert_eq!(
        child.Err().expect("child cancellation error").msg,
        "context canceled"
    );
}
