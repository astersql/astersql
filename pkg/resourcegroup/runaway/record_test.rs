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

// runaway 记录键和过期清理测试，覆盖 map 复合键、owner/DDL stub 以及 infoschema 缺表静默行为。
// testing/testify、mock、JWK/JWT、TiDB session、metrics、infoschema 等外部依赖均按 Go 调用形状保留。

// Record / QuarantineRecord SQL 构造与 done 事务测试。
//
// 前半为 Go 参考；后半验证 RecordKey 合并形状、批量 insert/delete 占位符，
// 以及 `handleRunawayWatchDone` 的 begin/insert/delete/commit 顺序。

const _GO_RECORD_TEST_REFERENCE: &str = r###"
#![allow(dead_code)]
#![allow(non_camel_case_types)]
#![allow(non_snake_case)]
#![allow(unused_variables)]

// stubOwnerManager 对应 Go 的同名辅助类型，字段和嵌入接口按原测试语义保留。
// Go 类型声明: type stubOwnerManager struct{ owner.Manager }
/*
type stubOwnerManager struct{ owner.Manager }
*/

// IsOwner 对应 Go 的同名测试/辅助函数。保留 Go 辅助函数/方法的调用形状。
// Go 签名: func (stubOwnerManager) IsOwner() bool {
pub fn is_owner() {
	 return true
}

// stubDDL 对应 Go 的同名辅助类型，字段和嵌入接口按原测试语义保留。
// Go 类型声明: type stubDDL struct {
/*
type stubDDL struct {
	ddl.DDL
	om owner.Manager
}
*/

// OwnerManager 对应 Go 的同名测试/辅助函数。保留 Go 辅助函数/方法的调用形状。
// Go 签名: func (s stubDDL) OwnerManager() owner.Manager {
pub fn owner_manager() {
	 return s.om
}

// TestRecordKey 对应 Go 的同名测试/辅助函数。保留 Go 测试函数和子测试断言顺序；testify 断言保留原期望文本。
// Go 签名: func TestRecordKey(t *testing.T) {
#[test]
pub fn test_record_key() {
		// Initialize test data
		key1 := recordKey{
			ResourceGroupName: "group1",
			SQLDigest:         "digest1",
			PlanDigest:        "plan1",
		}
		// key2 is identical to key1
		key2 := recordKey{
			ResourceGroupName: "group1",
			SQLDigest:         "digest1",
			PlanDigest:        "plan1",
		}
		key3 := recordKey{
			ResourceGroupName: "group2",
		}

		// Test MapKey method
		recordMap := make(map[recordKey]*Record)
		record1 := &Record{
			ResourceGroupName: "group1",
			SQLDigest:         "digest1",
			PlanDigest:        "plan1",
		}
		// put key1 into recordMap
		recordMap[key1] = record1
		assert.Len(t, recordMap, 1, "recordMap should have 1 element")
		assert.Equal(t, "group1", recordMap[key1].ResourceGroupName, "Repeats should not be updated")
		assert.Equal(t, 0, recordMap[key1].Repeats, "Repeats should be incremented")
		// key2 is identical to key1, so we can use key2 to get the record
		assert.NotNil(t, recordMap[key1], "key1 should exist in recordMap")
		assert.NotNil(t, recordMap[key2], "key2 should exist in recordMap")
		assert.Nil(t, recordMap[key3], "key3 should not exist in recordMap")

		// put key2 into recordMap and update Repeats
		record2 := &Record{
			ResourceGroupName: "group1",
			Repeats:           1,
		}
		recordMap[key2] = record2
		assert.Len(t, recordMap, 1, "recordMap should have 1 element")
		assert.Equal(t, 1, recordMap[key1].Repeats, "Repeats should be updated")
		// change ResourceGroupName of key2 will not affect key1
		key2.ResourceGroupName = "group2"
		record3 := &Record{
			ResourceGroupName: "group2",
		}
		recordMap[key2] = record3
		assert.Len(t, recordMap, 2, "recordMap should have 1 element")
		assert.Equal(t, "group1", recordMap[key1].ResourceGroupName, "Repeats should not be updated")
		assert.Equal(t, "group2", recordMap[key2].ResourceGroupName, "ResourceGroupName should be updated")
}

// TestDeleteExpiredRowsSkipsUnavailableRunawayTable 对应 Go 的同名测试/辅助函数。保留 Go 测试函数和子测试断言顺序；时间/休眠边界按原测试断言保留；testify 断言保留原期望文本；错误分支和错误文本按 Go 测试保留。
// Go 签名: func TestDeleteExpiredRowsSkipsUnavailableRunawayTable(t *testing.T) {
#[test]
pub fn test_delete_expired_rows_skips_unavailable_runaway_table() {
		assertDeleteExpiredRowsStaysQuiet := func(t *testing.T, infoCache *infoschema.InfoCache) {
			t.Helper()

			core, recorded := observer.New(zap.ErrorLevel)
			restore := log.ReplaceGlobals(
				zap.New(core),
				&log.ZapProperties{Core: core, Level: zap.NewAtomicLevelAt(zap.InfoLevel)},
			)
    // Go defer 表示资源收尾或计数递减；后续接线 Rust 时应改成 RAII/drop。
			defer restore()

			rm := &Manager{
				ddl:       stubDDL{om: stubOwnerManager{}},
				infoCache: infoCache,
			}
			rm.deleteExpiredRows(time.Second)

			require.Empty(t, recorded.FilterMessage("delete system table failed").All())
		}

		t.Run("nil latest infoschema", func(t *testing.T) {
			assertDeleteExpiredRowsStaysQuiet(t, infoschema.NewCache(nil, 1))
		})

		t.Run("missing runaway table", func(t *testing.T) {
			infoCache := infoschema.NewCache(nil, 1)
    // 时间相关断言依赖 Go time.Time；这里保留边界和比较语义。
			infoCache.Insert(infoschema.MockInfoSchema(nil), uint64(time.Now().Unix()))
			assertDeleteExpiredRowsStaysQuiet(t, infoCache)
		})
}
"###;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::record::{
    QuarantineRecord, Record, RecordKey, SqlValue, genBatchDeleteWatchByIDStmt,
    genBatchInsertWatchStmt, genRunawayQueriesStmt, handleRunawayWatchDone,
};
use crate::syncer::SqlRow;
use crate::{Error, ExecutorRef, RestrictedSqlExecutor, Result, RunawayAction, RunawayWatchType};

/// 记录每次 Execute 调用的 SQL 与参数，便于断言事务顺序。
#[derive(Default)]
struct CapturingExecutor {
    calls: Mutex<Vec<(String, Vec<SqlValue>)>>,
    fail_on: Option<&'static str>,
}

impl RestrictedSqlExecutor for CapturingExecutor {
    fn Execute(&self, sql: &str, params: &[SqlValue]) -> Result<Vec<SqlRow>> {
        self.calls
            .lock()
            .unwrap()
            .push((sql.to_owned(), params.to_vec()));
        if self.fail_on == Some(sql) {
            return Err(Error::Storage(format!("{sql} failed")));
        }
        Ok(Vec::new())
    }
}

/// 构造带 SwitchGroup 动作的样例 quarantine 记录。
fn quarantine(id: i64, text: &str) -> QuarantineRecord {
    QuarantineRecord {
        ID: id,
        ResourceGroupName: "rg".into(),
        StartTime: 100,
        EndTime: 200,
        Watch: RunawayWatchType::Similar,
        WatchText: text.into(),
        Source: "server-1".into(),
        ExceedCause: "ProcessedKeys".into(),
        Action: RunawayAction::SwitchGroup,
        SwitchGroupName: "fallback".into(),
    }
}

/// RecordKey 可作 map 键，且批量 INSERT 参数个数与 Go 对齐。
#[test]
fn record_key_and_query_batch_merge_shape_match_go() {
    let first = Record {
        ResourceGroupName: "group1".into(),
        SQLDigest: "digest1".into(),
        PlanDigest: "plan1".into(),
        Match: "identify".into(),
        Repeats: 3,
        ..Default::default()
    };
    let same = RecordKey::from(&first);
    let mut records = HashMap::new();
    records.insert(same.clone(), first.clone());
    assert_eq!(records[&same].Repeats, 3);

    let (sql, params) = genRunawayQueriesStmt(&records);
    assert!(sql.starts_with("INSERT INTO mysql.tidb_runaway_queries"));
    assert_eq!(params.len(), 10);
    assert_eq!(params.last(), Some(&SqlValue::Int(3)));
}

/// 批量 watch SQL 形状与 done 事务四步顺序对齐 Go。
#[test]
fn watch_insert_delete_and_done_transaction_match_go() {
    let one = quarantine(7, "digest");
    assert_eq!(one.getRecordKey(), "rg/digest");
    assert_eq!(one.GetActionString(), "SwitchGroup(fallback)");

    let mut by_key = HashMap::new();
    by_key.insert(one.getRecordKey(), one.clone());
    let (insert, insert_params) = genBatchInsertWatchStmt(&by_key);
    assert!(insert.contains("mysql.tidb_runaway_watch"));
    assert_eq!(insert_params.len(), 9);

    let mut by_id = HashMap::new();
    by_id.insert(one.ID, one.clone());
    let (delete, delete_params) = genBatchDeleteWatchByIDStmt(&by_id);
    assert!(delete.ends_with("where id in (%?)"));
    assert_eq!(delete_params, vec![SqlValue::Int(7)]);

    // 校验 begin → insert done → delete watch → commit。
    let executor = Arc::new(CapturingExecutor::default());
    let reference: ExecutorRef = executor.clone();
    handleRunawayWatchDone(&reference, &one).unwrap();
    let calls = executor.calls.lock().unwrap();
    assert_eq!(calls[0].0, "begin");
    assert!(calls[1].0.contains("tidb_runaway_watch_done"));
    assert!(
        calls[2]
            .0
            .starts_with("delete from mysql.tidb_runaway_watch")
    );
    assert_eq!(calls[3].0, "commit");
}

/// Go 使用未命名返回值，defer 中的 COMMIT 错误不会覆盖已确定的成功结果。
#[test]
fn done_transaction_commit_error_is_swallowed_like_go() {
    let executor = Arc::new(CapturingExecutor {
        calls: Mutex::new(Vec::new()),
        fail_on: Some("commit"),
    });
    let reference: ExecutorRef = executor.clone();

    assert_eq!(
        handleRunawayWatchDone(&reference, &quarantine(7, "digest")),
        Ok(())
    );
    let calls = executor.calls.lock().unwrap();
    assert_eq!(calls.last().map(|call| call.0.as_str()), Some("commit"));
    assert!(!calls.iter().any(|call| call.0 == "rollback"));
}

/// 插入或删除失败时，Go 都返回原始错误并执行一次 ROLLBACK。
#[test]
fn done_transaction_write_errors_roll_back_like_go() {
    for failed_sql in [
        "insert into mysql.tidb_runaway_watch_done VALUES (null, %?, %?, %?, %?, %?, %?, %?, %?, %?, %?, %?)",
        "delete from mysql.tidb_runaway_watch where id = %?",
    ] {
        let executor = Arc::new(CapturingExecutor {
            calls: Mutex::new(Vec::new()),
            fail_on: Some(failed_sql),
        });
        let reference: ExecutorRef = executor.clone();

        assert_eq!(
            handleRunawayWatchDone(&reference, &quarantine(7, "digest")),
            Err(Error::Storage(format!("{failed_sql} failed")))
        );
        let calls = executor.calls.lock().unwrap();
        assert_eq!(calls.last().map(|call| call.0.as_str()), Some("rollback"));
        assert!(!calls.iter().any(|call| call.0 == "commit"));
    }
}
