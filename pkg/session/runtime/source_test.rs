// Copyright 2026 AsterSQL.

use super::*;
use crate::runtime::{ConcreteSession, CreateAnalyzeSession};
use astersql_parser_ast as ast;

#[test]
fn distinct_is_applied_before_order_by_and_limit() {
    let (_domain, session) =
        crate::runtime::CreateAnalyzeSession().expect("create source parity session");
    let mut result = session
        .execute(
            "select distinct value \
             from (select 1 value union all select 1 union all select 2) source_rows \
             order by value limit 2",
        )
        .expect("execute DISTINCT ORDER BY LIMIT")
        .remove(0);

    assert_eq!(
        result.Next().expect("read first distinct row"),
        Some(vec!["1".to_owned()])
    );
    assert_eq!(
        result.Next().expect("read second distinct row"),
        Some(vec!["2".to_owned()])
    );
    assert_eq!(result.Next().expect("distinct rows exhausted"), None);

    let mut aggregate = session
        .execute(
            "select distinct count(*) \
             from (select 1 group_id union all select 2 union all \
                   select 3 union all select 3) source_rows \
             group by group_id order by count(*) limit 2",
        )
        .expect("execute aggregate DISTINCT ORDER BY LIMIT")
        .remove(0);
    assert_eq!(
        aggregate.Next().expect("read first aggregate row"),
        Some(vec!["1".to_owned()])
    );
    assert_eq!(
        aggregate.Next().expect("read second aggregate row"),
        Some(vec!["2".to_owned()])
    );
    assert_eq!(aggregate.Next().expect("aggregate rows exhausted"), None);
}

#[test]
fn mixed_type_equality_join_preserves_coercion_duplicates_and_outer_rows() {
    use crate::testutil::TestRecordSet;
    let (_domain, session) = crate::runtime::CreateAnalyzeSession().unwrap();
    for sql in [
        "create table mixed_left(id int, k varchar(64))",
        "create table mixed_right(id int, k double)",
        "insert into mixed_left values(1,'0'),(2,'-0.000'),(3,'text'),(4,'1tail'),(5,'9007199254740993'),(6,null)",
        "insert into mixed_right values(10,0),(11,0),(12,1),(13,9007199254740992),(14,null)",
    ] {
        session.execute(sql).unwrap();
    }
    let query = |sql: &str| {
        let mut result = session.execute(sql).unwrap().remove(0);
        let mut rows = Vec::new();
        while let Some(row) = result.Next().unwrap() {
            rows.push(row);
        }
        result.Close().unwrap();
        rows.sort();
        rows
    };
    for (join, count) in [("join", 8), ("left join", 9), ("right join", 9)] {
        let expected = query(&format!(
            "select l.id,r.id from mixed_left l {join} mixed_right r on not(l.k<>r.k)"
        ));
        assert_eq!(
            expected.len(),
            count,
            "{join} must retain SQL numeric coercion and NULL outer rows"
        );
        for condition in ["l.k=r.k", "r.k=l.k"] {
            assert_eq!(
                query(&format!(
                    "select l.id,r.id from mixed_left l {join} mixed_right r on {condition}"
                )),
                expected,
                "{join} {condition}"
            );
        }
    }
    // Bracketed text remains on the vector-aware comparison fallback.
    session
        .execute("insert into mixed_left values(7,'[1,2]')")
        .unwrap();
    assert_eq!(
        query("select l.id,r.id from mixed_left l join mixed_right r on l.k=r.k"),
        query("select l.id,r.id from mixed_left l join mixed_right r on not(l.k<>r.k)")
    );
}

/// Rejected non-equality candidates must not allocate a joined row for every pair.
#[test]
fn integer_non_equi_join_rejects_candidates_before_materialization() {
    use crate::testutil::TestRecordSet;
    let (_domain, session) = crate::runtime::CreateAnalyzeSession().unwrap();
    session
        .execute("create table range_left(k bigint)")
        .unwrap();
    session
        .execute("create table range_right(k bigint)")
        .unwrap();
    for (table, offset) in [("range_left", 0), ("range_right", 1000)] {
        let values = (0..256)
            .map(|n| format!("({})", n + offset))
            .collect::<Vec<_>>()
            .join(",");
        session
            .execute(&format!("insert into {table} values {values}"))
            .unwrap();
    }
    session.execute("prepare st from 'select count(*) from range_left l join range_right r on l.k>r.k where l.k<=?'").unwrap();
    session.execute("set @max=255").unwrap();
    for _ in 0..128 {
        for sql in [
            "select count(*) from range_left l join range_right r on l.k>r.k where l.k<=255",
            "execute st using @max",
        ] {
            let mut result = session.execute(sql).unwrap().remove(0);
            assert_eq!(result.Next().unwrap(), Some(vec!["0".to_owned()]));
            assert_eq!(result.Next().unwrap(), None);
            result.Close().unwrap();
        }
    }
}

#[test]
fn integer_non_equi_join_preserves_exact_comparison_and_outer_rows() {
    use crate::testutil::TestRecordSet;
    let (_domain, session) = crate::runtime::CreateAnalyzeSession().unwrap();
    for sql in [
        "create table integer_left(id int,k bigint)",
        "create table integer_right(id int,k bigint unsigned)",
        "insert into integer_left values(1,-1),(2,0),(3,9007199254740993),(4,null),(5,0)",
        "insert into integer_right values(10,0),(11,9007199254740992),(12,18446744073709551615),(13,null)",
    ] {
        session.execute(sql).unwrap();
    }
    let query = |sql: &str| {
        let mut result = session.execute(sql).unwrap().remove(0);
        let mut rows = Vec::new();
        while let Some(row) = result.Next().unwrap() {
            rows.push(row);
        }
        result.Close().unwrap();
        rows.sort();
        rows
    };
    for (condition, fallback) in [
        ("l.k>r.k", "not(l.k<=r.k)"),
        ("l.k>=r.k", "not(l.k<r.k)"),
        ("l.k<r.k", "not(l.k>=r.k)"),
        ("l.k<=r.k", "not(l.k>r.k)"),
        ("r.k<l.k", "not(l.k<=r.k)"),
        ("r.k<=l.k", "not(l.k<r.k)"),
        ("r.k>l.k", "not(l.k>=r.k)"),
        ("r.k>=l.k", "not(l.k>r.k)"),
    ] {
        for join in ["join", "left join", "right join"] {
            assert_eq!(
                query(&format!(
                    "select l.id,r.id from integer_left l {join} integer_right r on {condition}"
                )),
                query(&format!(
                    "select l.id,r.id from integer_left l {join} integer_right r on {fallback}"
                )),
                "{join} {condition}"
            );
        }
    }
    assert_eq!(
        query("select l.id,r.id from integer_left l join integer_right r on l.k>r.k"),
        vec![
            vec!["3".to_owned(), "10".to_owned()],
            vec!["3".to_owned(), "11".to_owned()]
        ]
    );
    session
        .execute("alter table integer_left modify column k varchar(64)")
        .unwrap();
    assert_eq!(
        query("select l.id,r.id from integer_left l left join integer_right r on l.k>r.k"),
        query("select l.id,r.id from integer_left l left join integer_right r on not(l.k<=r.k)")
    );
}

#[test]
fn pg13_025_view_binding_uses_shared_executor() {
    let (domain, _) = CreateAnalyzeSession().unwrap();
    let session = ConcreteSession::new_with_dialect(domain.clone(), ast::SqlDialect::PostgreSQL);
    session
        .execute("CREATE TABLE view_binding_base(id integer,label text)")
        .unwrap();
    session
        .execute("INSERT INTO view_binding_base VALUES(1,'one')")
        .unwrap();
    session
        .execute("CREATE VIEW view_binding AS SELECT id,label FROM view_binding_base WHERE id>0")
        .unwrap();
    let (_, before) = domain.stats_table("test", "view_binding").unwrap();
    assert!(before.PostgreSQLView.is_some());
    assert_eq!(
        session
            .describe_result_fields("SELECT * FROM view_binding")
            .unwrap()[0]
            .column
            .FieldType
            .GetType(),
        astersql_parser_mysql::r#type::TypeLong
    );
    assert!(
        session
            .execute(
                "CREATE OR REPLACE VIEW view_binding AS SELECT id+1 AS n FROM view_binding_base"
            )
            .is_err()
    );
    let (_, after) = domain.stats_table("test", "view_binding").unwrap();
    assert_eq!(
        before.ID, after.ID,
        "unsupported replacement must not drop the original"
    );
    session
        .execute("ALTER TABLE view_binding_base RENAME COLUMN label TO renamed")
        .unwrap();
    let mut rows = session.execute("SELECT * FROM view_binding").unwrap();
    assert_eq!(rows[0].next_row().unwrap().unwrap(), ["1", "one"]);
    session.execute("DROP TABLE view_binding_base").unwrap();
    session
        .execute("CREATE TABLE view_binding_base(id integer,label text)")
        .unwrap();
    session
        .execute("INSERT INTO view_binding_base VALUES(77,'replacement')")
        .unwrap();
    assert!(session.execute("SELECT * FROM view_binding").is_err());
    session.execute("DROP VIEW view_binding").unwrap();
    session.execute("DROP TABLE view_binding_base").unwrap();
}

#[test]
fn pg13_025_unsupported_view_predicates_do_not_publish() {
    let (domain, _) = CreateAnalyzeSession().unwrap();
    let session = ConcreteSession::new_with_dialect(domain.clone(), ast::SqlDialect::PostgreSQL);
    session
        .execute("CREATE TABLE view_predicate_base(id integer,label text)")
        .unwrap();
    for condition in ["id", "label='A'", "id+1>0"] {
        let sql = format!(
            "CREATE VIEW bad_view_predicate AS SELECT id FROM view_predicate_base WHERE {condition}"
        );
        assert!(session.execute(&sql).is_err(), "{sql}");
        assert!(domain.stats_table("test", "bad_view_predicate").is_none());
    }
    session.execute("DROP TABLE view_predicate_base").unwrap();
}

#[test]
fn pg13_033_view_columns_do_not_inherit_not_null_constraints() {
    let (domain, _) = CreateAnalyzeSession().unwrap();
    let pg = ConcreteSession::new_with_dialect(domain.clone(), ast::SqlDialect::PostgreSQL);
    pg.execute("CREATE TABLE pg33_nullable_base(id integer NOT NULL)")
        .unwrap();
    pg.execute("CREATE VIEW pg33_nullable_view AS SELECT id FROM pg33_nullable_base")
        .unwrap();
    let (_, base) = domain.stats_table("test", "pg33_nullable_base").unwrap();
    let (_, view) = domain.stats_table("test", "pg33_nullable_view").unwrap();
    assert!(astersql_parser_mysql::r#type::HasNotNullFlag(
        base.Columns[0].GetFlag()
    ));
    assert!(!astersql_parser_mysql::r#type::HasNotNullFlag(
        view.Columns[0].GetFlag()
    ));
    pg.execute("DROP VIEW pg33_nullable_view").unwrap();
    pg.execute("DROP TABLE pg33_nullable_base").unwrap();
    println!("pg13_033 view nullability: base_not_null=1 view_nullable=1 cleanup=2");
}

#[test]
fn pg13_034_numeric_declarations_validate_before_publication() {
    let (domain, _) = CreateAnalyzeSession().unwrap();
    let pg = ConcreteSession::new_with_dialect(domain.clone(), ast::SqlDialect::PostgreSQL);
    for declaration in ["numeric(0,0)", "numeric(1001,0)", "numeric(5,1001)"] {
        let version = domain.info_schema().SchemaMetaVersion();
        let sql = format!("CREATE TABLE pg34_invalid_numeric(v {declaration})");
        assert!(pg.execute(&sql).is_err(), "{sql}");
        assert!(domain.stats_table("test", "pg34_invalid_numeric").is_none());
        assert_eq!(domain.info_schema().SchemaMetaVersion(), version);
    }
    for declaration in ["numeric", "numeric(5,-2)", "numeric(1000,1000)"] {
        pg.execute(&format!("CREATE TABLE pg34_valid_numeric(v {declaration})"))
            .unwrap();
        pg.execute("DROP TABLE pg34_valid_numeric").unwrap();
    }
    println!(
        "pg13_034 numeric declaration: rejected=3 accepted=3 no_invalid_publication cleanup=3"
    );
}

#[test]
fn pg13_034_plain_view_preserves_column_declarations() {
    let (domain, _) = CreateAnalyzeSession().unwrap();
    let pg = ConcreteSession::new_with_dialect(domain.clone(), ast::SqlDialect::PostgreSQL);
    pg.execute("CREATE TABLE pg34_view_base(label varchar(7),stamp timestamp(3),amount numeric(12,3),iv interval day to second(2))").unwrap();
    pg.execute("CREATE VIEW pg34_view AS SELECT label,stamp,amount,iv FROM pg34_view_base")
        .unwrap();
    let (_, base) = domain.stats_table("test", "pg34_view_base").unwrap();
    let (_, view) = domain.stats_table("test", "pg34_view").unwrap();
    let declaration = |c: &astersql_meta_model::ColumnInfo| {
        (
            c.GetType(),
            c.GetFlen(),
            c.GetDecimal(),
            c.FieldType.PostgreSQLTextOid(),
            c.FieldType.IsPostgreSQLDeclaration(),
        )
    };
    assert_eq!(
        base.Columns.iter().map(declaration).collect::<Vec<_>>(),
        view.Columns.iter().map(declaration).collect::<Vec<_>>()
    );
    assert!(view.PostgreSQLView.is_some());
    pg.execute("DROP VIEW pg34_view").unwrap();
    pg.execute("DROP TABLE pg34_view_base").unwrap();
    println!("pg13_034 plain view declarations: preserved=4 cleanup=2");
}
