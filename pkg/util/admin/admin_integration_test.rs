// Copyright 2019 PingCAP, Inc.
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
// Copyright 2026 AsterSQL.

// admin 集成测试：内存表/索引模拟记录-索引一致性检查。
//
// 覆盖损坏记录键导致的不一致，以及 NOT NULL 列用 origin_default 回填后再查索引。

use std::collections::{BTreeMap, HashMap};

use super::*;

/// 不应被调用的受限 SQL 桩（本测试只走 KV 扫描路径）。
struct NoSql;

impl RestrictedSqlExecutor for NoSql {
    fn exec_restricted_sql(
        &self,
        _snapshot: u64,
        _sql: &str,
        _args: &[String],
    ) -> Result<Vec<CountRow>, AdminError> {
        unreachable!()
    }
}

/// 最小会话上下文，快照为 0。
struct TestSession(NoSql);

impl SessionContext for TestSession {
    fn optimizer_use_invisible_indexes(&self) -> bool {
        false
    }
    fn set_optimizer_use_invisible_indexes(&mut self, _enabled: bool) {}
    fn transaction_start_ts(&self) -> Result<Option<u64>, AdminError> {
        Ok(None)
    }
    fn snapshot_ts(&self) -> u64 {
        0
    }
    fn restricted_sql_executor(&self) -> &dyn RestrictedSqlExecutor {
        &self.0
    }
}

/// 按键范围过滤的内存 KV 检索器。
struct MemoryRetriever(Vec<KvPair>);

impl Retriever for MemoryRetriever {
    fn iter(&self, start_key: &[u8], upper_bound: &[u8]) -> Result<Vec<KvPair>, AdminError> {
        Ok(self
            .0
            .iter()
            .filter(|pair| pair.key.as_slice() >= start_key && pair.key.as_slice() < upper_bound)
            .cloned()
            .collect())
    }
}

/// 测试表：固定前缀 `t1_`，value 解码为两列 i64。
struct TestTable {
    columns: Vec<Column>,
}

impl Table for TestTable {
    fn name(&self) -> &str {
        "t"
    }

    fn columns(&self) -> &[Column] {
        &self.columns
    }

    fn record_prefix(&self) -> Vec<u8> {
        b"t1_".to_vec()
    }

    fn decode_row(
        &self,
        _handle: Handle,
        value: &[u8],
    ) -> Result<BTreeMap<i64, Datum>, AdminError> {
        if value.len() != 16 {
            return Err(AdminError::Decode("expected two encoded i64 values".into()));
        }
        let first = i64::from_be_bytes(value[0..8].try_into().unwrap());
        let second = i64::from_be_bytes(value[8..16].try_into().unwrap());
        Ok(BTreeMap::from([
            (1, Datum::Int(first)),
            (2, Datum::Int(second)),
        ]))
    }
}

/// 唯一索引内存实现：列值 → 句柄映射。
struct UniqueIndex {
    meta: IndexMeta,
    entries: HashMap<Vec<Datum>, Handle>,
}

impl Index for UniqueIndex {
    fn meta(&self) -> &IndexMeta {
        &self.meta
    }

    fn exists(&self, values: &[Datum], handle: Handle) -> Result<IndexLookup, AdminError> {
        Ok(match self.entries.get(values) {
            None => IndexLookup::Missing,
            Some(found) if *found == handle => IndexLookup::Found,
            Some(found) => IndexLookup::Duplicate(*found),
        })
    }
}

/// 将两列 i64 大端编码为 16 字节 value。
fn encoded_row(id: i64, value: i64) -> Vec<u8> {
    [id.to_be_bytes(), value.to_be_bytes()].concat()
}

/// 篡改记录键末字节使句柄变为 2，而索引仍指向 1，应报 Inconsistent。
#[test]
fn TestAdminCheckTableCorrupted() {
    let table = TestTable {
        columns: vec![
            Column {
                id: 1,
                name: "id".into(),
                not_null: false,
                origin_default: None,
            },
            Column {
                id: 2,
                name: "v".into(),
                not_null: false,
                origin_default: None,
            },
        ],
    };
    let index = UniqueIndex {
        meta: IndexMeta {
            name: "i1".into(),
            column_offsets: vec![0, 1],
            global: false,
        },
        entries: HashMap::from([(vec![Datum::Int(1), Datum::Int(1)], Handle::Int(1))]),
    };

    // Match the Go test exactly: copy the row value, mutate the last record-key
    // byte, then commit that corrupted key. It now decodes as handle 2 while the
    // unique index still points at handle 1.
    let mut corrupted_key = encode_record_key(&table.record_prefix(), Handle::Int(1));
    *corrupted_key.last_mut().unwrap() += 1;
    let txn = MemoryRetriever(vec![KvPair {
        key: corrupted_key,
        value: encoded_row(1, 1),
    }]);
    let error = CheckRecordAndIndex(&TestSession(NoSql), &txn, &table, &index).unwrap_err();
    assert!(matches!(
        error,
        AdminError::Inconsistent {
            index_record: Some(_),
            ..
        }
    ));
}

/// NULL 列有 origin_default 时，先填默认值再查索引应成功。
#[test]
fn null_index_value_uses_origin_default_before_lookup() {
    struct NullTable(TestTable);
    impl Table for NullTable {
        fn name(&self) -> &str {
            self.0.name()
        }
        fn columns(&self) -> &[Column] {
            self.0.columns()
        }
        fn record_prefix(&self) -> Vec<u8> {
            self.0.record_prefix()
        }
        fn decode_row(
            &self,
            _handle: Handle,
            _value: &[u8],
        ) -> Result<BTreeMap<i64, Datum>, AdminError> {
            Ok(BTreeMap::from([(1, Datum::Null)]))
        }
    }
    let table = NullTable(TestTable {
        columns: vec![Column {
            id: 1,
            name: "id".into(),
            not_null: true,
            origin_default: Some(Datum::Int(7)),
        }],
    });
    let index = UniqueIndex {
        meta: IndexMeta {
            name: "i".into(),
            column_offsets: vec![0],
            global: false,
        },
        entries: HashMap::from([(vec![Datum::Int(7)], Handle::Int(1))]),
    };
    let txn = MemoryRetriever(vec![KvPair {
        key: encode_record_key(&table.record_prefix(), Handle::Int(1)),
        value: vec![],
    }]);
    CheckRecordAndIndex(&TestSession(NoSql), &txn, &table, &index).unwrap();
}
