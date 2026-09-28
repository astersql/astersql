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

// 分区表统计版本参与计划缓存键（plan cache key）的用例。
//
// 计划缓存（plan cache）复用已生成的执行计划；缓存键必须编码表统计版本等元数据。
// 当会话开启「新统计使缓存失效」（`invalidate_on_fresh_stats`）时，同一语句在
// `stats_version` 变化后应生成不同的缓存键，避免沿用过期代价估算。

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
enum PartitionId {
    Int(i32),
    String(String),
}

fn get_id_str(id: &PartitionId) -> String {
    match id {
        PartitionId::Int(value) => value.to_string(),
        PartitionId::String(value) => format!("'{value}'"),
    }
}

fn get_row_data(
    row_data: &std::collections::HashMap<PartitionId, String>,
    filler: &str,
    columns: &[&str],
    case_sensitive: bool,
    ids: &[PartitionId],
) -> Vec<String> {
    use std::collections::HashSet;

    let mut rows = Vec::new();
    let mut seen = HashSet::new();
    for id in ids {
        let in_partition = match id {
            PartitionId::Int(value) => *value < 2_000_000,
            PartitionId::String(value) if case_sensitive => value.as_str() < "x",
            PartitionId::String(value) => value.to_lowercase().as_str() < "x",
        };
        if !in_partition || !seen.insert(id.clone()) {
            continue;
        }

        let row = columns
            .iter()
            .map(|column| match *column {
                "a" => match id {
                    PartitionId::Int(value) => value.to_string(),
                    PartitionId::String(value) => value.clone(),
                },
                "b" => row_data[id].clone(),
                "c" => filler.to_owned(),
                "space(1)" => " ".to_owned(),
                unsupported => panic!("unsupported projection {unsupported}"),
            })
            .collect::<Vec<_>>()
            .join(" ");
        rows.push(row);
    }
    rows.sort();
    rows
}

#[derive(Debug, PartialEq)]
struct PartCover<'a> {
    columns: [&'a str; 3],
    keys: &'a [&'a str],
    point_get_explain: Option<&'a str>,
}

#[derive(Debug, PartialEq)]
struct PartSql<'a> {
    sql: &'a str,
    can_use_batch_point_get: bool,
}

const VARCHAR_TABLES: [PartCover<'static>; 5] = [
    PartCover {
        columns: ["a varchar(255) primary key", "b varchar(255)", "c text"],
        keys: &["key(b)"],
        point_get_explain: Some("clustered index:PRIMARY(a)"),
    },
    PartCover {
        columns: ["a varchar(255) primary key", "b varchar(255)", "c text"],
        keys: &["key (b)"],
        point_get_explain: Some("clustered index:PRIMARY(a)"),
    },
    PartCover {
        columns: ["a varchar(255)", "b varchar(255)", "c text"],
        keys: &["key (b)", "unique index a(a)"],
        point_get_explain: Some("index:a"),
    },
    PartCover {
        columns: ["a varchar(255)", "b varchar(255)", "c text"],
        keys: &["key (b)"],
        point_get_explain: None,
    },
    PartCover {
        columns: ["a varchar(255)", "b varchar(255)", "c text"],
        keys: &["key (b)", "key (a)"],
        point_get_explain: None,
    },
];

const VARCHAR_PARTITIONS: [PartSql<'static>; 2] = [
    PartSql {
        sql: "partition by range columns (a) (partition p0 values less than ('k'), partition p1 values less than ('x'))",
        can_use_batch_point_get: false,
    },
    PartSql {
        sql: "partition by key (a) partitions 7",
        can_use_batch_point_get: false,
    },
];

const INT_TABLES: [PartCover<'static>; 5] = [
    PartCover {
        columns: [
            "a int unsigned primary key auto_increment",
            "b varchar(255)",
            "c text",
        ],
        keys: &["key(b)"],
        point_get_explain: Some("handle:"),
    },
    PartCover {
        columns: [
            "a int primary key auto_increment",
            "b varchar(255)",
            "c text",
        ],
        keys: &["key (b)"],
        point_get_explain: Some("handle:"),
    },
    PartCover {
        columns: ["a int", "b varchar(255)", "c text"],
        keys: &["key (b)", "unique index a(a)"],
        point_get_explain: Some("index:a"),
    },
    PartCover {
        columns: ["a int", "b varchar(255)", "c text"],
        keys: &["key (b)"],
        point_get_explain: None,
    },
    PartCover {
        columns: ["a int", "b varchar(255)", "c text"],
        keys: &["key (b)", "key (a)"],
        point_get_explain: None,
    },
];

const INT_PARTITIONS: [PartSql<'static>; 5] = [
    PartSql {
        sql: "partition by range (a) (partition p0 values less than (1000000), partition p1 values less than (2000000))",
        can_use_batch_point_get: true,
    },
    PartSql {
        sql: "partition by range (floor(a*0.5)*2) (partition p0 values less than (1000000), partition p1 values less than (2000000))",
        can_use_batch_point_get: false,
    },
    PartSql {
        sql: "partition by hash (a) partitions 7",
        can_use_batch_point_get: true,
    },
    PartSql {
        sql: "partition by hash (floor(a*0.5)) partitions 3",
        can_use_batch_point_get: false,
    },
    PartSql {
        sql: "partition by key (a) partitions 7",
        can_use_batch_point_get: false,
    },
];

#[test]
fn go_partition_case_matrix_and_row_oracle_are_preserved() {
    use std::collections::HashMap;

    assert_eq!((VARCHAR_TABLES.len(), VARCHAR_PARTITIONS.len()), (5, 2));
    assert_eq!((INT_TABLES.len(), INT_PARTITIONS.len()), (5, 5));
    assert_eq!(
        INT_PARTITIONS
            .iter()
            .filter(|p| p.can_use_batch_point_get)
            .count(),
        2
    );
    assert!(
        VARCHAR_PARTITIONS
            .iter()
            .all(|p| !p.can_use_batch_point_get)
    );

    let ids = [
        PartitionId::Int(13),
        PartitionId::Int(2_000_000),
        PartitionId::Int(13),
        PartitionId::String("Xray".into()),
        PartitionId::String("zulu".into()),
    ];
    let row_data = HashMap::from([
        (PartitionId::Int(13), "thirteen".into()),
        (PartitionId::String("Xray".into()), "mixed-case".into()),
    ]);
    assert_eq!(get_id_str(&ids[0]), "13");
    assert_eq!(get_id_str(&ids[3]), "'Xray'");
    assert_eq!(
        get_row_data(&row_data, "Filler", &["a", "b", "space(1)"], false, &ids),
        ["13 thirteen  "]
    );
    assert_eq!(
        get_row_data(&row_data, "Filler", &["a", "c"], true, &ids),
        ["13 Filler", "Xray Filler"]
    );
}

/// 真实分区表 PREPARE/EXECUTE：参数改变时仍应命中正确分区并返回对应行。
#[test]
fn prepared_point_get_reads_the_selected_partition() {
    use astersql_testkit::mockstore::CreateMockStoreAndDomain;
    use astersql_testkit::{Rows, TestKit};

    let (store, _domain) = CreateMockStoreAndDomain();
    let mut testkit = TestKit::new(store);
    testkit.MustExec(
        "create table pt (a int primary key, b int) partition by range (a) (partition p0 values less than (10), partition p1 values less than (20))",
        Vec::new(),
    );
    testkit.MustExec("insert into pt values (3, 30), (13, 130)", Vec::new());
    testkit.MustExec("prepare p from 'select b from pt where a = ?'", Vec::new());
    testkit.MustExec("set @a = 3", Vec::new());
    testkit
        .MustQuery("execute p using @a", Vec::new())
        .Check(Rows(&["30"]));
    testkit.MustExec("set @a = 13", Vec::new());
    testkit
        .MustQuery("execute p using @a", Vec::new())
        .Check(Rows(&["130"]));
}

/// 验证：开启 `invalidate_on_fresh_stats` 后，分区/表 `stats_version` 进入缓存键。
///
/// 构造同一 `PlanCacheStmt`，先后收集 `stats_version` 为 1 与 2 的 `PlanCacheTable`
/// 信息，再分别调用 `NewPlanCacheKey`；两份键的字节序列必须不相等。
#[test]
fn fresh_partition_stats_participate_in_the_cache_key() {
    use astersql_planner_core::{
        NewPlanCacheKey, PlanCacheKeyContext, PlanCacheStmt, PlanCacheTable, ast,
    };

    // 准备一条带 SchemaVersion 的缓存语句，并写入 stats_version=1 的表元数据。
    let mut statement =
        PlanCacheStmt::<()>::new(ast::misc::Prepared::default(), "select * from orders");
    statement.SchemaVersion = 7;
    statement.CollectPlanCacheStmtInfo(
        Vec::new(),
        false,
        vec![PlanCacheTable {
            database: ast::NewCIStr("test"),
            name: ast::NewCIStr("orders"),
            id: 10,
            stats_version: 1,
            ..Default::default()
        }],
    );
    let context = PlanCacheKeyContext {
        invalidate_on_fresh_stats: true,
        ..Default::default()
    };
    let first = NewPlanCacheKey(&context, &statement).unwrap().key.unwrap();

    // 同一语句再收集 stats_version=2，缓存键必须随之变化。
    statement.CollectPlanCacheStmtInfo(
        Vec::new(),
        false,
        vec![PlanCacheTable {
            database: ast::NewCIStr("test"),
            name: ast::NewCIStr("orders"),
            id: 10,
            stats_version: 2,
            ..Default::default()
        }],
    );
    let second = NewPlanCacheKey(&context, &statement).unwrap().key.unwrap();
    assert_ne!(first.AsBytes(), second.AsBytes());
}
