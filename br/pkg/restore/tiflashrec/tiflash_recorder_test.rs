// Copyright 2026 AsterSQL.
// Copyright 2022-present PingCAP, Inc.
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

//! 对应 Go `tiflash_recorder_test.go` 的等价测试。
//! InfoSchema 边界：crate 内 mock，对齐 `infoschema.MockInfoSchema`
//!（库名固定 `test`；无 kv/domain/kvproto/grpcio）。
//! 操作以闭包 Op 组合，镜像 Go 表驱动里的 add/rewrite/del/ops。

use std::collections::HashMap;

use crate::{CIStr, InfoSchema, TableMeta, TiFlashRecorder, TiFlashReplicaInfo};

/// 对应 Go `op`：对录制器施加一步状态变更。
type Op = Box<dyn Fn(&mut TiFlashRecorder)>;

/// 对应 Go `add`：登记表副本数（无 LocationLabels）。
fn add(table_id: i64, replica: i32) -> Op {
    // Count 由 i32 抬升为 u64，标签留空以聚焦 ID 语义。
    Box::new(move |tfr: &mut TiFlashRecorder| {
        tfr.AddTable(
            table_id,
            TiFlashReplicaInfo {
                Count: replica as u64,
                LocationLabels: Vec::new(),
            },
        );
    })
}

/// 对应 Go `rewrite`：表 ID 搬迁。
fn rewrite(table_id: i64, new_table_id: i64) -> Op {
    // 捕获新旧 ID，闭包可放入 ops 向量。
    Box::new(move |tfr: &mut TiFlashRecorder| {
        tfr.Rewrite(table_id, new_table_id);
    })
}

/// 对应 Go `del`：删除表记录。
fn del(table_id: i64) -> Op {
    // 删除不存在的 ID 也应安全（由实现静默处理）。
    Box::new(move |tfr: &mut TiFlashRecorder| {
        tfr.DelTable(table_id);
    })
}

/// 对应 Go `ops`：顺序组合多步操作。
fn ops(items: Vec<Op>) -> Op {
    // 顺序执行，保证 Rewrite 链可观测。
    Box::new(move |tfr: &mut TiFlashRecorder| {
        for item in &items {
            item(tfr);
        }
    })
}

/// 对应 Go `table`：期望表 ID 与副本数。
struct ExpectTable {
    /// 期望仍存在的 table ID。
    id: i64,
    /// 期望的副本 Count（i32 便于字面量）。
    replica: i32,
}

/// 对应 Go `t`：构造 ExpectTable。
fn t(id: i64, replica: i32) -> ExpectTable {
    ExpectTable { id, replica }
}

/// 单用例：操作序列 + 最终应保留的表集合。
struct RecorderCase {
    /// 对本案录制器执行的操作。
    o: Op,
    /// 操作结束后应恰好存在的表集合。
    ts: Vec<ExpectTable>,
}

/// crate 内替代 `infoschema.MockInfoSchema([]*model.TableInfo{...})`。
/// 表挂在 schema `test` 下，按 table ID 查找。
struct MockInfoSchema {
    /// id → (表元数据, 库名 CIStr)。
    tables: HashMap<i64, (TableMeta, CIStr)>,
}

impl MockInfoSchema {
    /// 批量注册表名；库名一律 `test`。
    fn new(tables: Vec<(i64, &str)>) -> Self {
        let mut map = HashMap::new();
        for (id, name) in tables {
            map.insert(
                id,
                (
                    TableMeta {
                        Name: CIStr::new(name),
                    },
                    CIStr::new("test"),
                ),
            );
        }
        Self { tables: map }
    }
}

impl InfoSchema for MockInfoSchema {
    fn TableByID(&self, id: i64) -> Option<(TableMeta, CIStr)> {
        // 未注册 ID 返回 None，驱动 DDL 跳过。
        self.tables.get(&id).cloned()
    }
}

/// 对齐 `require.ElementsMatch`：顺序无关的多重集相等。
fn elements_match(got: Vec<String>, want: &[&str]) {
    let mut got = got;
    let mut want: Vec<String> = want.iter().map(|s| (*s).to_string()).collect();
    // 排序后逐元素比较，忽略 HashMap 遍历顺序。
    got.sort();
    want.sort();
    assert_eq!(got, want, "ElementsMatch failed");
}

/// 对应 Go `TestRecorder`：增删改写后 Iterate 结果须与期望多重集一致。
#[test]
fn TestRecorder() {
    let cases: Vec<RecorderCase> = vec![
        // 简单双表登记。
        RecorderCase {
            o: ops(vec![add(42, 1), add(43, 2)]),
            ts: vec![t(42, 1), t(43, 2)],
        },
        // 删除后仅剩未删表。
        RecorderCase {
            o: ops(vec![add(42, 3), add(43, 1), del(42)]),
            ts: vec![t(43, 1)],
        },
        // 链式 Rewrite 最终落到新 ID，副本数随条目迁移。
        RecorderCase {
            o: ops(vec![
                add(41, 4),
                add(42, 8),
                rewrite(42, 1890),
                rewrite(1890, 43),
                rewrite(41, 100),
            ]),
            // 42→1890→43 保留 Count=8；41→100 保留 Count=4。
            ts: vec![t(43, 8), t(100, 4)],
        },
    ];

    for (i, case) in cases.into_iter().enumerate() {
        // 子测试名仅用于断言消息，对齐 Go t.Run 索引。
        let _subtest_name = format!("#{i}");
        let mut rec = TiFlashRecorder::New();
        (case.o)(&mut rec);
        // 用 map 消耗期望：多记/少记/副本不符都会失败。
        let mut tmap = HashMap::<i64, i32>::new();
        for expected in &case.ts {
            tmap.insert(expected.id, expected.replica);
        }

        rec.Iterate(|table_id, replica_real| {
            let replica = tmap.get(&table_id).copied();
            // 多记的 ID 会在此处失败。
            assert!(
                replica.is_some(),
                "the key {table_id} not recorded (subtest {_subtest_name})"
            );
            assert_eq!(
                replica.unwrap() as u64,
                replica_real.Count,
                "the replica mismatch (subtest {_subtest_name})"
            );
            tmap.remove(&table_id);
        });
        // 少记的 ID 会残留在 tmap。
        assert!(
            tmap.is_empty(),
            "not all required are recorded (subtest {_subtest_name}): {tmap:?}"
        );
    }
}

/// 对应 Go `TestGenSql`：校验 LOCATION LABELS 转义与库表反引号包裹。
#[test]
fn TestGenSql() {
    // 四张表覆盖：无标签、单标签、多标签、特殊字符标签。
    let fake_info = MockInfoSchema::new(vec![
        (1, "fruits"),
        (2, "whisper"),
        (3, "woods"),
        (4, "evils"),
    ]);
    let mut rec = TiFlashRecorder::New();
    rec.AddTable(
        1,
        TiFlashReplicaInfo {
            Count: 1,
            LocationLabels: Vec::new(),
        },
    );
    rec.AddTable(
        2,
        TiFlashReplicaInfo {
            Count: 2,
            LocationLabels: vec!["climate".to_string()],
        },
    );
    rec.AddTable(
        3,
        TiFlashReplicaInfo {
            Count: 3,
            LocationLabels: vec!["leaf".to_string(), "seed".to_string()],
        },
    );
    // Go: []string{`kIll'; OR DROP DATABASE test --`, `dEaTh with \"quoting\"`}
    // 恶意/特殊标签：验证单引号与反斜杠转义，防 SQL 断裂。
    rec.AddTable(
        4,
        TiFlashReplicaInfo {
            Count: 1,
            LocationLabels: vec![
                "kIll'; OR DROP DATABASE test --".to_string(),
                "dEaTh with \\\"quoting\\\"".to_string(),
            ],
        },
    );

    let sqls = rec.GenerateAlterTableDDLs(&fake_info);
    // Assert Go `alterTableSpecOf` / format.RestoreCtx output (SingleQuotes → `''`,
    // EscapeBackslash → `\\`). Go test file literal omits the doubled quote; Go
    // Restore is the semantic source of truth (Go test failure does not block).
    // 顺序无关比对；转义期望以 Go Restore 语义为准。
    elements_match(
        sqls,
        &[
            "ALTER TABLE `test`.`whisper` SET TIFLASH REPLICA 2 LOCATION LABELS 'climate'",
            "ALTER TABLE `test`.`woods` SET TIFLASH REPLICA 3 LOCATION LABELS 'leaf', 'seed'",
            "ALTER TABLE `test`.`fruits` SET TIFLASH REPLICA 1",
            "ALTER TABLE `test`.`evils` SET TIFLASH REPLICA 1 LOCATION LABELS 'kIll''; OR DROP DATABASE test --', 'dEaTh with \\\\\"quoting\\\\\"'",
        ],
    );
}

/// 对应 Go `TestGenResetSql`：每表先 REPLICA 0 再恢复目标配置。
#[test]
fn TestGenResetSql() {
    // 两表即可覆盖无标签与带标签的 Reset 成对输出。
    let fake_info = MockInfoSchema::new(vec![(1, "fruits"), (2, "whisper")]);
    let mut rec = TiFlashRecorder::New();
    rec.AddTable(
        1,
        TiFlashReplicaInfo {
            Count: 1,
            LocationLabels: Vec::new(),
        },
    );
    rec.AddTable(
        2,
        TiFlashReplicaInfo {
            Count: 2,
            LocationLabels: vec!["climate".to_string()],
        },
    );

    let sqls = rec.GenerateResetAlterTableDDLs(&fake_info);
    // 每表两条：清零 + 恢复；ElementsMatch 忽略 HashMap 遍历顺序。
    elements_match(
        sqls,
        &[
            "ALTER TABLE `test`.`whisper` SET TIFLASH REPLICA 0",
            "ALTER TABLE `test`.`whisper` SET TIFLASH REPLICA 2 LOCATION LABELS 'climate'",
            "ALTER TABLE `test`.`fruits` SET TIFLASH REPLICA 0",
            "ALTER TABLE `test`.`fruits` SET TIFLASH REPLICA 1",
        ],
    );
}
