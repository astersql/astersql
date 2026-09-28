// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
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

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use super::*;

// 中文总览：这组测试聚焦 `ReplaceConflictKeys` 的 replace-on-dup 清理路径。
// 与一般错误记录不同，这里要同时处理冲突索引、输家行、二级索引和已被提前删除的旧 key。
// 之所以单独放在文件里，是因为它比普通错误表初始化更依赖夹具编排和键空间构造。
// 整体策略是先在内存 SQL 里排布冲突记录，再通过回调模拟从 TiKV 读取最新值和批量删除旧 key。
// 也就是说，这些测试保护的不只是结果集合，还保护清理过程如何与外部依赖交互。
// 测试真正想保护的是三类语义。
// 第一类是 replace 流程会删除哪些 key，包括主键冲突对应的输家行和附带二级索引。
// 第二类是查询不到旧 key 时必须容忍 `ErrNotFound`，不能把已消失的历史垃圾当成致命错误。
// 第三类是冲突表清理必须循环执行到 `RowsAffected == 0`，确保不会留下尾巴。
// 文件里构造了整数主键和 varchar 主键两种情况，
// 因为非聚簇 varchar 主键会额外带出字符串主键索引，查询与删除次数都会不同。
// 这些差异如果不在测试里显式说明，后续很容易被误当成“随机多出来的 key”。
// 由于这里只补注释，所有键名、计数器和断言值都保持原样。
// 阅读顺序建议先看 `ReplaceCase`，再看 `run_replace_case`，最后看四个具体测试函数。
// 这样更容易理解每个具体 case 只是参数不同，而共享同一套 replace 语义护栏。
// 如果未来 replace 逻辑重构，最值得回归的就是这里的删除次数、查询次数和容错分支。

#[derive(Clone, Copy)]
enum PrimaryKey {
    // `Int` 和 `Varchar` 两个分支对应 Go 测试里最关键的两类主键形状。
    // 它们会影响冲突键如何编码，以及旧值查询时会额外碰到哪些索引 key。
    Int,
    Varchar,
}

struct ReplaceCase {
    // 这个夹具把每个测试真正变化的部分抽出来，
    // 避免重复写大段初始化逻辑时掩盖断言焦点。
    // 读取时可把它理解成一份“本次 replace 需要准备多少输家、多少二级索引和多少期望查询”。
    primary_key: PrimaryKey,
    index_name: &'static str,
    secondary_keys_per_loser: &'static [usize],
    missing_keys_per_loser: &'static [usize],
    expected_get_latest: usize,
    expected_deleted: usize,
}

fn record_key(handle: i64) -> Vec<u8> {
    // 这里构造的是行句柄 key，而不是索引 key。
    // replace 路径删除输家行时最终会落到这种 key 形状上。
    let mut key = b"t00000001_r".to_vec();
    key.extend_from_slice(&handle.to_be_bytes());
    key
}

fn named_key(kind: &str, id: usize) -> Vec<u8> {
    // 命名 key 用于模拟主键索引、二级索引和“已被提前删除”的伪键。
    format!("t00000001_{kind}_{id}").into_bytes()
}

fn bytes(value: impl Into<Vec<u8>>) -> sql::SqlValue {
    // SQL 桩读取的是统一 `SqlValue`，这里把字节向量包装成查询结果列值。
    sql::SqlValue::Bytes(value.into())
}

fn run_replace_case(case: ReplaceCase) {
    // 这是整份文件的核心夹具。
    // 它先准备冲突表查询结果，再准备“从存储取最新值”和“批量删除旧 key”两个回调。
    // 最后统一调用 `ReplaceConflictKeys`，把结果收束成删除 key 集合、查询次数和 SQL 执行日志。
    // 因而下面四个测试其实只是在复用这套流程时替换参数，而不改变流程骨架。
    // 这种写法也让不同主键形状之间真正共享同一套行为定义。
    let db = sql::DB::new_memory();
    let mut index_rows = Vec::new();
    let mut data_rows = Vec::new();
    let mut latest = HashMap::<Vec<u8>, Vec<u8>>::new();
    let mut encodings = Vec::<(Vec<u8>, Vec<kv::KvPair>)>::new();
    let mut expected_deleted_keys = Vec::new();
    let mut one_shot_row_values = HashSet::new();

    for (offset, secondary_count) in case.secondary_keys_per_loser.iter().enumerate() {
        let group = offset + 1;
        let conflict_key = match case.primary_key {
            PrimaryKey::Int => named_key("pk-int", group),
            PrimaryKey::Varchar => {
                named_key(&format!("pk-varchar-{}", ['x', 'y', 'z'][offset]), group)
            }
        };
        let winner_handle = record_key((group * 10) as i64);
        let loser_handle = record_key((group * 10 + 1) as i64);
        let winner_index_value = format!("winner-index-{group}").into_bytes();
        let loser_index_value = format!("loser-index-{group}").into_bytes();
        let loser_row_value = format!("loser-row-{group}").into_bytes();

        latest.insert(conflict_key.clone(), winner_index_value.clone());
        latest.insert(loser_handle.clone(), loser_row_value.clone());
        one_shot_row_values.insert(loser_handle.clone());

        index_rows.push(vec![
            sql::SqlValue::Int64((group * 2 - 1) as i64),
            bytes(conflict_key.clone()),
            sql::SqlValue::String(case.index_name.to_string()),
            bytes(winner_index_value),
            bytes(winner_handle),
        ]);
        index_rows.push(vec![
            sql::SqlValue::Int64((group * 2) as i64),
            bytes(conflict_key.clone()),
            sql::SqlValue::String(case.index_name.to_string()),
            bytes(loser_index_value.clone()),
            bytes(loser_handle.clone()),
        ]);
        data_rows.push(vec![
            sql::SqlValue::Int64(group as i64),
            bytes(loser_handle.clone()),
            bytes(loser_row_value.clone()),
        ]);

        let mut pairs = vec![kv::KvPair {
            Key: conflict_key,
            Val: loser_index_value,
        }];
        for secondary in 0..*secondary_count {
            let key = named_key("secondary", group * 10 + secondary);
            let value = format!("secondary-value-{group}-{secondary}").into_bytes();
            latest.insert(key.clone(), value.clone());
            expected_deleted_keys.push(key.clone());
            pairs.push(kv::KvPair {
                Key: key,
                Val: value,
            });
        }
        // Stale encoded keys model indexes already removed from TiKV. The lookup must
        // tolerate ErrNotFound and must not add these keys to the deletion batch.
        // 这部分尤其关键：replace 清理面对脏历史数据时，允许“查不到旧 key”继续前进。
        // 否则一次无害的残留索引就会把整批冲突解决流程打断。
        for missing in 0..case.missing_keys_per_loser[offset] {
            pairs.push(kv::KvPair {
                Key: named_key("already-deleted", group * 10 + missing),
                Val: format!("stale-value-{group}-{missing}").into_bytes(),
            });
        }
        encodings.push((loser_row_value, pairs));
        expected_deleted_keys.push(loser_handle);
    }

    db.push_query_rows("raw_handle", index_rows, 1);
    db.push_query_rows("kv_type <> 0", data_rows, 1);
    db.push_delete_affected(case.secondary_keys_per_loser.len() as i64);
    db.push_delete_affected(0);

    let mut cfg = config::Config::NewConfig();
    cfg.Conflict.Strategy = config::ReplaceOnDup;
    cfg.TikvImporter.Backend = config::BackendLocal.to_string();
    cfg.App.TaskInfoSchemaName = "lightning_task_info".to_string();
    let manager = New(Some(db.clone()), &cfg, log::Logger::L());
    *manager.encode_map.lock().unwrap() = encodings;
    manager.Init(context::Background()).unwrap();

    let latest = Arc::new(Mutex::new(latest));
    let one_shot_row_values = Arc::new(one_shot_row_values);
    let get_latest_count = Arc::new(AtomicUsize::new(0));
    let deleted = Arc::new(Mutex::new(Vec::<Vec<u8>>::new()));
    let latest_for_callback = latest.clone();
    let one_shot_for_callback = one_shot_row_values.clone();
    let get_count_for_callback = get_latest_count.clone();
    let deleted_for_callback = deleted.clone();
    let pool = util::WorkerPool::New(16, "resolve duplicate rows by replace");

    manager
        .ReplaceConflictKeys(
            context::Background(),
            tidbtbl::Table {
                meta: tidbtbl::TableMeta { clustered: false },
                cols: vec![tidbtbl::Column { id: 1 }],
            },
            "a",
            &pool,
            move |_ctx, key| {
                get_count_for_callback.fetch_add(1, Ordering::SeqCst);
                let mut latest = latest_for_callback.lock().unwrap();
                let result = if one_shot_for_callback.contains(key) {
                    latest.remove(key)
                } else {
                    latest.get(key).cloned()
                };
                result.ok_or_else(|| tikverr::ErrNotFound(format!("key {key:?} was deleted")))
            },
            move |_ctx, keys| {
                deleted_for_callback
                    .lock()
                    .unwrap()
                    .extend(keys.iter().cloned());
                Ok(())
            },
        )
        .unwrap();

    let mut actual_deleted = deleted.lock().unwrap().clone();
    actual_deleted.sort();
    expected_deleted_keys.sort();
    assert_eq!(expected_deleted_keys, actual_deleted);
    assert_eq!(case.expected_deleted, actual_deleted.len());
    assert_eq!(
        case.expected_get_latest,
        get_latest_count.load(Ordering::SeqCst)
    );

    let exec_log = db.exec_log();
    assert_eq!(
        1,
        exec_log
            .iter()
            .filter(
                |(query, _)| query.contains("INSERT INTO") && query.contains("conflict_error_v4")
            )
            .count(),
        "loser rows must be inserted for the data-conflict phase"
    );
    assert_eq!(
        2,
        exec_log
            .iter()
            .filter(|(query, _)| query.trim_start().starts_with("DELETE FROM")
                && query.contains("conflict_error_v4"))
            .count(),
        "cleanup must continue until RowsAffected reaches zero"
    );
    // 最后这组 SQL 日志断言对应的是“冲突数据插入一次、清理循环跑到 0 为止”。
    // 它保护的是过程副作用，而不只是最终删除 key 集合。
    // 对 replace 清理来说，副作用顺序与最终结果同样属于公共契约。
}

#[test]
fn test_replace_conflict_multiple_keys_nonclustered_pk() {
    // 一个输家带多个冲突 key 时，replace 需要把相关二级索引一起清掉。
    // 这是对“批量冲突”路径的基准覆盖。
    run_replace_case(ReplaceCase {
        primary_key: PrimaryKey::Int,
        index_name: "PRIMARY",
        secondary_keys_per_loser: &[2, 2],
        missing_keys_per_loser: &[1, 1],
        expected_get_latest: 16,
        expected_deleted: 6,
    });
}

#[test]
fn test_replace_conflict_one_key_nonclustered_pk() {
    // 这是最小整数主键 case，用来确认单 key 情况下不会多删也不会少删。
    // 它也提供了最容易人工推导的查询/删除计数。
    run_replace_case(ReplaceCase {
        primary_key: PrimaryKey::Int,
        index_name: "PRIMARY",
        secondary_keys_per_loser: &[1],
        missing_keys_per_loser: &[1],
        expected_get_latest: 7,
        expected_deleted: 2,
    });
}

#[test]
fn test_replace_conflict_one_unique_key_nonclustered_pk() {
    // 这里把冲突落在唯一索引上，验证 replace 路径不会把“唯一索引冲突”误当主键冲突处理。
    // 这样能把主键冲突和唯一索引冲突两类路径明确分开保护。
    run_replace_case(ReplaceCase {
        primary_key: PrimaryKey::Int,
        index_name: "uni_b",
        secondary_keys_per_loser: &[1, 1, 0],
        missing_keys_per_loser: &[1, 0, 0],
        expected_get_latest: 18,
        expected_deleted: 5,
    });
}

#[test]
fn test_replace_conflict_one_unique_key_nonclustered_varchar_pk() {
    // varchar 非聚簇主键会额外引入字符串主键索引探测，因此预期缺失 key 查询次数更多。
    // 这正是本文件专门保留第四个 case 的原因。
    run_replace_case(ReplaceCase {
        primary_key: PrimaryKey::Varchar,
        index_name: "uni_b",
        secondary_keys_per_loser: &[1, 1, 0],
        // A varchar nonclustered PK contributes a string-PK index in addition to
        // the hidden integer row handle, yielding four extra missing-key probes.
        // 这条英文注释描述的是 Go 对齐细节；中文补充说明其影响的是查询次数而非删除结果。
        missing_keys_per_loser: &[2, 1, 1],
        expected_get_latest: 21,
        expected_deleted: 5,
    });
}
