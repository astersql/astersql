// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
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

// 对应 `slow_log_test.go`：慢日志规则字段 accessor、匹配与解析。
// 覆盖单/多条件 AND、多规则 OR、类型化阈值解析，以及会话/全局规则字符串解析。
// 匹配语义保持 Go 约定：单条规则内部条件取 AND，多条规则之间取 OR。

#![allow(non_snake_case)]

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicI32, AtomicI64, Ordering};
use std::time::Duration;

use astersql_executor::adapter_slow_log::MatchSessionVars;
use astersql_sessionctx_variable::session::{RewritePhaseInfo, SessionVars};
use astersql_sessionctx_variable::slow_log::{
    self, NewSessionSlowLogRules, SlowLogCondition, SlowLogExecContext, SlowLogRule,
    SlowLogRuleFieldAccessors, SlowLogRules, SlowQueryLogItems, Threshold, UnsetConnID,
};
use astersql_util_execdetails::execdetails::{self as ed, CopExecDetails, ExecDetails};
use astersql_util_execdetails::util::{context as exec_context, util as tikv_util};

/// 构造带空慢日志规则集的会话变量。
fn new_mock_vars() -> SessionVars {
    let mut vars = SessionVars::new();
    vars.SlowLogRules = NewSessionSlowLogRules(Some(SlowLogRules {
        rules: Vec::new(),
        fields: BTreeSet::new(),
        raw_rules: String::new(),
    }));
    vars
}

/// 写入慢日志规则并汇总条件字段集合。
fn set_rules(vars: &mut SessionVars, rules: Vec<SlowLogRule>) {
    let fields = rules
        .iter()
        .flat_map(|rule| rule.conditions.iter().map(|c| c.field.to_ascii_lowercase()))
        .collect();
    vars.SlowLogRules.SlowLogRules = Some(SlowLogRules {
        rules,
        fields,
        raw_rules: String::new(),
    });
}

/// 构造单字段阈值条件。
fn cond(field: &str, threshold: Threshold) -> SlowLogCondition {
    SlowLogCondition {
        field: field.to_owned(),
        threshold,
    }
}

/// 按会话规则匹配慢查询日志项。
fn match_rules(vars: &SessionVars, items: &SlowQueryLogItems) -> bool {
    MatchSessionVars(vars, items, vars.SlowLogRules.SlowLogRules.as_ref())
}

/// 校验慢日志字段 accessor 完整性（含 Setter 例外字段）。
#[test]
fn TestSlowLogFieldAccessor() {
    for (field, accessor) in SlowLogRuleFieldAccessors.iter() {
        assert_eq!(
            field.to_ascii_lowercase(),
            *field,
            "field {field:?}: field name should be all lowercase"
        );
        let _ = accessor.Parse;
        let _ = &accessor.Match;
        if accessor.Setter.is_none() {
            if field == &slow_log::SlowLogParseTimeStr.to_ascii_lowercase()
                || field == &slow_log::SlowLogCompileTimeStr.to_ascii_lowercase()
                || field == &slow_log::SlowLogOptimizeTimeStr.to_ascii_lowercase()
                || field == &slow_log::SlowLogWaitTSTimeStr.to_ascii_lowercase()
                || field == &slow_log::SlowLogIsInternalStr.to_ascii_lowercase()
                || field == &slow_log::SlowLogConnIDStr.to_ascii_lowercase()
                || field == &slow_log::SlowLogSessAliasStr.to_ascii_lowercase()
                || field == &slow_log::SlowLogDBStr.to_ascii_lowercase()
            {
                continue;
            }
            panic!("field {field:?}: Setter function is missing");
        }
    }
}

/// 单规则单条件匹配与不匹配。
#[test]
fn TestMatchSingleRuleSingleCondition() {
    let mut vars = new_mock_vars();
    let mut items = SlowQueryLogItems {
        MemMax: 200,
        ..Default::default()
    };
    set_rules(
        &mut vars,
        vec![SlowLogRule {
            conditions: vec![cond(slow_log::SlowLogMemMax, Threshold::Int(100))],
        }],
    );
    assert!(match_rules(&vars, &items));
    items.MemMax = 50;
    assert!(!match_rules(&vars, &items));
}

/// 布尔/字符串/资源组等特殊类型条件匹配。
#[test]
fn TestMatchSpecialTypeConditions() {
    let mut vars = new_mock_vars();
    let mut items = SlowQueryLogItems {
        Succ: true,
        Digest: "abc".into(),
        ResourceGroupName: "rg_Test".into(),
        ..Default::default()
    };

    let check_ret = |vars: &mut SessionVars,
                     items: &SlowQueryLogItems,
                     expect_match: bool,
                     condition: SlowLogCondition| {
        set_rules(
            vars,
            vec![SlowLogRule {
                conditions: vec![condition],
            }],
        );
        assert_eq!(match_rules(vars, items), expect_match);
    };

    // 字符串类条件：库名、别名、摘要、资源组等。
    // string type
    {
        vars.SetCurrentDB("db_Test");
        vars.SessionAlias = "seA".into();
        set_rules(
            &mut vars,
            vec![SlowLogRule {
                conditions: vec![
                    cond(slow_log::SlowLogSucc, Threshold::Bool(true)),
                    cond(slow_log::SlowLogDigestStr, Threshold::String("abc".into())),
                    cond(
                        slow_log::SlowLogResourceGroup,
                        Threshold::String("rg_test".into()),
                    ),
                    cond(slow_log::SlowLogDBStr, Threshold::String("db_test".into())),
                    cond(
                        slow_log::SlowLogSessAliasStr,
                        Threshold::String("seA".into()),
                    ),
                ],
            }],
        );
        assert!(match_rules(&vars, &items));

        vars.SessionAlias = "sea".into();
        assert!(!match_rules(&vars, &items));
        vars.SessionAlias = "seA".into();
        items.Digest = "abC".into();
        assert!(!match_rules(&vars, &items));
        items.Digest = "abc".into();

        check_ret(
            &mut vars,
            &items,
            true,
            cond(slow_log::SlowLogRewriteTimeStr, Threshold::Float(0.0)),
        );
        check_ret(
            &mut vars,
            &items,
            false,
            cond(
                slow_log::SlowLogRewriteTimeStr,
                Threshold::Float(0.00000001),
            ),
        );
        vars.RestoreRewritePhaseInfo(RewritePhaseInfo {
            DurationRewrite: Duration::from_millis(5),
            ..Default::default()
        });
        let accessor = SlowLogRuleFieldAccessors
            .get(&slow_log::SlowLogRewriteTimeStr.to_ascii_lowercase())
            .unwrap();
        (accessor.Setter.as_ref().unwrap())(
            &SlowLogExecContext::default(),
            Some(&vars),
            &mut items,
        );
        check_ret(
            &mut vars,
            &items,
            true,
            cond(slow_log::SlowLogRewriteTimeStr, Threshold::Float(0.001)),
        );
        check_ret(
            &mut vars,
            &items,
            true,
            cond(slow_log::SlowLogRewriteTimeStr, Threshold::Float(0.0)),
        );
        check_ret(
            &mut vars,
            &items,
            false,
            cond(slow_log::SlowLogRewriteTimeStr, Threshold::Float(0.01)),
        );
        items.RewriteInfo.DurationRewrite = Duration::from_micros(500);
        check_ret(
            &mut vars,
            &items,
            false,
            cond(slow_log::SlowLogRewriteTimeStr, Threshold::Float(0.001)),
        );
        check_ret(
            &mut vars,
            &items,
            true,
            cond(slow_log::SlowLogRewriteTimeStr, Threshold::Float(0.0001)),
        );
    }

    // util.ExecDetails type
    {
        check_ret(
            &mut vars,
            &items,
            false,
            cond(slow_log::SlowLogKVTotal, Threshold::Float(0.00000001)),
        );
        check_ret(
            &mut vars,
            &items,
            true,
            cond(slow_log::SlowLogKVTotal, Threshold::Float(0.0)),
        );
        let tikv_exec_detail = Arc::new(tikv_util::ExecDetails {
            WaitKVRespDuration: AtomicI64::new(Duration::from_secs(10).as_nanos() as i64),
            ..Default::default()
        });
        let child_ctx = exec_context::WithValue(
            SlowLogExecContext::default(),
            &tikv_util::ExecDetailsKey,
            tikv_exec_detail.clone(),
        );
        let accessor = SlowLogRuleFieldAccessors
            .get(&slow_log::SlowLogKVTotal.to_ascii_lowercase())
            .unwrap();
        (accessor.Setter.as_ref().unwrap())(&child_ctx, None, &mut items);
        check_ret(
            &mut vars,
            &items,
            true,
            cond(slow_log::SlowLogKVTotal, Threshold::Float(0.00000001)),
        );
        check_ret(
            &mut vars,
            &items,
            true,
            cond(slow_log::SlowLogKVTotal, Threshold::Float(0.0)),
        );

        // setter snapshots kv exec details
        {
            let mut items = SlowQueryLogItems::default();
            let tikv_exec_detail = Arc::new(tikv_util::ExecDetails {
                WaitKVRespDuration: AtomicI64::new(Duration::from_secs(10).as_nanos() as i64),
                ..Default::default()
            });
            let accessor = SlowLogRuleFieldAccessors
                .get(&slow_log::SlowLogKVTotal.to_ascii_lowercase())
                .unwrap();
            let ctx = exec_context::WithValue(
                SlowLogExecContext::default(),
                &tikv_util::ExecDetailsKey,
                tikv_exec_detail.clone(),
            );
            (accessor.Setter.as_ref().unwrap())(&ctx, None, &mut items);
            tikv_exec_detail
                .WaitKVRespDuration
                .store(Duration::from_secs(20).as_nanos() as i64, Ordering::Relaxed);

            let detail = items.KVExecDetail.as_ref().expect("KVExecDetail set");
            assert_eq!(
                detail.WaitKVRespDuration.load(Ordering::Relaxed),
                Duration::from_secs(10).as_nanos() as i64
            );
            assert!((accessor.Match)(None, &items, &Threshold::Float(9.0)));
            assert!(!(accessor.Match)(None, &items, &Threshold::Float(11.0)));
        }
    }

    // execdetails.ExecDetails type
    {
        vars.StmtCtx.SyncExecDetails.Reset();
        check_ret(
            &mut vars,
            &items,
            false,
            cond(ed::ProcessTimeStr, Threshold::Float(1.0)),
        );
        check_ret(
            &mut vars,
            &items,
            false,
            cond(ed::TotalKeysStr, Threshold::UInt(2)),
        );
        check_ret(
            &mut vars,
            &items,
            false,
            cond(
                slow_log::SlowLogCopMVCCReadAmplification,
                Threshold::Float(0.00000001),
            ),
        );
        check_ret(
            &mut vars,
            &items,
            false,
            cond(ed::PreWriteTimeStr, Threshold::Float(0.123)),
        );
        check_ret(
            &mut vars,
            &items,
            false,
            cond(ed::PrewriteRegionStr, Threshold::UInt(4)),
        );
        check_ret(
            &mut vars,
            &items,
            true,
            cond(ed::ProcessTimeStr, Threshold::Float(0.0)),
        );
        check_ret(
            &mut vars,
            &items,
            true,
            cond(ed::TotalKeysStr, Threshold::UInt(0)),
        );
        check_ret(
            &mut vars,
            &items,
            true,
            cond(
                slow_log::SlowLogCopMVCCReadAmplification,
                Threshold::Float(0.0),
            ),
        );
        check_ret(
            &mut vars,
            &items,
            true,
            cond(ed::PreWriteTimeStr, Threshold::Float(0.0)),
        );
        check_ret(
            &mut vars,
            &items,
            true,
            cond(ed::PrewriteRegionStr, Threshold::UInt(0)),
        );

        let exec_detail = ExecDetails {
            CopExecDetails: CopExecDetails {
                BackoffTime: Duration::from_millis(1),
                ..Default::default()
            },
            ..Default::default()
        };
        vars.StmtCtx
            .SyncExecDetails
            .MergeCopExecDetails(Some(&exec_detail.CopExecDetails), Duration::default());
        let accessor = SlowLogRuleFieldAccessors
            .get(&ed::TotalKeysStr.to_ascii_lowercase())
            .unwrap();
        (accessor.Setter.as_ref().unwrap())(
            &SlowLogExecContext::default(),
            Some(&vars),
            &mut items,
        );
        check_ret(
            &mut vars,
            &items,
            true,
            cond(ed::ProcessTimeStr, Threshold::Float(0.0)),
        );
        check_ret(
            &mut vars,
            &items,
            true,
            cond(ed::TotalKeysStr, Threshold::UInt(0)),
        );
        check_ret(
            &mut vars,
            &items,
            true,
            cond(
                slow_log::SlowLogCopMVCCReadAmplification,
                Threshold::Float(0.0),
            ),
        );
        check_ret(
            &mut vars,
            &items,
            true,
            cond(ed::PreWriteTimeStr, Threshold::Float(0.0)),
        );
        check_ret(
            &mut vars,
            &items,
            true,
            cond(ed::PrewriteRegionStr, Threshold::UInt(0)),
        );

        vars.StmtCtx.SyncExecDetails.Reset();
        let exec_detail = ExecDetails {
            CopExecDetails: CopExecDetails {
                ScanDetail: Some(ed::util::ScanDetail {
                    ProcessedKeys: 10,
                    TotalKeys: 100,
                    ..Default::default()
                }),
                ..Default::default()
            },
            ..Default::default()
        };
        vars.StmtCtx
            .SyncExecDetails
            .MergeCopExecDetails(Some(&exec_detail.CopExecDetails), Duration::default());
        items.ExecDetail = None;
        (accessor.Setter.as_ref().unwrap())(
            &SlowLogExecContext::default(),
            Some(&vars),
            &mut items,
        );
        check_ret(
            &mut vars,
            &items,
            true,
            cond(
                slow_log::SlowLogCopMVCCReadAmplification,
                Threshold::Float(9.99),
            ),
        );
        check_ret(
            &mut vars,
            &items,
            true,
            cond(
                slow_log::SlowLogCopMVCCReadAmplification,
                Threshold::Float(10.0),
            ),
        );
        check_ret(
            &mut vars,
            &items,
            false,
            cond(
                slow_log::SlowLogCopMVCCReadAmplification,
                Threshold::Float(10.01),
            ),
        );

        items
            .ExecDetail
            .as_mut()
            .unwrap()
            .CopExecDetails
            .ScanDetail
            .as_mut()
            .unwrap()
            .ProcessedKeys = 0;
        check_ret(
            &mut vars,
            &items,
            true,
            cond(
                slow_log::SlowLogCopMVCCReadAmplification,
                Threshold::Float(0.0),
            ),
        );
        check_ret(
            &mut vars,
            &items,
            false,
            cond(
                slow_log::SlowLogCopMVCCReadAmplification,
                Threshold::Float(0.01),
            ),
        );

        let scan = items
            .ExecDetail
            .as_mut()
            .unwrap()
            .CopExecDetails
            .ScanDetail
            .as_mut()
            .unwrap();
        scan.TotalKeys = 0;
        scan.ProcessedKeys = 10;
        check_ret(
            &mut vars,
            &items,
            true,
            cond(
                slow_log::SlowLogCopMVCCReadAmplification,
                Threshold::Float(0.0),
            ),
        );
        check_ret(
            &mut vars,
            &items,
            false,
            cond(
                slow_log::SlowLogCopMVCCReadAmplification,
                Threshold::Float(0.01),
            ),
        );
    }
}

/// 单规则多条件为 AND 语义。
#[test]
fn TestMatchSingleRuleMultipleConditions() {
    let mut vars = new_mock_vars();
    let mut items = SlowQueryLogItems {
        MemMax: 200,
        Digest: "abc".into(),
        Succ: true,
        WriteSQLRespTotal: Duration::from_nanos(1_500_000),
        ..Default::default()
    };
    set_rules(
        &mut vars,
        vec![SlowLogRule {
            conditions: vec![
                cond(slow_log::SlowLogMemMax, Threshold::Int(100)),
                cond(slow_log::SlowLogDigestStr, Threshold::String("abc".into())),
                cond(slow_log::SlowLogSucc, Threshold::Bool(true)),
                cond(slow_log::SlowLogWriteSQLRespTotal, Threshold::Float(0.0015)),
            ],
        }],
    );
    assert!(match_rules(&vars, &items));
    items.Succ = false;
    assert!(!match_rules(&vars, &items));
}

/// 多规则之间为 OR 语义。
#[test]
fn TestMatchMultipleRulesOR() {
    let mut vars = new_mock_vars();
    let mut items = SlowQueryLogItems {
        ExecRetryCount: 5,
        Digest: "abc".into(),
        Succ: true,
        MemMax: 200,
        ..Default::default()
    };
    vars.SlowLogRules = NewSessionSlowLogRules(Some(SlowLogRules {
        rules: vec![
            SlowLogRule {
                conditions: vec![
                    cond(slow_log::SlowLogExecRetryCount, Threshold::UInt(3)),
                    cond(slow_log::SlowLogSucc, Threshold::Bool(true)),
                ],
            },
            SlowLogRule {
                conditions: vec![cond(slow_log::SlowLogMemMax, Threshold::Int(500))],
            },
        ],
        fields: BTreeSet::new(),
        raw_rules: String::new(),
    }));
    assert!(match_rules(&vars, &items));
    items.ExecRetryCount = 1;
    assert!(!match_rules(&vars, &items));

    items.Digest = "plan_digest".into();
    vars.SlowLogRules = NewSessionSlowLogRules(Some(SlowLogRules {
        rules: vec![SlowLogRule {
            conditions: vec![cond(
                slow_log::SlowLogDigestStr,
                Threshold::String("plan_digest".into()),
            )],
        }],
        fields: BTreeSet::new(),
        raw_rules: String::new(),
    }));
    assert!(match_rules(&vars, &items));
}

/// 解析后不同类型阈值的匹配。
#[test]
fn TestMatchDifferentTypesAfterParse() {
    let mut vars = new_mock_vars();
    let items = SlowQueryLogItems {
        MemMax: 123,
        DiskMax: 456,
        ExecRetryCount: 789,
        ResourceGroupName: "rg1".into(),
        Succ: true,
        TimeTotal: Duration::from_millis(3140),
        ..Default::default()
    };
    let slow_log_rules = slow_log::ParseSessionSlowLogRules(
        "Mem_max: 100, Exec_retry_count: 300, Succ: true, Query_time: 2.52, Resource_group: rg1",
    )
    .unwrap()
    .unwrap();
    vars.SlowLogRules.SlowLogRules = Some(slow_log_rules);
    assert!(match_rules(&vars, &items));
}

/// 执行详情无符号整数字段解析后匹配。
#[test]
fn TestMatchUintExecDetailFieldsAfterParse() {
    // commit fields
    {
        let mut vars = new_mock_vars();
        let rules =
            slow_log::ParseSessionSlowLogRules("Write_keys:1,Write_size:2,Prewrite_region:3")
                .unwrap()
                .unwrap();
        vars.SlowLogRules.SlowLogRules = Some(rules);
        let new_items =
            |write_keys: i32, write_size: i32, prewrite_region_num: i32| SlowQueryLogItems {
                ExecDetail: Some(Box::new(ExecDetails {
                    CommitDetail: Some(ed::util::CommitDetails {
                        WriteKeys: write_keys,
                        WriteSize: write_size,
                        PrewriteRegionNum: AtomicI32::new(prewrite_region_num),
                        ..Default::default()
                    }),
                    ..Default::default()
                })),
                ..Default::default()
            };
        assert!(match_rules(&vars, &new_items(2, 3, 4)));
        assert!(!match_rules(&vars, &new_items(-1, 3, 4)));
        assert!(!match_rules(&vars, &new_items(2, -1, 4)));
        assert!(!match_rules(&vars, &new_items(2, 3, -1)));
    }

    // scan fields
    {
        let mut vars = new_mock_vars();
        let rules = slow_log::ParseSessionSlowLogRules("Total_keys:1,Process_keys:2")
            .unwrap()
            .unwrap();
        vars.SlowLogRules.SlowLogRules = Some(rules);
        let new_items = |total_keys: i64, processed_keys: i64| SlowQueryLogItems {
            ExecDetail: Some(Box::new(ExecDetails {
                CopExecDetails: CopExecDetails {
                    ScanDetail: Some(ed::util::ScanDetail {
                        TotalKeys: total_keys,
                        ProcessedKeys: processed_keys,
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                ..Default::default()
            })),
            ..Default::default()
        };
        assert!(match_rules(&vars, &new_items(2, 3)));
        assert!(!match_rules(&vars, &new_items(-1, 3)));
        assert!(!match_rules(&vars, &new_items(2, -1)));
    }
}

/// 单字段慢日志条件解析。
#[test]
fn TestParseSingleSlowLogField() {
    assert_eq!(SlowLogRuleFieldAccessors.len(), 39);
    let accessor = SlowLogRuleFieldAccessors
        .get(&slow_log::SlowLogPlanDigest.to_ascii_lowercase())
        .expect("plan digest accessor");
    assert!(accessor.Setter.is_some());
    let _ = &accessor.Match;

    let v = slow_log::ParseSlowLogFieldValue(slow_log::SlowLogMemMax, "123").unwrap();
    assert_eq!(v, Threshold::Int(123));
    assert!(slow_log::ParseSlowLogFieldValue(slow_log::SlowLogMemMax, "abc").is_err());

    let v = slow_log::ParseSlowLogFieldValue(slow_log::SlowLogConnIDStr, "456").unwrap();
    assert_eq!(v, Threshold::UInt(456));
    assert!(slow_log::ParseSlowLogFieldValue(slow_log::SlowLogConnIDStr, "-1").is_err());

    let v = slow_log::ParseSlowLogFieldValue(slow_log::SlowLogQueryTimeStr, "1.234").unwrap();
    assert_eq!(v, Threshold::Float(1.234));
    let v = slow_log::ParseSlowLogFieldValue(slow_log::SlowLogQueryTimeStr, "1.5e6").unwrap();
    assert_eq!(v, Threshold::Float(1.5e6));
    let v = slow_log::ParseSlowLogFieldValue(slow_log::SlowLogCopMVCCReadAmplification, "10.5")
        .unwrap();
    assert_eq!(v, Threshold::Float(10.5));
    assert!(
        slow_log::ParseSlowLogFieldValue(slow_log::SlowLogCopMVCCReadAmplification, "abc").is_err()
    );
    assert!(slow_log::ParseSlowLogFieldValue(slow_log::SlowLogQueryTimeStr, "abc").is_err());

    let err = slow_log::ParseSlowLogFieldValue(slow_log::SlowLogQueryTimeStr, "-1.5").unwrap_err();
    assert!(err.contains("non-negative"));
    let err = slow_log::ParseSlowLogFieldValue(slow_log::SlowLogCopMVCCReadAmplification, "-0.1")
        .unwrap_err();
    assert!(err.contains("non-negative"));
    let v = slow_log::ParseSlowLogFieldValue(slow_log::SlowLogQueryTimeStr, "0").unwrap();
    assert_eq!(v, Threshold::Float(0.0));

    let err = slow_log::ParseSlowLogFieldValue(slow_log::SlowLogMemMax, "-100").unwrap_err();
    assert!(err.contains("non-negative"));
    let v = slow_log::ParseSlowLogFieldValue(slow_log::SlowLogMemMax, "0").unwrap();
    assert_eq!(v, Threshold::Int(0));

    let v = slow_log::ParseSlowLogFieldValue(slow_log::SlowLogDBStr, "testdb").unwrap();
    assert_eq!(v, Threshold::String("testdb".into()));

    let v = slow_log::ParseSlowLogFieldValue(slow_log::SlowLogSucc, "true").unwrap();
    assert_eq!(v, Threshold::Bool(true));
    let v = slow_log::ParseSlowLogFieldValue(slow_log::SlowLogSucc, "false").unwrap();
    assert_eq!(v, Threshold::Bool(false));
    assert!(slow_log::ParseSlowLogFieldValue(slow_log::SlowLogSucc, "notabool").is_err());

    let err = slow_log::ParseSlowLogFieldValue("NonExistField", "xxx").unwrap_err();
    assert!(err.contains("unknown slow log field name"));
}

/// 无序比较两组慢日志条件。
fn compare_conditions_unordered(
    got: &[SlowLogCondition],
    want: &[SlowLogCondition],
    raw_rule: &str,
) {
    assert_eq!(got.len(), want.len());
    let got_map: BTreeMap<String, &Threshold> = got
        .iter()
        .map(|c| (c.field.to_ascii_lowercase(), &c.threshold))
        .collect();
    let raw_rule = raw_rule.to_ascii_lowercase();
    for c in want {
        let field = c.field.to_ascii_lowercase();
        let val = got_map.get(&field).expect("missing field");
        let kv = format!("{field}: {val}");
        assert!(raw_rule.contains(&kv), "rawRule:{raw_rule}, substr: {kv}");
        assert_eq!(&c.threshold, *val);
    }
}

/// 解析会话级慢日志规则字符串。
#[test]
fn TestParseSessionSlowLogRules() {
    let err = slow_log::ParseSessionSlowLogRules(
        "Conn_ID: 123, DB: db1, Succ: true, Query_time: 0.5276, Resource_group: rg1",
    )
    .unwrap_err();
    assert_eq!(err, "do not allow ConnID value:123");

    let raw_rule =
        "Exec_retry_count: 10, DB: db1, Succ: true, Query_time: 0.5276, Resource_group: rg1";
    let slow_log_rules = slow_log::ParseSessionSlowLogRules(raw_rule)
        .unwrap()
        .unwrap();
    let rules = [SlowLogRule {
        conditions: vec![
            cond(
                &slow_log::SlowLogExecRetryCount.to_ascii_lowercase(),
                Threshold::UInt(10),
            ),
            cond(
                &slow_log::SlowLogDBStr.to_ascii_lowercase(),
                Threshold::String("db1".into()),
            ),
            cond(
                &slow_log::SlowLogSucc.to_ascii_lowercase(),
                Threshold::Bool(true),
            ),
            cond(
                &slow_log::SlowLogQueryTimeStr.to_ascii_lowercase(),
                Threshold::Float(0.5276),
            ),
            cond(
                &slow_log::SlowLogResourceGroup.to_ascii_lowercase(),
                Threshold::String("rg1".into()),
            ),
        ],
    }];
    let all_condition_fields: BTreeSet<String> = [
        slow_log::SlowLogExecRetryCount,
        slow_log::SlowLogDBStr,
        slow_log::SlowLogSucc,
        slow_log::SlowLogQueryTimeStr,
        slow_log::SlowLogResourceGroup,
    ]
    .into_iter()
    .map(|s| s.to_ascii_lowercase())
    .collect();
    compare_conditions_unordered(
        &rules[0].conditions,
        &slow_log_rules.rules[0].conditions,
        raw_rule,
    );
    assert_eq!(all_condition_fields, slow_log_rules.fields);

    let slow_log_rules = slow_log::ParseSessionSlowLogRules(
        "Exec_retry_count: 10, DB: db1, Succ: true, Query_time: 0.5276, Resource_group: rg1;",
    )
    .unwrap()
    .unwrap();
    compare_conditions_unordered(
        &rules[0].conditions,
        &slow_log_rules.rules[0].conditions,
        raw_rule,
    );
    assert_eq!(all_condition_fields, slow_log_rules.fields);

    let raw_rule1 =
        "Exec_retry_count: 123, DB: db1, Succ: true, Query_time: 0.5276, Resource_group: rg1;";
    let raw_rule2 = "Exec_retry_count: 124, DB: db2, Succ: false, Query_time: 1.5276";
    let slow_log_rules = slow_log::ParseSessionSlowLogRules(&format!("{raw_rule1}{raw_rule2}"))
        .unwrap()
        .unwrap();
    let rules = [
        SlowLogRule {
            conditions: vec![
                cond(
                    &slow_log::SlowLogExecRetryCount.to_ascii_lowercase(),
                    Threshold::UInt(123),
                ),
                cond(
                    &slow_log::SlowLogDBStr.to_ascii_lowercase(),
                    Threshold::String("db1".into()),
                ),
                cond(
                    &slow_log::SlowLogSucc.to_ascii_lowercase(),
                    Threshold::Bool(true),
                ),
                cond(
                    &slow_log::SlowLogQueryTimeStr.to_ascii_lowercase(),
                    Threshold::Float(0.5276),
                ),
                cond(
                    &slow_log::SlowLogResourceGroup.to_ascii_lowercase(),
                    Threshold::String("rg1".into()),
                ),
            ],
        },
        SlowLogRule {
            conditions: vec![
                cond(
                    &slow_log::SlowLogExecRetryCount.to_ascii_lowercase(),
                    Threshold::UInt(124),
                ),
                cond(
                    &slow_log::SlowLogDBStr.to_ascii_lowercase(),
                    Threshold::String("db2".into()),
                ),
                cond(
                    &slow_log::SlowLogSucc.to_ascii_lowercase(),
                    Threshold::Bool(false),
                ),
                cond(
                    &slow_log::SlowLogQueryTimeStr.to_ascii_lowercase(),
                    Threshold::Float(1.5276),
                ),
            ],
        },
    ];
    compare_conditions_unordered(
        &rules[0].conditions,
        &slow_log_rules.rules[0].conditions,
        raw_rule1,
    );
    compare_conditions_unordered(
        &rules[1].conditions,
        &slow_log_rules.rules[1].conditions,
        raw_rule2,
    );
    assert_eq!(all_condition_fields, slow_log_rules.fields);

    assert!(slow_log::ParseSessionSlowLogRules("  ").unwrap().is_none());
    assert!(
        slow_log::ParseSessionSlowLogRules("  ; ; ")
            .unwrap()
            .is_none()
    );

    let err = slow_log::ParseSessionSlowLogRules(&"Conn_ID:1;".repeat(11)).unwrap_err();
    assert!(err.contains("invalid slow log rules count"));

    let slow_log_rules = slow_log::ParseSessionSlowLogRules(r#"DB:"a,b", Succ:true"#)
        .unwrap()
        .unwrap();
    assert_eq!(slow_log_rules.rules[0].conditions.len(), 2);

    let err = slow_log::ParseSessionSlowLogRules("Exec_retry_count 123").unwrap_err();
    assert!(err.contains("invalid slow log rule format:Exec_retry_count 123"));
    let err = slow_log::ParseSessionSlowLogRules("Exec_retry_count > 123").unwrap_err();
    assert!(err.contains("invalid slow log rule format:Exec_retry_count > 123"));

    let err = slow_log::ParseSessionSlowLogRules("Query_time:-1.5").unwrap_err();
    assert!(err.contains("non-negative"));
    let err = slow_log::ParseSessionSlowLogRules("cop_mvcc_read_amplification:-0.1").unwrap_err();
    assert!(err.contains("non-negative"));
    let err = slow_log::ParseSessionSlowLogRules("Mem_max:-100").unwrap_err();
    assert!(err.contains("non-negative"));

    let err = slow_log::ParseSessionSlowLogRules("Query_time:NaN").unwrap_err();
    assert!(err.contains("finite"));
    let err = slow_log::ParseSessionSlowLogRules("Query_time:Inf").unwrap_err();
    assert!(err.contains("finite"));
    let err = slow_log::ParseSessionSlowLogRules("cop_mvcc_read_amplification:+Inf").unwrap_err();
    assert!(err.contains("finite"));

    let slow_log_rules =
        slow_log::ParseSessionSlowLogRules("Mem_max:100,Succ:true,Succ:false,Mem_max:200")
            .unwrap()
            .unwrap();
    assert_eq!(slow_log_rules.rules.len(), 1);
    let mut m = BTreeMap::new();
    for cond in &slow_log_rules.rules[0].conditions {
        m.insert(cond.field.clone(), cond.threshold.clone());
    }
    assert_eq!(
        m.get(&slow_log::SlowLogMemMax.to_ascii_lowercase()),
        Some(&Threshold::Int(200))
    );
    assert_eq!(
        m.get(&slow_log::SlowLogSucc.to_ascii_lowercase()),
        Some(&Threshold::Bool(false))
    );

    let slow_log_rules = slow_log::ParseSessionSlowLogRules("Exec_retry_count:1,  , DB:db")
        .unwrap()
        .unwrap();
    assert_eq!(slow_log_rules.rules[0].conditions.len(), 2);
}

/// 按字段名断言规则中存在期望阈值。
fn check_rule_by_field(rules: &[SlowLogRule], field: &str, val: &str) {
    for r in rules {
        for cond in &r.conditions {
            if cond.field == field {
                assert_eq!(format!("{}", cond.threshold), val);
            }
        }
    }
}

/// 解析全局慢日志规则字符串。
#[test]
fn TestParseGlobalSlowLogRules() {
    let raw_rule = "Conn_ID: 123, Exec_retry_count: 10, DB: db1, Succ: true, Query_time: 0.5276, Resource_group: rg1";
    let slow_log_rule_set = slow_log::ParseGlobalSlowLogRules(raw_rule).unwrap();
    let rules = [SlowLogRule {
        conditions: vec![
            cond(
                &slow_log::SlowLogConnIDStr.to_ascii_lowercase(),
                Threshold::UInt(123),
            ),
            cond(
                &slow_log::SlowLogExecRetryCount.to_ascii_lowercase(),
                Threshold::UInt(10),
            ),
            cond(
                &slow_log::SlowLogDBStr.to_ascii_lowercase(),
                Threshold::String("db1".into()),
            ),
            cond(
                &slow_log::SlowLogSucc.to_ascii_lowercase(),
                Threshold::Bool(true),
            ),
            cond(
                &slow_log::SlowLogQueryTimeStr.to_ascii_lowercase(),
                Threshold::Float(0.5276),
            ),
            cond(
                &slow_log::SlowLogResourceGroup.to_ascii_lowercase(),
                Threshold::String("rg1".into()),
            ),
        ],
    }];
    let all_condition_fields: BTreeSet<String> = [
        slow_log::SlowLogConnIDStr,
        slow_log::SlowLogExecRetryCount,
        slow_log::SlowLogDBStr,
        slow_log::SlowLogSucc,
        slow_log::SlowLogQueryTimeStr,
        slow_log::SlowLogResourceGroup,
    ]
    .into_iter()
    .map(|s| s.to_ascii_lowercase())
    .collect();
    assert_eq!(slow_log_rule_set.rules_map.len(), 1);
    compare_conditions_unordered(
        &rules[0].conditions,
        &slow_log_rule_set.rules_map[&123].rules[0].conditions,
        raw_rule,
    );
    assert_eq!(
        all_condition_fields,
        slow_log_rule_set.rules_map[&123].fields
    );
    assert!(!slow_log_rule_set.rules_map.contains_key(&UnsetConnID));

    let slow_log_rule_set = slow_log::ParseGlobalSlowLogRules("").unwrap();
    assert!(slow_log_rule_set.rules_map.is_empty());
    assert_eq!(slow_log_rule_set.raw_rules, "");
    assert_eq!(slow_log_rule_set.raw_rules_hash, 0);

    let slow_log_rule_set = slow_log::ParseGlobalSlowLogRules("Conn_id:123;").unwrap();
    assert_eq!(
        slow_log_rule_set.rules_map[&123].rules[0].conditions[0].threshold,
        Threshold::UInt(123)
    );
    assert_eq!(slow_log_rule_set.raw_rules, "conn_id:123");

    let err = slow_log::ParseGlobalSlowLogRules("Conn_ID: -123, DB: db1;").unwrap_err();
    assert!(err.contains("invalid slow log format"));

    let mut all_condition_fields: BTreeSet<String> =
        [slow_log::SlowLogConnIDStr, slow_log::SlowLogDBStr]
            .into_iter()
            .map(|s| s.to_ascii_lowercase())
            .collect();
    let raw_rule =
        "Conn_ID: 123, DB: db1; Conn_ID: 456, DB: db2; DB: db3; Conn_ID: 789; Conn_ID: 123;;";
    let slow_log_rule_set = slow_log::ParseGlobalSlowLogRules(raw_rule).unwrap();
    assert!(
        slow_log_rule_set
            .raw_rules
            .contains("conn_id:123,db:db1;conn_id:123")
            || slow_log_rule_set
                .raw_rules
                .contains("db:db1,conn_id:123;conn_id:123")
    );
    assert!(
        slow_log_rule_set.raw_rules.contains("conn_id:456,db:db2")
            || slow_log_rule_set.raw_rules.contains("db:db2,conn_id:456")
    );
    assert!(slow_log_rule_set.raw_rules.contains("db:db3"));
    assert!(slow_log_rule_set.raw_rules.contains("conn_id:789"));
    assert_eq!(slow_log_rule_set.rules_map.len(), 4);

    check_rule_by_field(
        &slow_log_rule_set.rules_map[&123].rules,
        slow_log::SlowLogDBStr,
        "db1",
    );
    check_rule_by_field(
        &slow_log_rule_set.rules_map[&123].rules,
        slow_log::SlowLogConnIDStr,
        "123",
    );
    assert_eq!(slow_log_rule_set.rules_map[&123].raw_rules, "");
    assert_eq!(
        all_condition_fields,
        slow_log_rule_set.rules_map[&123].fields
    );

    check_rule_by_field(
        &slow_log_rule_set.rules_map[&456].rules,
        slow_log::SlowLogDBStr,
        "db2",
    );
    assert_eq!(slow_log_rule_set.rules_map[&456].raw_rules, "");
    assert_eq!(
        all_condition_fields,
        slow_log_rule_set.rules_map[&456].fields
    );

    assert_eq!(
        slow_log_rule_set.rules_map[&UnsetConnID].rules[0].conditions[0].threshold,
        Threshold::String("db3".into())
    );
    assert_eq!(slow_log_rule_set.rules_map[&UnsetConnID].raw_rules, "");
    all_condition_fields = [slow_log::SlowLogDBStr]
        .into_iter()
        .map(|s| s.to_ascii_lowercase())
        .collect();
    assert_eq!(
        all_condition_fields,
        slow_log_rule_set.rules_map[&UnsetConnID].fields
    );

    assert_eq!(
        slow_log_rule_set.rules_map[&789].rules[0].conditions[0].threshold,
        Threshold::UInt(789)
    );
}

/// Go executor init: Plan_digest parses strings and lowercases only the item.
#[test]
fn TestPlanDigestRuleAccessor() {
    let mut vars = new_mock_vars();
    let accessor = &SlowLogRuleFieldAccessors["plan_digest"];
    let parsed = slow_log::ParseSessionSlowLogRules("Plan_digest:ab12")
        .unwrap()
        .unwrap();
    vars.SlowLogRules.SlowLogRules = Some(parsed);
    let mut items = SlowQueryLogItems {
        PlanDigest: "AB12".into(),
        ..Default::default()
    };
    assert!(match_rules(&vars, &items));
    assert!(!(accessor.Match)(
        None,
        &items,
        &Threshold::String("AB12".into())
    ));
    assert!(!(accessor.Match)(
        None,
        &items,
        &Threshold::String("other".into())
    ));
    assert_eq!(
        slow_log::ParseSlowLogFieldValue("PLAN_DIGEST", "AB12").unwrap(),
        Threshold::String("AB12".into())
    );

    // Cached digest is used even without a plan; no digest yields the empty string.
    vars.StmtCtx.SetPlanDigest(
        "cached plan",
        Some(astersql_parser::digester_impl::NewDigest(vec![0xab, 0x12])),
    );
    (accessor.Setter.as_ref().unwrap())(&SlowLogExecContext::default(), Some(&vars), &mut items);
    assert_eq!(items.PlanDigest, "ab12");
    let empty = new_mock_vars();
    (accessor.Setter.as_ref().unwrap())(&SlowLogExecContext::default(), Some(&empty), &mut items);
    assert_eq!(items.PlanDigest, "");
    assert!(empty.StmtCtx.GetPlanDigest().1.unwrap().Bytes().is_empty());
}

/// Uncached physical/DML plans must be flattened, normalized and cached by the setter.
#[test]
fn TestPlanDigestSetterBuildsMissingCache() {
    use astersql_planner_core::{FlattenPhysicalPlan, NormalizeFlatPlan, PlanKind, PlanNode};
    for wrapper in [
        None,
        Some(PlanKind::Insert),
        Some(PlanKind::Update),
        Some(PlanKind::Delete),
    ] {
        let select = PlanNode::New(2, PlanKind::TableScan { table: "t".into() }, vec![]);
        let plan = match wrapper {
            Some(kind) => PlanNode::New(1, kind, vec![select]),
            None => select,
        };
        let expected_flat = FlattenPhysicalPlan(Some(&plan), false).unwrap();
        let (normalized, digest) = NormalizeFlatPlan(&expected_flat);
        assert!(!normalized.is_empty());
        let mut vars = new_mock_vars();
        vars.StmtCtx.SetPlan(Some(Arc::new(plan)));
        let mut items = SlowQueryLogItems::default();
        let setter = SlowLogRuleFieldAccessors["plan_digest"]
            .Setter
            .as_ref()
            .unwrap();
        setter(&SlowLogExecContext::default(), Some(&vars), &mut items);
        assert_eq!(items.PlanDigest, digest.String());
        let cached = vars.StmtCtx.GetPlanDigest();
        assert_eq!(cached.0, normalized);
        assert_eq!(cached.1.unwrap().String(), digest.String());
        let flat = vars.StmtCtx.GetFlatPlan().unwrap();
        // An empty normalized cache must regenerate the digest using the cached flat plan.
        vars.StmtCtx.SetPlanDigest(
            "",
            Some(astersql_parser::digester_impl::NewDigest(Vec::new())),
        );
        setter(&SlowLogExecContext::default(), Some(&vars), &mut items);
        assert!(Arc::ptr_eq(&flat, &vars.StmtCtx.GetFlatPlan().unwrap()));
        assert_eq!(items.PlanDigest, digest.String());
    }
}
