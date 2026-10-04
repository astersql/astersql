// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//	http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// TTL SQL 拼接器（`SQLBuilder`）的单元测试。
//
// 覆盖分区表上带 TTL 过期条件、ORDER BY 与 LIMIT 的 SELECT 语句生成。

/// 构造带分区名的物理表，断言生成的 SELECT 含 `PARTITION(...)`、TTL 条件与 LIMIT。
#[test]
fn sql_builder_generates_partitioned_select_with_ttl_condition() {
    use crate::{Column, FieldKind, FieldType, PhysicalTable, SQLBuilder};

    let id = Column::new("id", FieldType::new(FieldKind::Int));
    let time = Column::new("created_at", FieldType::new(FieldKind::DateTime));
    let table = PhysicalTable::new(
        "test",
        "events",
        vec![id.clone()],
        time,
        Some("p0".to_owned()),
    )
    .expect("TTL table");
    let mut builder = SQLBuilder::new(&table);
    builder.write_select().expect("select");
    builder.write_expire_condition(1_700_000_000).expect("ttl");
    builder.write_order_by(&[id], false).expect("order");
    builder.write_limit(128).expect("limit");
    let sql = builder.build().expect("sql");
    assert!(sql.contains("FROM `test`.`events` PARTITION(`p0`)"));
    assert!(sql.contains("LIMIT 128"));
}

/// Go distinguishes a missing continuation key from an explicitly empty row:
/// only the former falls back to `rangeStart`.
#[test]
fn scan_generator_does_not_reuse_range_start_for_an_empty_continuation_row() {
    use crate::{Column, Datum, FieldKind, FieldType, NewScanQueryGenerator, PhysicalTable};

    let id = Column::new("id", FieldType::new(FieldKind::Int));
    let table = PhysicalTable::new(
        "test",
        "events",
        vec![id],
        Column::new("created_at", FieldType::new(FieldKind::DateTime)),
        None,
    )
    .expect("TTL table");
    let mut generator =
        NewScanQueryGenerator(&table, 0, vec![Datum::Int(10)], vec![Datum::Int(100)])
            .expect("generator");

    generator.NextSQL(&[], 1).expect("first page");
    let sql = generator
        .NextSQL(&[Vec::new()], 1)
        .expect("explicit empty continuation row");

    assert_eq!(
        sql,
        "SELECT LOW_PRIORITY SQL_NO_CACHE `id` FROM `test`.`events` WHERE `id` < 100 AND `created_at` < CAST('1970-01-01 00:00:00' AS DATETIME) ORDER BY `id` ASC LIMIT 1"
    );
}

fn table_with_keys(key_columns: Vec<Column>) -> PhysicalTable {
    PhysicalTable::new(
        "test",
        "t",
        key_columns,
        Column::new("time", FieldType::new(FieldKind::DateTime)),
        None,
    )
    .expect("TTL table")
}

use crate::{
    BuildDeleteSQL, Column, Datum, FieldKind, FieldType, NewScanQueryGenerator, PhysicalTable,
    SQLBuilder,
};

#[test]
fn sql_builder_matches_go_state_and_delete_safety_contracts() {
    let id = Column::new("id", FieldType::new(FieldKind::Varchar));
    let table = table_with_keys(vec![id.clone()]);

    let mut empty = SQLBuilder::new(&table);
    assert_eq!(
        empty.build().unwrap_err().to_string(),
        "invalid state: writeBegin"
    );

    let mut delete = SQLBuilder::new(&table);
    delete.write_delete().unwrap();
    assert_eq!(
        delete.build().unwrap_err().to_string(),
        "expire condition not write"
    );

    let mut select = SQLBuilder::new(&table);
    select.write_select().unwrap();
    select
        .write_common_condition(&[id], ">", &[Datum::String("a1';'".into())])
        .unwrap();
    select.write_expire_condition(0).unwrap();
    select.write_limit(128).unwrap();
    assert_eq!(
        select.build().unwrap(),
        "SELECT LOW_PRIORITY SQL_NO_CACHE `id` FROM `test`.`t` WHERE `id` > 'a1\\';\\'' AND `time` < CAST('1970-01-01 00:00:00' AS DATETIME) LIMIT 128"
    );
    assert!(select.write_limit(1).is_err());
}

#[test]
fn composite_scan_generator_matches_go_prefix_stack_pagination() {
    let keys = vec![
        Column::new("a", FieldType::new(FieldKind::Int)),
        Column::new("b", FieldType::new(FieldKind::Varchar)),
        Column::new("c", FieldType::binary(FieldKind::String)),
    ];
    let table = table_with_keys(keys);
    let mut generator = NewScanQueryGenerator(
        &table,
        0,
        vec![
            Datum::Int(1),
            Datum::String("x".into()),
            Datum::Bytes(vec![0x0e]),
        ],
        vec![
            Datum::Int(100),
            Datum::String("z".into()),
            Datum::Bytes(vec![0xff]),
        ],
    )
    .unwrap();

    assert_eq!(
        generator.NextSQL(&[], 5).unwrap(),
        "SELECT LOW_PRIORITY SQL_NO_CACHE `a`, `b`, `c` FROM `test`.`t` WHERE `a` = 1 AND `b` = 'x' AND `c` >= x'0e' AND (`a`, `b`, `c`) < (100, 'z', x'ff') AND `time` < CAST('1970-01-01 00:00:00' AS DATETIME) ORDER BY `a`, `b`, `c` ASC LIMIT 5"
    );
    let mut full_page = vec![Vec::new(); 5];
    full_page[4] = vec![
        Datum::Int(1),
        Datum::String("y".into()),
        Datum::Bytes(vec![0x0a]),
    ];
    assert_eq!(
        generator.NextSQL(&full_page, 5).unwrap(),
        "SELECT LOW_PRIORITY SQL_NO_CACHE `a`, `b`, `c` FROM `test`.`t` WHERE `a` = 1 AND `b` = 'y' AND `c` > x'0a' AND (`a`, `b`, `c`) < (100, 'z', x'ff') AND `time` < CAST('1970-01-01 00:00:00' AS DATETIME) ORDER BY `a`, `b`, `c` ASC LIMIT 5"
    );
}

#[test]
fn build_delete_sql_matches_go_composite_key_contract() {
    let table = table_with_keys(vec![
        Column::new("a", FieldType::new(FieldKind::Int)),
        Column::new("b", FieldType::new(FieldKind::Varchar)),
    ]);
    let rows = vec![
        vec![Datum::Int(1), Datum::String("a".into())],
        vec![Datum::Int(2), Datum::String("b".into())],
    ];
    assert_eq!(
        BuildDeleteSQL(&table, &rows, 0).unwrap(),
        "DELETE LOW_PRIORITY FROM `test`.`t` WHERE (`a`, `b`) IN ((1, 'a'), (2, 'b')) AND `time` < CAST('1970-01-01 00:00:00' AS DATETIME) LIMIT 2"
    );
    assert_eq!(
        BuildDeleteSQL(&table, &[], 0).unwrap_err().to_string(),
        "Cannot build delete SQL with empty rows"
    );
}

#[test]
fn expiration_predicate_distinguishes_timestamp_from_wall_clock_types() {
    for kind in [FieldKind::Timestamp, FieldKind::DateTime, FieldKind::Date] {
        let table = PhysicalTable::new(
            "test",
            "times",
            vec![Column::new("id", FieldType::new(FieldKind::Int))],
            Column::new("expires", FieldType::new(kind)),
            None,
        )
        .unwrap();
        let sql = BuildDeleteSQL(&table, &[vec![Datum::Int(1)]], 1_730_615_400).unwrap();
        let expected = if kind == FieldKind::Timestamp {
            "FROM_UNIXTIME(1730615400)"
        } else {
            "CAST('2024-11-03 06:30:00' AS DATETIME)"
        };
        assert!(sql.contains(expected), "{kind:?}: {sql}");
    }
}

#[test]
fn captured_offset_is_shared_by_select_and_delete_expiration() {
    use crate::ExpireTime;
    for (offset, wall) in [
        (-14400, "2024-11-03 02:30:00"),
        (-18000, "2024-11-03 01:30:00"),
        (19800, "2024-11-03 12:00:00"),
        (28800, "2024-11-03 14:30:00"),
    ] {
        for kind in [FieldKind::Timestamp, FieldKind::DateTime, FieldKind::Date] {
            let table = PhysicalTable::new(
                "test",
                "times",
                vec![Column::new("id", FieldType::new(FieldKind::Int))],
                Column::new("expires", FieldType::new(kind)),
                None,
            )
            .unwrap();
            let expire = ExpireTime {
                unix_seconds: 1730615400,
                utc_offset_seconds: offset,
            };
            let expected = if kind == FieldKind::Timestamp {
                "FROM_UNIXTIME(1730615400)".into()
            } else {
                format!("CAST('{wall}' AS DATETIME)")
            };
            let sql = BuildDeleteSQL(&table, &[vec![Datum::Int(1)]], expire).unwrap();
            assert!(sql.contains(&expected), "{sql}");
            let mut generator = NewScanQueryGenerator(&table, expire, vec![], vec![]).unwrap();
            assert!(generator.NextSQL(&[], 1).unwrap().contains(&expected));
        }
    }
}
