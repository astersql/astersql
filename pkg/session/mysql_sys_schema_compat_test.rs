// Copyright 2026 AsterSQL.

// 验证 AsterSQL 对 MySQL `sys` schema 的窄兼容边界：已实现对象必须按清单暴露，
// 未实现的 MySQL 8 原生视图与例程则必须保持缺失，不能注册为空壳。

use std::collections::BTreeMap;

use crate::runtime::{ConcreteRecordSet, CreateAnalyzeSession};
use crate::testutil::TestRecordSet;

// 结果集采用逐行拉取接口，这里统一收集后再做目录内容断言。
fn collect(mut result: ConcreteRecordSet) -> Vec<Vec<String>> {
    let mut rows = Vec::new();
    while let Some(row) = result.Next().expect("read sys compatibility row") {
        rows.push(row);
    }
    rows
}

// 本文件中的查询都是单语句，只读取会话返回的第一个结果集。
fn execute_rows(session: &crate::runtime::ConcreteSession, sql: &str) -> Vec<Vec<String>> {
    let result = session
        .execute(sql)
        .unwrap_or_else(|error| panic!("sys compatibility query failed: {sql}: {error}"))
        .remove(0);
    collect(result)
}

#[test]
fn sys_schema_and_routine_catalog_respect_compatibility_manifest() {
    use astersql_meta_metadef::{
        MySQL80NativeSysRoutineCount, MySQL80NativeSysViewCount, SysSchemaSupportedObjects,
        UnsupportedMySQL80SysRoutineSamples, UnsupportedMySQL80SysViewSamples,
    };

    let (_domain, session) = CreateAnalyzeSession().expect("canonical sys compatibility session");

    // 名称与对象类型按目录约定规范化，并用有序映射消除展示顺序对清单比较的影响。
    let shown = execute_rows(&session, "show full tables from sys")
        .into_iter()
        .map(|row| (row[0].to_ascii_lowercase(), row[1].to_ascii_uppercase()))
        .collect::<BTreeMap<_, _>>();
    assert_eq!(
        shown,
        SysSchemaSupportedObjects
            .iter()
            .map(|object| (object.name.to_owned(), object.object_type.to_owned()))
            .collect(),
        "sys must expose only implemented Go/MySQL80 compatibility objects",
    );

    // 当前唯一受支持的 sys 视图还必须在 information_schema 中暴露准确的视图属性。
    let views = execute_rows(
        &session,
        "select table_name, check_option, is_updatable, security_type \
         from information_schema.views where table_schema='sys' order by table_name",
    );
    assert_eq!(
        views,
        vec![vec![
            "schema_unused_indexes".to_owned(),
            "NONE".to_owned(),
            "NO".to_owned(),
            "DEFINER".to_owned(),
        ]],
    );

    // 当前兼容边界不实现 sys 例程，因此例程及其参数目录都必须为空。
    assert_eq!(
        execute_rows(
            &session,
            "select count(*) from information_schema.routines where routine_schema='sys'",
        ),
        vec![vec!["0".to_owned()]],
    );
    assert_eq!(
        execute_rows(
            &session,
            "select count(*) from information_schema.parameters where specific_schema='sys'",
        ),
        vec![vec!["0".to_owned()]],
    );

    // 抽样验证未支持视图不仅不出现在清单中，直接解析时也应维持“对象不存在”的边界。
    for name in UnsupportedMySQL80SysViewSamples {
        assert!(
            !shown.contains_key(*name),
            "unsupported MySQL 8 sys view {name} must not be registered as an empty shell",
        );
        let error = match session.execute(&format!("select * from sys.`{name}`")) {
            Ok(_) => panic!("unsupported MySQL 8 sys view {name} must not resolve"),
            Err(error) => error,
        };
        assert!(
            error.to_string().contains("doesn't exist"),
            "unsupported view {name} must retain the missing-object boundary: {error}",
        );
    }

    // 未支持例程同样不能获得占位目录记录。
    let routine_rows = execute_rows(
        &session,
        "select routine_name from information_schema.routines where routine_schema='sys'",
    );
    for name in UnsupportedMySQL80SysRoutineSamples {
        assert!(
            routine_rows.iter().all(|row| row[0] != *name),
            "unsupported MySQL 8 sys routine {name} must not receive a catalog row",
        );
    }

    println!(
        "sys compatibility manifest: supported={} (go=1, mysql80=1), \
         unsupported_samples={} (mysql80 views={}, routines={}); \
         native_mysql80_boundary=100 views/48 routines",
        SysSchemaSupportedObjects.len(),
        UnsupportedMySQL80SysViewSamples.len() + UnsupportedMySQL80SysRoutineSamples.len(),
        UnsupportedMySQL80SysViewSamples.len(),
        UnsupportedMySQL80SysRoutineSamples.len(),
    );
    // 固定 MySQL 8 原生对象总量，防止兼容清单悄然被误解为完整实现范围。
    assert_eq!(
        (MySQL80NativeSysViewCount, MySQL80NativeSysRoutineCount),
        (100, 48)
    );
}
