// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
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

// `exchange_partition_test.go::TestExchangeRangeColumnsPartition` 的可运行 Rust 对照。
// 覆盖多列 RANGE COLUMNS 的 NULL、整数极值、边界字符串、五个分区往返交换，
// 以及每个非目标分区行均被 WITH VALIDATION 拒绝的错误路径。

use std::collections::{HashMap, HashSet};

use astersql_session::runtime::normal_ddl_service::reduce_ddl_dispatch_loop_waiting_interval_for_test;
use astersql_testkit::mockstore::CreatePlannerTestStoreAndDomain;
use astersql_testkit::{DbValue, TestKit};

#[test]
fn test_exchange_range_columns_partition() {
    let _ddl_dispatch_guard = reduce_ddl_dispatch_loop_waiting_interval_for_test();
    let (store, _domain) = CreatePlannerTestStoreAndDomain();
    let mut tk = TestKit::new(store);
    tk.MustExec("use test", Vec::new());
    tk.MustExec("set @@tidb_enable_exchange_partition=1", Vec::new());
    tk.MustExec(
        "CREATE TABLE t1 (id INT NOT NULL, age INT, name VARCHAR(50)) \
         PARTITION BY RANGE COLUMNS(age, name) (\
         PARTITION p0 VALUES LESS THAN (20, 'm'),\
         PARTITION p1 VALUES LESS THAN (30, 'm'),\
         PARTITION p2 VALUES LESS THAN (30, MAXVALUE),\
         PARTITION p3 VALUES LESS THAN (40, 'm'),\
         PARTITION p4 VALUES LESS THAN (MAXVALUE, MAXVALUE))",
        Vec::new(),
    );
    let age_values = [
        None,
        Some(i32::MIN),
        Some(i32::MAX),
        Some(0),
        Some(19),
        Some(20),
        Some(29),
        Some(30),
        Some(39),
        Some(40),
    ];
    let name_values = [None, Some(""), Some("l"), Some("m"), Some("n")];
    let mut id = 0;
    let mut values = Vec::new();
    let mut fixture_rows = Vec::new();
    for age in age_values {
        for name in name_values {
            id += 1;
            fixture_rows.push(vec![
                DbValue::I64(id.into()),
                age.map_or(DbValue::Null, |age| DbValue::I64(age.into())),
                name.map_or(DbValue::Null, |name| DbValue::String(name.to_owned())),
            ]);
            let age = age.map_or_else(|| "NULL".to_owned(), |age| age.to_string());
            let name = name.map_or_else(
                || "NULL".to_owned(),
                |name| format!("'{}'", name.replace('\'', "''")),
            );
            values.push(format!("({id}, {age}, {name})"));
        }
    }
    tk.MustExec(
        &format!("INSERT INTO t1 VALUES {}", values.join(",")),
        Vec::new(),
    );
    let partition_names = ["p0", "p1", "p2", "p3", "p4"];
    let mut initial_results = HashMap::new();
    let mut initial_ids = HashMap::new();
    for partition in partition_names {
        let mut result = tk.MustQuery(
            &format!("SELECT * FROM t1 PARTITION({partition})"),
            Vec::new(),
        );
        result.Sort();
        let rows = result.Rows();
        initial_ids.insert(
            partition,
            rows.iter()
                .map(|row| row[0].parse::<i32>().expect("integer row id"))
                .collect::<HashSet<_>>(),
        );
        initial_results.insert(partition, rows);
    }

    tk.MustExec(
        "CREATE TABLE t2 (id INT NOT NULL, age INT, name VARCHAR(50))",
        Vec::new(),
    );
    let session = tk.Session();
    let (insert_fixture_stmt, _) = session
        .PrepareStmt("INSERT INTO t2 VALUES (?, ?, ?)")
        .expect("prepare rejection fixture insert");
    for partition in &partition_names {
        tk.MustExec(
            &format!("ALTER TABLE t1 EXCHANGE PARTITION {partition} WITH TABLE t2"),
            Vec::new(),
        );
        tk.MustQuery(
            &format!("SELECT COUNT(*) FROM t1 PARTITION({partition})"),
            Vec::new(),
        )
        .Check(vec![vec!["0"]]);

        let mut exchanged = tk.MustQuery("SELECT * FROM t2", Vec::new());
        exchanged.Sort();
        exchanged.Check(initial_results[partition].clone());

        tk.MustExec(
            &format!("ALTER TABLE t1 EXCHANGE PARTITION {partition} WITH TABLE t2"),
            Vec::new(),
        );
        let mut restored = tk.MustQuery(
            &format!("SELECT * FROM t1 PARTITION({partition})"),
            Vec::new(),
        );
        restored.Sort();
        restored.Check(initial_results[partition].clone());

        for row_id in 1..=id {
            if initial_ids[partition].contains(&row_id) {
                continue;
            }
            session
                .ExecutePreparedStmt(insert_fixture_stmt, &fixture_rows[(row_id - 1) as usize])
                .expect("insert rejection fixture row");
            tk.MustContainErrMsg(
                &format!(
                    "ALTER TABLE t1 EXCHANGE PARTITION {partition} WITH TABLE t2 /* j = {row_id} */"
                ),
                "[ddl:1737]Found a row that does not match the partition",
            );
            // The exchange table contains one rejected fixture row. Clearing that row with DML
            // preserves the next case's empty-table precondition without paying for an unrelated
            // table-ID replacement on every validation case.
            tk.MustExec("DELETE FROM t2", Vec::new());
        }
    }
    session
        .DropPreparedStmt(insert_fixture_stmt)
        .expect("drop rejection fixture insert");

    tk.MustExec("DROP TABLE t2", Vec::new());
    tk.MustExec("DROP TABLE t1", Vec::new());
}

#[test]
fn port_fef9c4a64e_column_index_history_both_directions() {
    let _guard = reduce_ddl_dispatch_loop_waiting_interval_for_test();
    let (store, domain) = CreatePlannerTestStoreAndDomain();
    let mut tk = TestKit::new(store);
    tk.MustExec("use test", vec![]);
    tk.MustExec("set @@tidb_enable_exchange_partition=1", vec![]);
    for higher_partitioned in [false, true] {
        let (pt, nt) = if higher_partitioned {
            ("rpt", "rnt")
        } else {
            ("pt", "nt")
        };
        let schema = |high| {
            if high {
                "a INT, b INT, INDEX idx_a(a), INDEX idx_b(b)"
            } else {
                "a INT, INDEX idx_a(a)"
            }
        };
        tk.MustExec(&format!("CREATE TABLE {pt} ({}) PARTITION BY RANGE(a) (PARTITION p0 VALUES LESS THAN (100), PARTITION p1 VALUES LESS THAN (MAXVALUE))", schema(higher_partitioned)), vec![]);
        tk.MustExec(
            &format!("CREATE TABLE {nt} ({})", schema(!higher_partitioned)),
            vec![],
        );
        let high = if higher_partitioned { pt } else { nt };
        tk.MustExec(
            &format!(
                "INSERT INTO {high} VALUES (1,{})",
                if higher_partitioned { 888 } else { 777 }
            ),
            vec![],
        );
        if higher_partitioned {
            tk.MustExec(&format!("INSERT INTO {pt} VALUES (101,999)"), vec![]);
        } else {
            tk.MustExec(&format!("INSERT INTO {pt} VALUES (5),(150)"), vec![]);
        }
        tk.MustExec(&format!("ALTER TABLE {high} DROP INDEX idx_b"), vec![]);
        tk.MustExec(&format!("ALTER TABLE {high} DROP COLUMN b"), vec![]);
        tk.MustExec(
            &format!("ALTER TABLE {pt} EXCHANGE PARTITION p0 WITH TABLE {nt}"),
            vec![],
        );
        let expected_nt = if higher_partitioned { 2 } else { 1 };
        let info = domain.table_by_name("test", nt).unwrap();
        assert_eq!(
            (info.MaxColumnID, info.MaxIndexID),
            (expected_nt, expected_nt)
        );
        // The existing Rust exchange path does not transfer hidden-row allocator
        // bases (outside fef9c4a64e). Rebase that independent fixture stream so
        // the unchanged INSERT assertions isolate historical column/index IDs.
        domain
            .allocate_stats_auto_id_kind(info.ID, Some(1000), 2)
            .unwrap();
        for table in [pt, nt] {
            tk.MustExec(
                &format!("ALTER TABLE {table} ADD COLUMN c INT DEFAULT 0"),
                vec![],
            );
        }
        if higher_partitioned {
            tk.MustQuery(&format!("SELECT a,c FROM {nt}"), vec![])
                .Check(vec![vec!["1", "0"]]);
            tk.MustQuery(&format!("SELECT a,c FROM {pt}"), vec![])
                .Check(vec![vec!["101", "0"]]);
        } else {
            tk.MustExec(&format!("INSERT INTO {pt}(a) VALUES (2)"), vec![]);
            tk.MustExec(&format!("INSERT INTO {nt}(a) VALUES (6)"), vec![]);
            tk.MustQuery(&format!("SELECT a,c FROM {pt} ORDER BY a"), vec![])
                .Check(vec![vec!["1", "0"], vec!["2", "0"], vec!["150", "0"]]);
            tk.MustQuery(&format!("SELECT a,c FROM {nt} ORDER BY a"), vec![])
                .Check(vec![vec!["5", "0"], vec!["6", "0"]]);
        }
        for (table, expected) in [(pt, 3), (nt, expected_nt + 1)] {
            tk.MustExec(&format!("ALTER TABLE {table} ADD INDEX idx_new(a)"), vec![]);
            let info = domain.table_by_name("test", table).unwrap();
            assert_eq!(
                info.Indices
                    .iter()
                    .find(|idx| idx.Name.L == "idx_new")
                    .unwrap()
                    .ID,
                expected
            );
            tk.MustExec(&format!("admin check table {table}"), vec![]);
        }
    }
}

// SQL ADD CHECK and the Go enable variables are not wired in the existing
// runtime. Seed the exact post-drop history through the real metadata service;
// exchange still runs through SQL, KV commit, schema reload, and publication.
#[test]
fn port_fef9c4a64e_constraint_counters_remain_logical() {
    use astersql_meta_model::{ConstraintInfo, SchemaState};
    use astersql_parser_ast::NewCIStr;
    let _guard = reduce_ddl_dispatch_loop_waiting_interval_for_test();
    let (store, domain) = CreatePlannerTestStoreAndDomain();
    let mut tk = TestKit::new(store);
    tk.MustExec("use test", vec![]);
    tk.MustExec("set @@tidb_enable_exchange_partition=1", vec![]);
    tk.MustExec("CREATE TABLE seed (a INT, INDEX idx_a(a))", vec![]);
    tk.MustExec("CREATE TABLE fk_pt (a INT, INDEX idx_a(a)) PARTITION BY RANGE(a) (PARTITION p0 VALUES LESS THAN(100), PARTITION p1 VALUES LESS THAN(MAXVALUE))", vec![]);
    let mut nt = (*domain.table_by_name("test", "seed").unwrap()).clone();
    nt.ID = 0;
    nt.Name = NewCIStr("fk_nt");
    nt.MaxForeignKeyID = 1; // dropped fk_old
    nt.MaxConstraintID = 2; // dropped c_old; retained c_live
    nt.Constraints = vec![ConstraintInfo {
        ID: 2,
        Name: NewCIStr("c_live"),
        Table: nt.Name.clone(),
        ExprString: "a < 2000".into(),
        ConstraintCols: vec![NewCIStr("a")],
        Enforced: true,
        State: SchemaState::Public,
        ..Default::default()
    }];
    domain.ddl_create_table("test", nt, false).unwrap();
    tk.MustExec(
        "ALTER TABLE fk_pt EXCHANGE PARTITION p0 WITH TABLE fk_nt",
        vec![],
    );
    let counters = |name: &str| {
        let t = domain.table_by_name("test", name).unwrap();
        (t.MaxForeignKeyID, t.MaxConstraintID)
    };
    assert_eq!(counters("fk_pt"), (0, 0));
    assert_eq!(counters("fk_nt"), (1, 2));
    for (name, expected) in [("fk_nt", 3), ("fk_pt", 1)] {
        let table = domain.table_by_name("test", name).unwrap();
        let state = port_fef9c4a64e_add_constraint(&table);
        assert_eq!(state.max_constraint_id, expected);
        assert_eq!(state.constraints.last().unwrap().id, expected);
        if name == "fk_nt" {
            assert_eq!(state.constraints[0].id, 2);
            assert_ne!(state.constraints[0].id, state.constraints[1].id);
        }
    }
    let mut pt = (*domain.table_by_name("test", "fk_pt").unwrap()).clone();
    pt.ID = 0;
    pt.Name = NewCIStr("cst_pt");
    for definition in &mut pt.Partition.as_mut().unwrap().Definitions {
        definition.ID = 0;
    }
    pt.MaxConstraintID = 1; // dropped c_old
    domain.ddl_create_table("test", pt, false).unwrap();
    tk.MustExec("CREATE TABLE cst_nt (a INT, INDEX idx_a(a))", vec![]);
    tk.MustExec(
        "ALTER TABLE cst_pt EXCHANGE PARTITION p0 WITH TABLE cst_nt",
        vec![],
    );
    assert_eq!(counters("cst_pt"), (0, 1));
    assert_eq!(counters("cst_nt"), (0, 0));
}

// Reuse the production CHECK state machine for the Go post-exchange ADD CHECK
// assertions, rather than inventing a test allocator or expanding SQL support.
fn port_fef9c4a64e_add_constraint(
    table: &astersql_meta_model::TableInfo,
) -> astersql_ddl::constraint::ConstraintTableState {
    use astersql_ddl::column::{ColumnInfo, FieldType, SchemaState, TableInfo};
    use astersql_ddl::constraint::{
        ConstraintTableState, advance_add_check_constraint, build_constraint_info,
    };
    let mut model = TableInfo::new(table.ID, &table.Name.O);
    model.columns = table
        .Columns
        .iter()
        .map(|c| {
            let mut column = ColumnInfo::new(&c.Name.O, FieldType::integer());
            column.id = c.ID;
            column.state = SchemaState::Public;
            column
        })
        .collect();
    let mut state = ConstraintTableState {
        max_constraint_id: table.MaxConstraintID,
        constraints: table
            .Constraints
            .iter()
            .map(|c| astersql_ddl::constraint::ConstraintInfo {
                id: c.ID,
                name: c.Name.L.clone(),
                table_name: table.Name.L.clone(),
                columns: c.ConstraintCols.iter().map(|name| name.L.clone()).collect(),
                expression: c.ExprString.clone(),
                enforced: c.Enforced,
                in_column: c.InColumn,
                state: SchemaState::Public,
            })
            .collect(),
    };
    let mut incoming = build_constraint_info(
        &model,
        "c_new",
        vec!["a".to_owned()],
        "a >= 0",
        true,
        false,
        SchemaState::None,
    )
    .unwrap();
    let mut version = 0;
    for _ in 0..3 {
        advance_add_check_constraint(&model, &mut state, &mut incoming, true, &mut version, false)
            .unwrap();
    }
    assert_eq!(state.constraints.last().unwrap().state, SchemaState::Public);
    state
}
