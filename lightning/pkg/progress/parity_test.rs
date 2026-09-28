// Copyright 2026 AsterSQL.

//! Parity tests for `lightning/pkg/progress` public contracts vs Go.
//! 这些测试保护的是进度模块对外可见的状态推进与 JSON 外观。
//! 它们不依赖真实网络或存储，只验证 Rust 端是否维持 Go 同款可观察结果。
//! 场景仍按正常路径、边界条件、错误传播与资源清理分类，
//! 方便快速定位哪类契约发生了漂移。
//! `sample_db_metas` 负责稳定初始化输入，避免样例数据把断言目标分散。
//! `sample_checkpoint` 把 checkpoint 构造约束收敛到单引擎单 chunk。
//! `contract_normal` 主要保护状态推进、JSON 字段名和步骤去重行为。
//! `contract_boundary` 主要保护未启用、空表集和 not found 路径。
//! `contract_error` 主要保护表级错误与任务级错误的消息传播。
//! `contract_resource_cleanup` 主要保护全局单例重置与 checkpoint 清理。
//! 这些中文注释故意把每组用例的目的写清楚，
//! 这样未来有人改序列化细节时能知道自己碰到了哪条承诺。
//! 测试里的数值样例不重要，真正重要的是它们触发的分支条件。
//! 例如 1000 字节总量、40 字节已写入，只是为了落到 Go 对齐公式。
//! 如果后续 checkpoint 包调整内部结构，这里仍应继续守护外部表现。
//! 因此测试关注的是 `w/z/s/m/progresses` 这些公开字段，而不是内部锁状态。
//! 表级状态和引擎级状态可以短暂不一致，这一点也被明确纳入契约。
//! 缺失表上的 `BroadcastError` 仍应无副作用，防止监听器收到竞态噪音时崩溃。
//! `UniqueTable` 的转义规则也在这里顺带受保护，因为它直接影响 JSON 键名。
//! 即使实现改成别的锁模型，只要外部 JSON 和错误语义不变，这些测试仍然成立。
//! 这也是 parity test 相比单元测试更强调“外观一致”的原因。

use std::collections::HashMap;

use astersql_lightning_pkg_checkpoints::{
    CheckpointStatusAllWritten, CheckpointStatusLoaded, ChunkCheckpoint, ChunkCheckpointKey,
    EngineCheckpoint, StatusCheckpointMerger, TableCheckpoint, TableCheckpointDiff,
    TableCheckpointMerger, WholeTableEngineID,
};

use crate::common::UniqueTable;
use crate::errors::{self, Error};
use crate::mydump::{MDDatabaseMeta, MDTableMeta};
use crate::*;

#[test]
fn go_rust_public_contract_matches() {
    // 主测试只做场景编排，
    // 让具体失败点落在更细粒度的契约函数中。
    contract_normal();
    contract_boundary();
    contract_error();
    contract_resource_cleanup();
}

#[test]
fn unique_table_preserves_utf8_identifiers() {
    assert_eq!(UniqueTable("数`据库", "表名"), "`数``据库`.`表名`");
}

fn sample_db_metas() -> Vec<MDDatabaseMeta> {
    // 样例元数据只覆盖两张表，
    // 足够验证初始化 map、唯一表名和总大小填充。
    vec![MDDatabaseMeta {
        Name: "test".into(),
        Tables: vec![
            MDTableMeta {
                Name: "t1".into(),
                TotalSize: 1000,
            },
            MDTableMeta {
                Name: "t2".into(),
                TotalSize: 2000,
            },
        ],
    }]
}

fn sample_checkpoint(offset: i64, end: i64, status: u8) -> TableCheckpoint {
    // checkpoint 样例故意只保留单引擎单 chunk，
    // 这样可以把断言聚焦在聚合公式，而不是复杂测试夹具。
    let mut engines = HashMap::new();
    engines.insert(
        0,
        EngineCheckpoint {
            Status: status,
            Chunks: vec![ChunkCheckpoint {
                Key: ChunkCheckpointKey {
                    Path: "/tmp/t1.sql".into(),
                    Offset: 0,
                },
                Chunk: astersql_lightning_pkg_checkpoints::mydump::Chunk {
                    Offset: offset,
                    RealOffset: offset,
                    EndOffset: end,
                    PrevRowIDMax: 0,
                    RowIDMax: 10,
                },
                ..Default::default()
            }],
        },
    );
    TableCheckpoint {
        Status: status,
        Engines: engines,
        ..Default::default()
    }
}

fn contract_normal() {
    // 正常路径覆盖“启用 -> 开始 -> 初始化 -> 更新 -> 序列化”完整链路。
    reset_progress_for_test();
    EnableCurrentProgress();
    BroadcastStartTask();
    BroadcastInitProgress(&sample_db_metas());

    let t1 = UniqueTable("test", "t1");
    let t2 = UniqueTable("test", "t2");

    let cp = sample_checkpoint(100, 1000, CheckpointStatusLoaded);
    BroadcastTableCheckpoint(&t1, &cp);
    BroadcastTableProgress(&t1, "encode", 0.25);
    BroadcastTableProgress(&t1, "encode", 0.5); // update same step
    BroadcastTableProgress(&t1, "import", 0.1);

    let mut diffs = HashMap::new();
    let mut cpd = TableCheckpointDiff {
        engines: HashMap::new(),
        ..Default::default()
    };
    StatusCheckpointMerger {
        EngineID: 0,
        Status: CheckpointStatusAllWritten,
    }
    .MergeInto(&mut cpd);
    // bump chunk offset via apply path used by progress.update
    // 这里复用 checkpoint diff 的真实合并路径，
    // 避免测试只验证手工拼出的理想状态。
    diffs.insert(t1.clone(), cpd);

    // Re-insert checkpoint then apply a status-only diff (TotalSize path).
    let cp_written = sample_checkpoint(100, 1000, CheckpointStatusLoaded);
    BroadcastTableCheckpoint(&t1, &cp_written);
    BroadcastCheckpointDiff(&diffs);

    let bytes = MarshalTaskProgress().expect("marshal task");
    let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(v["s"], 1); // running
    assert_eq!(v["t"][&t1]["z"], 1000);
    assert_eq!(v["t"][&t2]["z"], 2000);
    assert_eq!(v["t"][&t1]["s"], 1); // table running
    // encode updated in place; import appended
    let progresses = v["t"][&t1]["progresses"].as_array().unwrap();
    assert_eq!(progresses.len(), 2);
    assert_eq!(progresses[0]["step"], "encode");
    assert_eq!(progresses[0]["progress"], 0.5);
    assert_eq!(progresses[1]["step"], "import");
    // engine status >= AllWritten → TotalSize = EndOffset - Key.Offset = 1000
    assert_eq!(v["t"][&t1]["w"], 1000);

    let cp_bytes = MarshalTableCheckpoints(&t1).expect("marshal cp");
    let cp_v: serde_json::Value = serde_json::from_slice(&cp_bytes).unwrap();
    // 表级状态与引擎级状态并不总是同步抬升，
    // 这里显式保护这种细粒度差异。
    assert_eq!(cp_v["Status"], CheckpointStatusLoaded);
    assert_eq!(cp_v["Engines"]["0"]["Status"], CheckpointStatusAllWritten);
    assert!(cp_v["Engines"]["0"]["Chunks"].is_array());

    BroadcastEndTask(None);
    let bytes = MarshalTaskProgress().unwrap();
    let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(v["s"], 2); // completed
    assert!(v.get("m").is_none() || v["m"] == ""); // omitempty empty
}

fn contract_boundary() {
    // 未启用时所有广播都应退化为 no-op，
    // 同时查询接口要返回显式错误而不是空结果。
    reset_progress_for_test();
    BroadcastStartTask();
    BroadcastInitProgress(&sample_db_metas());
    BroadcastTableProgress("x", "step", 1.0);
    assert!(MarshalTaskProgress().is_err());
    assert!(MarshalTableCheckpoints("x").is_err());
    let err = MarshalTaskProgress().unwrap_err();
    assert_eq!(err.msg, "progress is not enabled");

    EnableCurrentProgress();
    // Tables nil → JSON null for "t"
    // 这里保护 `Option` 到 JSON `null` 的外观，而不是空对象。
    let bytes = MarshalTaskProgress().unwrap();
    let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert!(v["t"].is_null());
    assert_eq!(v["s"], 0);

    // missing checkpoint → NotFound
    let err = MarshalTableCheckpoints("`db`.`t`").unwrap_err();
    assert!(err.not_found);
    assert!(err.msg.contains("not found"));

    // BroadcastError on missing table is a no-op (Go nil map entry)
    BroadcastInitProgress(&sample_db_metas());
    BroadcastError("`missing`.`x`", Some(&Error::new("boom")));
    let bytes = MarshalTaskProgress().unwrap();
    let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let t1 = UniqueTable("test", "t1");
    assert_eq!(v["t"][&t1]["s"], 0);
}

fn contract_error() {
    // 错误路径既要保护表级消息，也要保护任务级消息的最终覆盖顺序。
    reset_progress_for_test();
    EnableCurrentProgress();
    BroadcastInitProgress(&sample_db_metas());
    let t1 = UniqueTable("test", "t1");

    let e = Error::new("encode failed");
    BroadcastError(&t1, Some(&e));
    let bytes = MarshalTaskProgress().unwrap();
    let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(v["t"][&t1]["s"], 2); // completed
    assert_eq!(v["t"][&t1]["m"], "encode failed");

    BroadcastEndTask(Some(&Error::new("task failed")));
    let bytes = MarshalTaskProgress().unwrap();
    let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(v["s"], 2);
    assert_eq!(v["m"], "task failed");

    // ErrorStack(nil) → empty message
    // 空错误不能被序列化成 `"null"` 或其他占位文本。
    BroadcastEndTask(None);
    let bytes = MarshalTaskProgress().unwrap();
    let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert!(v.get("m").is_none() || v["m"] == "");

    assert_eq!(errors::ErrorStack(None), "");
    assert_eq!(errors::ErrorStack(Some(&Error::new("x"))), "x".to_string());
}

fn contract_resource_cleanup() {
    // 这一组主要保护全局单例与 checkpoint 快照的清理语义。
    reset_progress_for_test();
    EnableCurrentProgress();
    BroadcastInitProgress(&sample_db_metas());
    let t1 = UniqueTable("test", "t1");
    let cp = sample_checkpoint(10, 100, CheckpointStatusLoaded);
    BroadcastTableCheckpoint(&t1, &cp);
    assert!(checkpoints_contains_for_test(&t1));

    // StartTask clears checkpoint snapshot (Go clear)
    BroadcastStartTask();
    assert!(!checkpoints_contains_for_test(&t1));
    let err = MarshalTableCheckpoints(&t1).unwrap_err();
    assert!(err.not_found);

    // unfinished engine uses Offset - Key.Offset for TotalWritten
    // 这个分支保护“未完成引擎只按已推进 offset 计数”的展示语义。
    let cp = sample_checkpoint(40, 100, CheckpointStatusLoaded);
    BroadcastTableCheckpoint(&t1, &cp);
    let diffs: HashMap<String, TableCheckpointDiff> = HashMap::from([(
        t1.clone(),
        TableCheckpointDiff {
            engines: HashMap::new(),
            ..Default::default()
        },
    )]);
    BroadcastCheckpointDiff(&diffs);
    let bytes = MarshalTaskProgress().unwrap();
    let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(v["t"][&t1]["w"], 40); // 40 - 0
    assert_eq!(v["t"][&t1]["s"], 1);

    // UniqueTable escaping
    assert_eq!(UniqueTable("a`b", "c"), "`a``b`.`c`");

    // Whole-table status merge still builds a usable diff (smoke vs checkpoints API)
    // 整表状态合并属于 checkpoint 包协作边界，这里至少做烟雾保护。
    let mut cpd = astersql_lightning_pkg_checkpoints::NewTableCheckpointDiff();
    StatusCheckpointMerger {
        EngineID: WholeTableEngineID,
        Status: CheckpointStatusAllWritten,
    }
    .MergeInto(&mut cpd);
    assert!(cpd.hasStatus);
}
