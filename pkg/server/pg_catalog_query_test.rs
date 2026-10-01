// Copyright 2026 AsterSQL.
use crate::pg_catalog_query::{CastType, Expr, parse};

#[test]
fn catalog_select_structure() {
    let select = parse(crate::pg_catalog::DATABASES_SQL).unwrap().unwrap();
    assert_eq!(select.projections.len(), 6);
    assert_eq!(select.from.name, "pg_database");
    assert_eq!(select.from.alias, "n");
    assert_eq!(select.join.unwrap().relation.name, "pg_shdescription");
    assert!(matches!(select.order[0].expr, Expr::Case { .. }));
    let transactions = parse(crate::pg_catalog::TRANSACTIONS_SQL).unwrap().unwrap();
    assert!(matches!(
        transactions.projections[0].expr,
        Expr::Cast(_, CastType::Bigint)
    ));
    assert!(matches!(transactions.filter, Some(Expr::NotNull(_))));
    assert!(transactions.order[0].descending);
    assert_eq!(transactions.limit, Some(1));
    let namespace = parse("select N.oid::bigint as id, N.xmin as state_number, nspname as name, D.description, pg_catalog.pg_get_userbyid(N.nspowner) as \"owner\" from pg_catalog.pg_namespace N left join pg_catalog.pg_description D on N.oid = D.objoid order by case when nspname = pg_catalog.current_schema() then -1::bigint else N.oid::bigint end").unwrap().unwrap();
    assert_eq!(namespace.from.name, "pg_namespace");
    assert_eq!(namespace.projections.len(), 5);
    assert_eq!(
        parse("select oid, spcname from pg_catalog.pg_tablespace")
            .unwrap()
            .unwrap()
            .from
            .name,
        "pg_tablespace"
    );
}

#[test]
fn catalog_projection_syntax_and_boundaries() {
    let select = parse("/* outer /* nested */ */ SeLeCt datname AS \"Database Name\", NULL missing, 'pg_catalog.pg_database' literal, oid::varchar::bigint id FROM \"pg_catalog\".\"pg_database\" AS n ORDER BY oid DESC LIMIT 2; -- done").unwrap().unwrap();
    assert_eq!(select.projections[0].name, "Database Name");
    assert_eq!(select.projections[1].name, "missing");
    assert_eq!(select.limit, Some(2));
    assert_eq!(parse("select 'from pg_catalog.pg_database'").unwrap(), None);
    assert_eq!(
        parse("select 1 /* from pg_catalog.pg_database */").unwrap(),
        None
    );
    for sql in [
        "select oid from pg_catalog.pg_database; select 1",
        "select * from pg_catalog.pg_database",
        "select distinct oid from pg_catalog.pg_database",
        "select oid::numeric from pg_catalog.pg_database",
        "select oid + 1 from pg_catalog.pg_database",
        "select oid from pg_catalog.pg_database union select 1",
    ] {
        assert_eq!(parse(sql).unwrap_err().0, "0A000", "{sql}");
    }
    for sql in [
        "select from pg_catalog.pg_database",
        "select oid, from pg_catalog.pg_database",
        "select oid from pg_catalog.pg_database limit",
        "select oid from pg_catalog.pg_database /*",
        "select oid as \"unfinished from pg_catalog.pg_database",
    ] {
        assert_eq!(parse(sql).unwrap_err().0, "42601", "{sql}");
    }
}

#[test]
fn catalog_binding_has_explicit_support() {
    use crate::pg_catalog::CatalogQuery;
    for sql in [
        "select unknown from pg_catalog.pg_database",
        "select x.oid from pg_catalog.pg_database n",
        "select pg_catalog.unknown(oid) from pg_catalog.pg_database",
        "select oid from pg_catalog.pg_database where oid",
        "select datistemplate::bigint from pg_catalog.pg_database",
        "select current_database() from pg_catalog.pg_locks",
        "select pg_catalog.age(transactionid) from pg_catalog.pg_locks",
    ] {
        assert_eq!(CatalogQuery::parse(sql).unwrap_err().0, "0A000", "{sql}");
    }
    assert!(
        CatalogQuery::parse(
            "select datname name, oid id from pg_catalog.pg_database order by oid limit 2"
        )
        .unwrap()
        .is_some()
    );
}

#[test]
fn catalog_parser_bounds_expression_depth() {
    let sql = format!(
        "select {}oid{} from pg_catalog.pg_database",
        "(".repeat(100),
        ")".repeat(100)
    );
    assert_eq!(parse(&sql).unwrap_err().0, "0A000");
    let sql = format!(
        "select oid{} from pg_catalog.pg_database",
        "::bigint".repeat(100)
    );
    assert_eq!(parse(&sql).unwrap_err().0, "0A000");
}
