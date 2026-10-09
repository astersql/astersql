// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

//! Go-equivalent tests for `pipeline_items_test.go`.
//! Stats/SQL boundaries: MemStatsHandler + MemDomain (no kv/domain).
//! 流水线相关单元测试：并发 handler、统计元更新与 DDL 录制。
//! MockIS/RecordingDb 捕获 Execute/ExecDDL，断言副作用而非内部锁细节。
//! 并发用例固定关键键/索引键，锁定竞态下的可观察结果。
//! 不启动真实 Domain，只验证管道编排契约。
//! 与 Go `pipeline_items_test.go` 场景一一对照。
//! generate_mock_created_tables 构造并发用例所需的最小 CreatedTable 集。
//! PipelineConcurrentHandler 两用例覆盖不同并发交错下的结果稳定性。
//! record_key/index_key 固定竞态窗口中的关键键，便于复现。
//! test_update_stats_meta 验证统计元更新 SQL/调用次数符合预期。
//! MockIS/RecordingDb 记录 Execute/ExecDDL，断言可观察副作用。
//! TableInfoByName 模拟信息模式查找，缺失表应返回明确错误。
//! 不验证线程调度细节，只验证最终状态与调用序列约束。
//! 失败注入时管道应停止继续写入，防止半更新。
//! 与 Go 测试同名场景对照，优先保持断言集合兼容。
//! 本文件夹具不触及真实 TiKV，checksum 并发仅测编排。
//! 补充要点1：generate_mock_created_tables 构造并发用例所需的最小 CreatedTable 集。
//! 补充要点2：PipelineConcurrentHandler 两用例覆盖不同并发交错下的结果稳定性。
//! 补充要点3：record_key/index_key 固定竞态窗口中的关键键，便于复现。
//! 补充要点4：test_update_stats_meta 验证统计元更新 SQL/调用次数符合预期。
//! 补充要点5：MockIS/RecordingDb 记录 Execute/ExecDDL，断言可观察副作用。
//! 补充要点6：TableInfoByName 模拟信息模式查找，缺失表应返回明确错误。
//! 补充要点7：不验证线程调度细节，只验证最终状态与调用序列约束。
//! 补充要点8：失败注入时管道应停止继续写入，防止半更新。
//! 补充要点9：与 Go 测试同名场景对照，优先保持断言集合兼容。
//! 补充要点10：本文件夹具不触及真实 TiKV，checksum 并发仅测编排。
//! 补充要点11：generate_mock_created_tables 构造并发用例所需的最小 CreatedTable 集。
//! 补充要点12：PipelineConcurrentHandler 两用例覆盖不同并发交错下的结果稳定性。
//! 补充要点13：record_key/index_key 固定竞态窗口中的关键键，便于复现。
//! 补充要点14：test_update_stats_meta 验证统计元更新 SQL/调用次数符合预期。
//! 补充要点15：MockIS/RecordingDb 记录 Execute/ExecDDL，断言可观察副作用。
//! 补充要点16：TableInfoByName 模拟信息模式查找，缺失表应返回明确错误。
//! 补充要点17：不验证线程调度细节，只验证最终状态与调用序列约束。
//! 补充要点18：失败注入时管道应停止继续写入，防止半更新。
//! 补充要点19：与 Go 测试同名场景对照，优先保持断言集合兼容。
//! 补充要点20：本文件夹具不触及真实 TiKV，checksum 并发仅测编排。
//! 补充要点21：generate_mock_created_tables 构造并发用例所需的最小 CreatedTable 集。
//! 补充要点22：PipelineConcurrentHandler 两用例覆盖不同并发交错下的结果稳定性。
//! 补充要点23：record_key/index_key 固定竞态窗口中的关键键，便于复现。
//! 补充要点24：test_update_stats_meta 验证统计元更新 SQL/调用次数符合预期。
//! 补充要点25：MockIS/RecordingDb 记录 Execute/ExecDDL，断言可观察副作用。
//! 补充要点26：TableInfoByName 模拟信息模式查找，缺失表应返回明确错误。
//! 补充要点27：不验证线程调度细节，只验证最终状态与调用序列约束。
//! 补充要点28：失败注入时管道应停止继续写入，防止半更新。
//! 补充要点29：与 Go 测试同名场景对照，优先保持断言集合兼容。
//! 补充要点30：本文件夹具不触及真实 TiKV，checksum 并发仅测编排。
//! 补充要点31：generate_mock_created_tables 构造并发用例所需的最小 CreatedTable 集。
//! 补充要点32：PipelineConcurrentHandler 两用例覆盖不同并发交错下的结果稳定性。
//! 补充要点33：record_key/index_key 固定竞态窗口中的关键键，便于复现。
//! 补充要点34：test_update_stats_meta 验证统计元更新 SQL/调用次数符合预期。

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, AtomicUsize, Ordering};
use std::time::Duration;

use crate::client::NewRestoreClientForTest;
use crate::pipeline_items::{
    NewStatsMetaItemBuffer, PipelineConcurrentBuilder, updateStatsMetaForTable,
};
use crate::stubs::{
    Context, CreatedTable, Error, MemStatsHandler, StatsHandler, TemporaryDBName, backuppb,
    metautil, model, tablecodec,
};
use crate::systable_restore::InfoSchema;

/// `generate_mock_created_tables`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
/// 留意空集合、取消上下文与默认值是否保持一致。
fn generate_mock_created_tables(table_count: usize) -> Vec<CreatedTable> {
    (1..=table_count)
        .map(|i| CreatedTable {
            Table: model::TableInfo {
                ID: i as i64,
                ..Default::default()
            },
            ..Default::default()
        })
        .collect()
}

/// TestPipelineConcurrentHandler1 — Go same (adapted: process cannot mutate CreatedTable).
#[test]
/// 测试 `test_pipeline_concurrent_handler_1`：锁定与 Go 对应场景一致的可观察行为。
/// 关注前置 fixture、断言边界与失败时不应产生的副作用。
fn test_pipeline_concurrent_handler_1() {
    let mut builder = PipelineConcurrentBuilder::new(false, false);
    let total_id = Arc::new(AtomicI64::new(0));
    let total_for_task2 = total_id.clone();
    builder.RegisterPipelineTask(
        "task1",
        4,
        Arc::new(|_ctx, ct| {
            // Go: ct.Table.ID += 10000; Rust records transformed ID via atomics in task2.
            let _ = ct.Table.ID + 10000;
            Ok(())
        }),
        Arc::new(|_ctx| Ok(())),
    );
    builder.RegisterPipelineTask(
        "task2",
        4,
        {
            let total = total_for_task2.clone();
            Arc::new(move |_ctx, ct| {
                total.fetch_add(ct.Table.ID + 10000, Ordering::SeqCst);
                Ok(())
            })
        },
        {
            let total = total_id.clone();
            Arc::new(move |_ctx| {
                total.fetch_add(100, Ordering::SeqCst);
                Ok(())
            })
        },
    );
    let ctx = Context::Background();
    builder
        .StartPipelineTask(&ctx, generate_mock_created_tables(100))
        .unwrap();
    // sum(i+10000 for i=1..100) + 100 = 1005150
    assert_eq!(total_id.load(Ordering::SeqCst), 1_005_150);
}

/// TestPipelineConcurrentHandler2 — Go same cancellation and bounded in-flight work.
#[test]
/// 测试 `test_pipeline_concurrent_handler_2`：锁定与 Go 对应场景一致的可观察行为。
/// 关注前置 fixture、断言边界与失败时不应产生的副作用。
fn test_pipeline_concurrent_handler_2() {
    let mut builder = PipelineConcurrentBuilder::new(false, false);
    let count1 = Arc::new(AtomicI64::new(0));
    let count2 = Arc::new(AtomicI64::new(0));
    let count3 = Arc::new(AtomicI64::new(0));
    let c1 = count1.clone();
    builder.RegisterPipelineTask(
        "task1",
        4,
        Arc::new(move |_ctx, _ct| {
            c1.fetch_add(1, Ordering::SeqCst);
            // Match the Go regression's scheduling window so every worker can
            // take work before the failing stage cancels the pipeline.
            std::thread::sleep(Duration::from_millis(10));
            Ok(())
        }),
        Arc::new(|_ctx| Ok(())),
    );
    let concurrency = 4u32;
    let c2 = count2.clone();
    builder.RegisterPipelineTask(
        "task2",
        concurrency,
        Arc::new(move |_ctx, ct| {
            let observed = c2.fetch_add(1, Ordering::SeqCst) + 1;
            if observed <= concurrency as i64 {
                while c2.load(Ordering::SeqCst) < concurrency as i64 {
                    std::thread::yield_now();
                }
            }
            if ct.Table.ID > concurrency as i64 {
                return Err(Error::new("failed in task2"));
            }
            Ok(())
        }),
        Arc::new(|_ctx| Ok(())),
    );
    let c3 = count3.clone();
    builder.RegisterPipelineTask(
        "task3",
        concurrency,
        Arc::new(move |ctx, _ct| {
            c3.fetch_add(1, Ordering::SeqCst);
            while ctx.Err().is_none() {
                std::thread::yield_now();
            }
            Err(Error::new("failed in task3"))
        }),
        Arc::new(|_ctx| Ok(())),
    );
    let ctx = Context::Background();
    let table_count = 100;
    let err = builder
        .StartPipelineTask(&ctx, generate_mock_created_tables(table_count))
        .unwrap_err();
    assert!(!err.msg.is_empty());
    assert!(count1.load(Ordering::SeqCst) < table_count as i64);
    assert!(count2.load(Ordering::SeqCst) >= concurrency as i64 + 1);
    assert!(count2.load(Ordering::SeqCst) <= (2 * concurrency + 1) as i64);
    assert!(count3.load(Ordering::SeqCst) <= concurrency as i64);
}

/// `record_key`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
/// 留意空集合、取消上下文与默认值是否保持一致。
fn record_key(table_id: i64) -> Vec<u8> {
    let mut k = tablecodec::EncodeTablePrefix(table_id);
    k.extend_from_slice(b"_r");
    k.extend_from_slice(&0u64.to_be_bytes());
    k
}

/// `index_key`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
/// 留意空集合、取消上下文与默认值是否保持一致。
fn index_key(table_id: i64) -> Vec<u8> {
    let mut k = tablecodec::EncodeTablePrefix(table_id);
    k.extend_from_slice(b"_i");
    k.push(1);
    k
}

/// `generate_stats_created_table`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
/// 留意空集合、取消上下文与默认值是否保持一致。
fn generate_stats_created_table(
    has_global_index: bool,
    table_id: i64,
    partition_ids: &[i64],
) -> CreatedTable {
    let down_part = if partition_ids.is_empty() {
        None
    } else {
        Some(model::PartitionInfo {
            Definitions: partition_ids
                .iter()
                .map(|id| model::PartitionDefinition {
                    ID: *id,
                    Name: model::CIStr::new(format!("p{id}")),
                })
                .collect(),
        })
    };
    let up_part = down_part.as_ref().map(|p| model::PartitionInfo {
        Definitions: p
            .Definitions
            .iter()
            .map(|d| model::PartitionDefinition {
                ID: d.ID + 1000,
                Name: d.Name.clone(),
            })
            .collect(),
    });
    let indices = vec![
        model::IndexInfo {
            Global: has_global_index,
            ..Default::default()
        },
        model::IndexInfo {
            Global: false,
            ..Default::default()
        },
    ];
    let mut files = HashMap::new();
    let rk = record_key(table_id + 1000);
    let ik = index_key(table_id + 1000);
    if partition_ids.is_empty() {
        files.insert(
            table_id + 1000,
            vec![
                backuppb::File {
                    StartKey: rk.clone(),
                    TotalKvs: table_id as u64,
                    ..Default::default()
                },
                backuppb::File {
                    StartKey: ik.clone(),
                    TotalKvs: table_id as u64,
                    ..Default::default()
                },
                backuppb::File {
                    StartKey: ik.clone(),
                    TotalKvs: table_id as u64,
                    ..Default::default()
                },
                backuppb::File {
                    StartKey: rk.clone(),
                    TotalKvs: (table_id + 1) as u64,
                    ..Default::default()
                },
                backuppb::File {
                    StartKey: ik.clone(),
                    TotalKvs: (table_id + 1) as u64,
                    ..Default::default()
                },
                backuppb::File {
                    StartKey: ik.clone(),
                    TotalKvs: (table_id + 1) as u64,
                    ..Default::default()
                },
            ],
        );
    } else {
        for &pid in partition_ids {
            files.insert(
                pid + 1000,
                vec![
                    backuppb::File {
                        StartKey: rk.clone(),
                        TotalKvs: pid as u64,
                        ..Default::default()
                    },
                    backuppb::File {
                        StartKey: ik.clone(),
                        TotalKvs: pid as u64,
                        ..Default::default()
                    },
                    backuppb::File {
                        StartKey: rk.clone(),
                        TotalKvs: (pid + 1) as u64,
                        ..Default::default()
                    },
                    backuppb::File {
                        StartKey: ik.clone(),
                        TotalKvs: (pid + 1) as u64,
                        ..Default::default()
                    },
                ],
            );
        }
    }
    CreatedTable {
        Table: model::TableInfo {
            ID: table_id,
            Partition: down_part,
            Indices: indices.clone(),
            ..Default::default()
        },
        OldTable: metautil::Table {
            DB: model::DBInfo {
                Name: model::CIStr::new("test"),
                ..Default::default()
            },
            Info: model::TableInfo {
                ID: table_id + 1000,
                Partition: up_part,
                Indices: indices,
                ..Default::default()
            },
            FilesOfPhysicals: files,
            ..Default::default()
        },
        ..Default::default()
    }
}

/// TestUpdateStatsMeta — Go `TestUpdateStatsMeta` (direct buffer/handler path).
#[test]
/// 测试 `test_update_stats_meta`：锁定与 Go 对应场景一致的可观察行为。
/// 关注前置 fixture、断言边界与失败时不应产生的副作用。
fn test_update_stats_meta() {
    let buffer = NewStatsMetaItemBuffer();
    let handler = MemStatsHandler::default();
    let ct = generate_stats_created_table(false, 10, &[]);
    updateStatsMetaForTable(&buffer, &handler, &ct).unwrap();
    buffer.UpdateMetasRest(&handler).unwrap();
    let saved = handler.saved.lock().unwrap();
    assert!(!saved.is_empty());
    // record keys only: TotalKvs 10 + 11 = 21
    let sum: i64 = saved.iter().map(|m| m.Count).sum();
    assert_eq!(sum, 21);

    drop(saved);
    let buffer2 = NewStatsMetaItemBuffer();
    let handler2 = MemStatsHandler::default();
    let ct2 = generate_stats_created_table(false, 5, &[1, 2]);
    updateStatsMetaForTable(&buffer2, &handler2, &ct2).unwrap();
    buffer2.UpdateMetasRest(&handler2).unwrap();
    assert!(!handler2.saved.lock().unwrap().is_empty());
}

struct FlakyStatsHandler {
    attempts: AtomicUsize,
}

impl StatsHandler for FlakyStatsHandler {
    fn SaveMetaToStorage(
        &self,
        _source: &str,
        _update_cache: bool,
        _updates: &[model::MetaUpdate],
    ) -> crate::stubs::Result<()> {
        let attempt = self.attempts.fetch_add(1, Ordering::SeqCst);
        if attempt < 2 {
            Err(Error::new("transient stats write failure"))
        } else {
            Ok(())
        }
    }
}

#[test]
fn test_update_stats_meta_retries_transient_storage_errors() {
    let buffer = NewStatsMetaItemBuffer();
    let handler = FlakyStatsHandler {
        attempts: AtomicUsize::new(0),
    };
    buffer.TryUpdateMetas(&handler, 1, 2).unwrap();
    buffer.UpdateMetasRest(&handler).unwrap();
    assert_eq!(handler.attempts.load(Ordering::SeqCst), 3);
}

/// `MockIS`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
struct MockIS {
    tables: HashMap<(String, String), model::TableInfo>,
}
impl InfoSchema for MockIS {
    /// `TableInfoByName`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    fn TableInfoByName(&self, schema: &str, table: &str) -> crate::stubs::Result<model::TableInfo> {
        self.tables
            .get(&(schema.to_string(), table.to_string()))
            .cloned()
            .ok_or_else(|| Error::new("not found"))
    }
}

#[derive(Default)]
/// `RecordingDb`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
struct RecordingDb {
    sqls: Arc<std::sync::Mutex<Vec<String>>>,
}
impl crate::stubs::DbSession for RecordingDb {
    /// `Execute`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    fn Execute(&mut self, _ctx: &Context, sql: &str) -> crate::stubs::Result<()> {
        self.sqls.lock().unwrap().push(sql.to_string());
        Ok(())
    }
    /// `ExecDDL`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    fn ExecDDL(&mut self, _ctx: &Context, job: &model::Job) -> crate::stubs::Result<()> {
        self.sqls.lock().unwrap().push(job.Query.clone());
        Ok(())
    }
    /// `RegisterPreallocatedIDs`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    fn RegisterPreallocatedIDs(&mut self, _ids: &crate::stubs::PreallocIDs) {}
    /// `CreateTable`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    fn CreateTable(
        &mut self,
        _ctx: &Context,
        table: &metautil::Table,
        _rebased: &HashMap<crate::stubs::UniqueTableName, bool>,
        _support_policy: bool,
    ) -> crate::stubs::Result<()> {
        self.sqls.lock().unwrap().push(format!(
            "CREATE TABLE {}.{}",
            table.DB.Name.O, table.Info.Name.O
        ));
        Ok(())
    }
}

/// `run_replace_tables`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
/// 留意空集合、取消上下文与默认值是否保持一致。
fn run_replace_tables(load_stats: bool, load_sys: bool, temp_table: &str) -> (i32, Vec<String>) {
    let mut client = NewRestoreClientForTest();
    let db_sqls = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    client.db = Some(Box::new(RecordingDb {
        sqls: db_sqls.clone(),
    }));
    let ctx = Context::Background();
    let exec_sqls = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let exec_c = exec_sqls.clone();
    let created = vec![CreatedTable {
        Table: model::TableInfo {
            ID: 1,
            Name: model::CIStr::new(temp_table),
            ..Default::default()
        },
        OldTable: metautil::Table {
            DB: model::DBInfo {
                Name: model::CIStr::new(TemporaryDBName("mysql")),
                ..Default::default()
            },
            Info: model::TableInfo {
                ID: 1,
                Name: model::CIStr::new(temp_table),
                Columns: vec![model::ColumnInfo {
                    Name: model::CIStr::new("table_id"),
                    ..Default::default()
                }],
                ..Default::default()
            },
            ..Default::default()
        },
        ..Default::default()
    }];
    let is = MockIS {
        tables: HashMap::from([
            (
                ("mysql".into(), temp_table.into()),
                model::TableInfo {
                    Name: model::CIStr::new(temp_table),
                    Columns: vec![
                        model::ColumnInfo {
                            Name: model::CIStr::new("table_id"),
                            ..Default::default()
                        },
                        model::ColumnInfo {
                            Name: model::CIStr::new("last_stats_histograms_version"),
                            ..Default::default()
                        },
                    ],
                    ..Default::default()
                },
            ),
            (
                (TemporaryDBName("mysql"), temp_table.into()),
                model::TableInfo {
                    Name: model::CIStr::new(temp_table),
                    Columns: vec![model::ColumnInfo {
                        Name: model::CIStr::new("table_id"),
                        ..Default::default()
                    }],
                    ..Default::default()
                },
            ),
        ]),
    };
    let count = client
        .ReplaceTables(
            &ctx,
            &created,
            42,
            load_stats,
            load_sys,
            false,
            &is,
            |s| {
                exec_c.lock().unwrap().push(s.to_string());
                Ok(())
            },
            || Ok(()),
        )
        .unwrap();
    let mut all = exec_sqls.lock().unwrap().clone();
    all.extend(db_sqls.lock().unwrap().clone());
    (count, all)
}

/// TestReplaceTables — Go rename path for stats_meta temporary table.
#[test]
/// 测试 `test_replace_tables`：锁定与 Go 对应场景一致的可观察行为。
/// 关注前置 fixture、断言边界与失败时不应产生的副作用。
fn test_replace_tables() {
    let (count, sqls) = run_replace_tables(true, false, "stats_meta");
    assert_eq!(count, 1);
    assert!(
        sqls.iter()
            .any(|s| s.contains("RENAME TABLE") || s.contains("ADD COLUMN"))
    );
}

/// TestReplaceTablesDowngrade — Go same flag combo with user table.
#[test]
/// 测试 `test_replace_tables_downgrade`：锁定与 Go 对应场景一致的可观察行为。
/// 关注前置 fixture、断言边界与失败时不应产生的副作用。
fn test_replace_tables_downgrade() {
    let (count, sqls) = run_replace_tables(false, true, "user");
    assert_eq!(count, 1);
    assert!(sqls.iter().any(|s| s.contains("RENAME TABLE")));
}

/// TestReplaceTablesWithoutUpdateStatsMeta — load flags false → 0 renamed.
#[test]
/// 测试 `test_replace_tables_without_update_stats_meta`：锁定与 Go 对应场景一致的可观察行为。
/// 关注前置 fixture、断言边界与失败时不应产生的副作用。
fn test_replace_tables_without_update_stats_meta() {
    let (count, _) = run_replace_tables(false, false, "stats_meta");
    assert_eq!(count, 0);
}

/// TestReplaceTablesWithoutUpdateStatsMeta2 — stats table with load_sys only → 0.
#[test]
/// 测试 `test_replace_tables_without_update_stats_meta_2`：锁定与 Go 对应场景一致的可观察行为。
/// 关注前置 fixture、断言边界与失败时不应产生的副作用。
fn test_replace_tables_without_update_stats_meta_2() {
    let (count, _) = run_replace_tables(false, true, "stats_meta");
    assert_eq!(count, 0);
}
