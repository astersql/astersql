// Copyright 2026 AsterSQL.
// Copyright 2015 PingCAP, Inc.
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

// 语句级优化器 hint（提示）与 SQL Binding（绑定）交互的单元测试。
//
// 覆盖：绑定命中时有效 hint 覆盖查询 hint、Finish 后恢复会话变量；
// 以及 SEM（Security Enhanced Mode，安全增强模式）限制 hint 时产生告警。

use astersql_bindinfo::{Binding, StatusEnabled, TableName};
use astersql_parser::Parser;
use astersql_sessionctx_vardef as vardef;

use crate::hint_runtime::{
    BindingStatementFromAST, SessionBindingCatalog, StartStatementHintsWithBindings,
};

/// 将单条 SQL 解析为 AST 节点，供 hint 生命周期测试使用。
fn parse(sql: &str) -> Box<dyn astersql_parser_ast::Node> {
    Parser::default()
        .ParseSQL(sql, &[])
        .expect("parse statement")
        .0
        .into_iter()
        .next()
        .expect("one statement")
}

/// 验证会话 Binding 命中时覆盖查询 hint，Finish 后恢复 SET_VAR 与 TiDBFoundInBinding 标记。
#[test]
fn real_bindinfo_match_overrides_query_hints_and_restores_statement_state() {
    let query = "SELECT /*+ SET_VAR(tidb_opt_partial_ordered_index_for_topn=DISABLE) MAX_EXECUTION_TIME(2222) WRITE_SLOW_LOG */ a FROM t_multi";
    let mut variables = astersql_sessionctx_variable::session::SessionVars::default();
    variables.SetCurrentDB("test");
    variables
        .SetSystemVar(vardef::TiDBOptPartialOrderedIndexForTopN, "DISABLE")
        .expect("set ordinary session value");
    // 注册与查询原文匹配的会话绑定，其 BindSQL 携带不同的 hint 值。
    let mut bindings = SessionBindingCatalog::New("test");
    bindings.AddSessionBinding(Binding {
        OriginalSQL: query.to_owned(),
        Db: "test".to_owned(),
        BindSQL: "SELECT /*+ SET_VAR(tidb_opt_partial_ordered_index_for_topn=COST) MAX_EXECUTION_TIME(1234) */ a FROM t_multi".to_owned(),
        Status: StatusEnabled.to_owned(),
        Charset: "utf8mb4".to_owned(),
        Collation: "utf8mb4_bin".to_owned(),
        TableNames: vec![TableName {
            Schema: "test".to_owned(),
            Name: "t_multi".to_owned(),
            Alias: String::new(),
        }],
        ..Default::default()
    });

    let statement = parse(query);
    let guard =
        StartStatementHintsWithBindings(&variables, statement.as_ref(), query, &mut bindings);
    // 查询原文 hint 保留在 QueryHints；生效值取自 Binding。
    assert_eq!(guard.QueryHints().MaxExecutionTime, 2222);
    assert_eq!(guard.EffectiveHints().MaxExecutionTime, 1234);
    assert!(variables.IsPartialOrderedIndexForTopNEnabled());
    assert!(!variables.StmtCtx.StmtHints.WriteSlowLog());
    assert_eq!(
        variables
            .GetHintSystemVar(vardef::TiDBFoundInBinding)
            .unwrap(),
        vardef::Off
    );
    // Finish 恢复 SET_VAR，并标记本次命中了 Binding。
    guard.Finish().expect("restore binding SET_VAR");
    assert_eq!(
        variables
            .GetHintSystemVar(vardef::TiDBOptPartialOrderedIndexForTopN)
            .unwrap(),
        "DISABLE"
    );
    assert_eq!(
        variables
            .GetHintSystemVar(vardef::TiDBFoundInBinding)
            .unwrap(),
        vardef::On
    );

    // 换表名后不再命中 Binding，回退为查询自身 hint。
    let different_table = query.replace("t_multi", "t_other");
    let statement = parse(&different_table);
    let guard = StartStatementHintsWithBindings(
        &variables,
        statement.as_ref(),
        &different_table,
        &mut bindings,
    );
    assert_eq!(guard.EffectiveHints().MaxExecutionTime, 2222);
    assert!(variables.StmtCtx.StmtHints.WriteSlowLog());
    guard.Finish().expect("restore query SET_VAR");
    assert_eq!(
        variables
            .GetHintSystemVar(vardef::TiDBFoundInBinding)
            .unwrap(),
        vardef::Off
    );
}

/// 模糊绑定开关应由真实 session sysvar 驱动，并在切换时使旧匹配缓存失效。
#[test]
fn fuzzy_binding_switch_controls_cross_database_session_binding() {
    let query = "SELECT /*+ MAX_EXECUTION_TIME(2222) */ a FROM tenant.t_multi";
    let mut variables = astersql_sessionctx_variable::session::SessionVars::default();
    variables.SetCurrentDB("tenant");
    let mut bindings = SessionBindingCatalog::New("tenant");
    bindings.AddSessionBinding(Binding {
        OriginalSQL: query.to_owned(),
        Db: "*".to_owned(),
        BindSQL: "SELECT /*+ MAX_EXECUTION_TIME(1234) */ a FROM *.t_multi".to_owned(),
        Status: StatusEnabled.to_owned(),
        Charset: "utf8mb4".to_owned(),
        Collation: "utf8mb4_bin".to_owned(),
        TableNames: vec![TableName {
            Schema: "*".to_owned(),
            Name: "t_multi".to_owned(),
            Alias: String::new(),
        }],
        ..Default::default()
    });
    let statement = parse(query);

    variables
        .SetSystemVar(vardef::TiDBOptEnableFuzzyBinding, vardef::On)
        .expect("enable fuzzy binding");
    let guard =
        StartStatementHintsWithBindings(&variables, statement.as_ref(), query, &mut bindings);
    assert_eq!(guard.EffectiveHints().MaxExecutionTime, 1234);
    guard.Finish().expect("finish fuzzy binding statement");

    variables
        .SetSystemVar(vardef::TiDBOptEnableFuzzyBinding, vardef::Off)
        .expect("disable fuzzy binding");
    let guard =
        StartStatementHintsWithBindings(&variables, statement.as_ref(), query, &mut bindings);
    assert_eq!(guard.EffectiveHints().MaxExecutionTime, 2222);
    guard.Finish().expect("finish ordinary statement");
}

/// AST 适配器必须收集表达式子查询中的表，且不把字符串里的 `?` 当参数。
#[test]
fn binding_statement_adapter_uses_complete_parser_facts() {
    let sql = "select '?' from t1 where a in (select a from db2.t2)";
    let statement = parse(sql);
    let binding_statement = BindingStatementFromAST(sql, statement.as_ref());
    assert_eq!(
        binding_statement.Tables,
        vec![
            TableName {
                Schema: String::new(),
                Name: "t1".to_owned(),
                Alias: String::new(),
            },
            TableName {
                Schema: "db2".to_owned(),
                Name: "t2".to_owned(),
                Alias: String::new(),
            },
        ]
    );
    assert!(!binding_statement.HasParamMarker);
}

/// 验证 SEM 受限 hint（如 RESOURCE_GROUP）被过滤，并留下含 "restricted" 的语句告警。
#[test]
fn sem_restricted_hint_is_filtered_with_a_statement_warning() {
    astersql_util_sem_v2::Disable();
    // 启用 SEM v2，将 resource_group 列入受限 hint 名单。
    astersql_util_sem_v2::EnableBy(&astersql_util_sem_v2::Config {
        Version: "1.0".to_owned(),
        TiDBVersion: "v6.0.0".to_owned(),
        RestrictedHints: vec!["resource_group".to_owned()],
        ..Default::default()
    })
    .expect("enable SEM v2");

    let mut variables = astersql_sessionctx_variable::session::SessionVars::default();
    variables.SetCurrentDB("test");
    let sql = "SELECT /*+ RESOURCE_GROUP(rg1) */ a FROM t_multi";
    let statement = parse(sql);
    let mut bindings = SessionBindingCatalog::New("test");
    let guard = StartStatementHintsWithBindings(&variables, statement.as_ref(), sql, &mut bindings);
    assert!(!guard.EffectiveHints().HasResourceGroup);
    assert!(
        guard
            .Warnings()
            .iter()
            .any(|warning| warning.contains("restricted"))
    );
    guard.Finish().expect("finish restricted hint statement");
    astersql_util_sem_v2::Disable();
}
