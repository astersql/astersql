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

// 规划器（Optimizer）会话级标识分配与 MPP 告警语义的单元测试。
//
// 覆盖：PlanID / PlanColumnID 单调递增、隔离读引擎默认集合，
// 以及强制 MPP（Massively Parallel Processing，大规模并行处理）时的告警写入。

use crate::session::SessionVars;
use kv::StoreType;
use std::sync::Arc;

/// PlanID 与列 ID 应在会话内单调分配，并默认包含 TiKV/TiFlash/TiDB 隔离读引擎。
#[test]
fn planner_identifiers_are_session_scoped_and_monotonic() {
    let vars = SessionVars::new();

    assert_eq!(vars.AllocNewPlanID(), 1);
    assert_eq!(vars.AllocNewPlanID(), 2);
    assert_eq!(vars.AllocPlanColumnID(), 1);
    assert_eq!(vars.AllocPlanColumnID(), 2);
    assert!(!vars.StmtCtx.IgnoreExplainIDSuffix);
    assert!(vars.GetIsolationReadEngines().contains(&StoreType::TiKV));
    assert!(vars.GetIsolationReadEngines().contains(&StoreType::TiFlash));
    assert!(vars.GetIsolationReadEngines().contains(&StoreType::TiDB));
}

/// Go 使用原子 Add 分配规划器 ID；并发调用不得重复或跳号。
#[test]
fn planner_identifiers_are_atomic_under_concurrent_session_use() {
    const THREADS: usize = 8;
    const IDS_PER_THREAD: usize = 64;
    let vars = Arc::new(SessionVars::new());
    let handles = (0..THREADS)
        .map(|_| {
            let vars = Arc::clone(&vars);
            std::thread::spawn(move || {
                (0..IDS_PER_THREAD)
                    .map(|_| (vars.AllocNewPlanID(), vars.AllocPlanColumnID()))
                    .collect::<Vec<_>>()
            })
        })
        .collect::<Vec<_>>();

    let (mut plan_ids, mut column_ids): (Vec<_>, Vec<_>) = handles
        .into_iter()
        .flat_map(|handle| handle.join().expect("planner ID allocator thread"))
        .unzip();
    plan_ids.sort_unstable();
    column_ids.sort_unstable();

    let expected_plan_ids = (1..=(THREADS * IDS_PER_THREAD) as i32).collect::<Vec<_>>();
    let expected_column_ids = (1..=(THREADS * IDS_PER_THREAD) as i64).collect::<Vec<_>>();
    assert_eq!(plan_ids, expected_plan_ids);
    assert_eq!(column_ids, expected_column_ids);
}

/// SET 必须同步更新规划器读取的字符串匹配选择率字段。
#[test]
fn string_match_selectivity_set_updates_planner_field() {
    let mut vars = SessionVars::new();

    vars.SetSystemVar(vardef::TiDBDefaultStrMatchSelectivity, "0")
        .expect("set zero string match selectivity");
    assert_eq!(vars.DefaultStrMatchSelectivity, 0.0);
    assert_eq!(vars.GetStrMatchDefaultSelectivity(), 0.1);

    vars.SetSystemVar(vardef::TiDBDefaultStrMatchSelectivity, "0.8")
        .expect("set string match selectivity");

    assert_eq!(vars.DefaultStrMatchSelectivity, 0.8);
    assert_eq!(vars.GetStrMatchDefaultSelectivity(), 0.8);
}

/// Select 块别名存储与 MPP 强制执行时的额外警告应保持会话语义。
#[test]
fn planner_block_names_and_mpp_warnings_keep_session_semantics() {
    let mut vars = SessionVars::new();
    vars.PlannerSelectBlockAsName
        .Store(Some(vec![parser_ast::HintTable::default()]));
    assert_eq!(vars.PlannerSelectBlockAsName.Load().unwrap().len(), 1);

    // 未强制 MPP 时与 Go 一样直接返回，不写入任何告警。
    vars.RaiseWarningWhenMPPEnforced("ignored warning");
    assert!(vars.StmtCtx.GetWarnings().is_empty());
    assert!(vars.StmtCtx.GetExtraWarnings().is_empty());

    // 同时打开 Allow 与 Enforce 后，普通语句写入 ExtraWarnings。
    vars.AllowMPPExecution = true;
    vars.EnforceMPPExecution = true;
    vars.RaiseWarningWhenMPPEnforced("mpp warning");
    let warnings = vars.StmtCtx.GetExtraWarnings();
    assert_eq!(warnings.len(), 1);
    assert_eq!(
        warnings[0].Err.as_ref().map(ToString::to_string).as_deref(),
        Some("mpp warning")
    );

    // Explain 语句必须写入普通 Warning，而不是 ExtraWarnings。
    let mut explain_vars = SessionVars::new();
    explain_vars.AllowMPPExecution = true;
    explain_vars.EnforceMPPExecution = true;
    explain_vars.StmtCtx.SetExplainContext(true, true, "ru");
    explain_vars.RaiseWarningWhenMPPEnforced("explain mpp warning");
    let warnings = explain_vars.StmtCtx.GetWarnings();
    assert_eq!(warnings.len(), 1);
    assert_eq!(
        warnings[0].Err.as_ref().map(ToString::to_string).as_deref(),
        Some("explain mpp warning")
    );
    assert!(explain_vars.StmtCtx.GetExtraWarnings().is_empty());
}
