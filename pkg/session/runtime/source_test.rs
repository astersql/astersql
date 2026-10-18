// Copyright 2026 AsterSQL.

use super::*;

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
