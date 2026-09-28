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

// `hint` 核心逻辑第二组单测：匹配、语句级 hint、还原与 ParsePlanHints。
//
// 对齐 Go 行为：表名匹配只标记等价项、语句 hint 取最后有效值与 clone 语义、
// 索引/分区还原文案、QB offset 转换，以及未匹配警告顺序与去重。

use super::*;

/// 构造大小写不敏感标识符（CIStr）。
fn ci(value: &str) -> ast::CIStr {
    ast::NewCIStr(value)
}

/// 构造带库名/表名/SelectOffset 的 HintedTable。
fn hinted_table(db: &str, table: &str, offset: i32) -> HintedTable {
    HintedTable {
        DBName: ci(db),
        TblName: ci(table),
        SelectOffset: offset,
        ..Default::default()
    }
}

/// 构造只含单表的 TableOptimizerHint。
fn table_hint(name: &str, table: &str) -> ast::TableOptimizerHint {
    ast::TableOptimizerHint {
        HintName: ci(name),
        Tables: vec![ast::HintTable {
            TableName: ci(table),
            ..Default::default()
        }],
        ..Default::default()
    }
}

/// 测试用 set_var 放行回调：一律允许。
fn allow_set_var(_name: String, _hint: String) -> (bool, Option<errors::Error>) {
    (true, None)
}

/// 不应被调用的 hypo index 检查器；调用则 panic。
fn unused_hypo_checker(
    _db: ast::CIStr,
    _table: ast::CIStr,
    _column: ast::CIStr,
) -> (i32, Option<errors::Error>) {
    panic!("hypo checker must not be called")
}

/// 收集 hint 警告文本的测试桩。
#[derive(Default)]
struct Warnings {
    text: Vec<String>,
}

impl hintWarnHandler for Warnings {
    fn SetHintWarning(&mut self, warn: String) {
        self.text.push(warn);
    }

    fn SetHintWarningFromError(&mut self, err: &dyn std::error::Error) {
        self.text.push(err.to_string());
    }
}

/// IfPreferMergeJoin 仅匹配库表名等价且 offset 相同的项，并置 Matched。
#[test]
fn hint_2_matching_marks_only_go_equivalent_entries() {
    let mut plan = PlanHints {
        SortMergeJoin: vec![
            hinted_table("*", "Orders", 2),
            hinted_table("db", "Other", 2),
        ],
        ..Default::default()
    };

    assert!(plan.IfPreferMergeJoin(vec![hinted_table("sales", "orders", 2)]));
    assert!(plan.SortMergeJoin[0].Matched);
    assert!(!plan.SortMergeJoin[1].Matched);
    assert!(!plan.IfPreferMergeJoin(vec![hinted_table("sales", "orders", 3)]));
}

/// ParseStmtHints：重复定义取最后生效值、offset 列表与 Clone 清空 hypo indexes。
#[test]
fn hint_2_statement_hints_keep_last_values_offsets_and_clone_semantics() {
    let make_data_hint = |name: &str, data: ast::HintData| ast::TableOptimizerHint {
        HintName: ci(name),
        HintData: data,
        ..Default::default()
    };
    let hints = vec![
        make_data_hint(HintMemoryQuota, ast::HintData::Signed(5)),
        make_data_hint(HintMemoryQuota, ast::HintData::Signed(0)),
        make_data_hint(
            "set_var",
            ast::HintData::SetVar(ast::HintSetVar {
                VarName: "x".into(),
                Value: "1".into(),
            }),
        ),
        make_data_hint(
            "set_var",
            ast::HintData::SetVar(ast::HintSetVar {
                VarName: "x".into(),
                Value: "2".into(),
            }),
        ),
        make_data_hint(HintMaxExecutionTime, ast::HintData::Unsigned(9)),
        make_data_hint("nth_plan", ast::HintData::Signed(0)),
        make_data_hint("resource_group", ast::HintData::Name("rg1".into())),
    ];

    let (mut statement, offsets, warnings) =
        ParseStmtHints(hints, allow_set_var, unused_hypo_checker, "db".into(), 1);
    assert_eq!(statement.MemQuotaQuery, 0);
    assert_eq!(statement.MaxExecutionTime, 9);
    assert_eq!(statement.ResourceGroup, "rg1");
    assert_eq!(statement.ForceNthPlan, -1);
    assert_eq!(statement.SetVars.get("x").map(String::as_str), Some("1"));
    assert_eq!(offsets, vec![1, 2, 4, 6]);
    assert_eq!(warnings.len(), 4);
    assert_eq!(
        warnings[1].to_string(),
        "MEMORY_QUOTA() is defined more than once, only the last definition takes effect: MEMORY_QUOTA(0)"
    );

    statement.addHypoIndex("db".into(), "t".into(), "idx".into(), Default::default());
    let cloned = statement.Clone();
    assert!(cloned.HintedHypoIndexes.is_empty());
    assert_eq!(cloned.SetVars, statement.SetVars);
    assert_eq!(cloned.OriginalTableHints, statement.OriginalTableHints);
}

/// HintedIndex 匹配、pushdown 标记与 Restore2IndexHint 文案对齐 Go。
#[test]
fn hint_2_index_metadata_and_restore_match_go_text() {
    let index = HintedIndex {
        DBName: ci("Sales"),
        TblName: ci("Orders"),
        Partitions: vec![ci("P0")],
        IndexHint: Some(ast::IndexHint {
            IndexNames: vec![ci("Idx_A"), ci("Idx_B")],
            HintType: ast::HintUse,
            HintScope: ast::HintForScan,
        }),
        PushDownLookUp: true,
        ..Default::default()
    };

    assert!(index.Match(ci("sales"), ci("orders")));
    assert!(index.ShouldPushDownIndexLookUp());
    assert_eq!(index.HintTypeString(), HintIndexLookUpPushDown);
    assert_eq!(index.IndexString(), "Sales.Orders, idx_a, idx_b");
    assert_eq!(
        Restore2IndexHint(HintUseIndex, index),
        "/*+ USE_INDEX(orders PARTITION(p0) idx_a, idx_b) */"
    );
}

/// 表/Join/Storage hint 还原时分区列表空格与大小写对齐 Go。
#[test]
fn hint_2_restore_helpers_preserve_partition_spacing() {
    let tables = vec![
        HintedTable {
            TblName: ci("T1"),
            Partitions: vec![ci("P0"), ci("P1")],
            ..Default::default()
        },
        hinted_table("db", "T2", 0),
    ];

    assert_eq!(
        restore2TableHint(tables.clone()),
        "t1 PARTITION(p0, p1), t2"
    );
    assert_eq!(
        Restore2JoinHint(HintSMJ, tables.clone()),
        "/*+ MERGE_JOIN(t1 PARTITION(p0, p1), t2) */"
    );
    assert_eq!(Restore2JoinHint(HintSMJ, vec![]), "MERGE_JOIN");
    assert_eq!(
        Restore2StorageHint(vec![tables[0].clone()], vec![tables[1].clone()]),
        "/*+ READ_FROM_STORAGE(tiflash[t1 PARTITION(p0, p1)], tikv[t2]) */"
    );
}

/// tableNames2HintTableInfo：按 QB 名解析 SelectOffset；Join hint 拒绝分区列表。
#[test]
fn hint_2_table_conversion_uses_qb_offset_and_rejects_join_partitions() {
    let mut processor = QBHintHandler::default();
    processor.QBNameToSelOffset.insert("named".into(), 7);
    let mut warnings = Warnings::default();
    let table = ast::HintTable {
        TableName: ci("T"),
        QBName: ci("named"),
        ..Default::default()
    };

    let converted = tableNames2HintTableInfo(
        "CurrentDB",
        HintUseIndex,
        vec![table.clone()],
        &processor,
        3,
        &mut warnings,
    );
    assert_eq!(converted[0].DBName.L, "currentdb");
    assert_eq!(converted[0].SelectOffset, 7);

    let mut partitioned = table;
    partitioned.PartitionList = vec![ci("p0")];
    assert!(
        tableNames2HintTableInfo(
            "CurrentDB",
            HintSMJ,
            vec![partitioned],
            &processor,
            3,
            &mut warnings,
        )
        .is_empty()
    );
    assert_eq!(warnings.text.len(), 1);
    assert!(warnings.text[0].contains("Optimizer Hint /*+ MERGE_JOIN(t PARTITION(p0)) */"));
}

/// ParsePlanHints 按类型路由到 PlanHints 各字段，并设置子查询相关 flags。
#[test]
fn hint_2_parse_plan_hints_routes_and_flags_without_duplication() {
    let mut use_index = table_hint(HintUseIndex, "Orders");
    use_index.Indexes = vec![ci("PRIMARY")];
    let mut tiflash = table_hint(HintReadFromStorage, "Orders");
    tiflash.HintData = ast::HintData::CIStr(ci(HintTiFlash));
    let hints = vec![
        table_hint(HintSMJ, "Orders"),
        use_index,
        tiflash,
        ast::TableOptimizerHint {
            HintName: ci(HintHashAgg),
            ..Default::default()
        },
        ast::TableOptimizerHint {
            HintName: ci(HintLimitToCop),
            ..Default::default()
        },
        ast::TableOptimizerHint {
            HintName: ci(HintSemiJoinRewrite),
            ..Default::default()
        },
    ];
    let mut processor = QBHintHandler::default();
    let mut warnings = Warnings::default();

    let (plan, flags) = ParsePlanHints(
        hints,
        1,
        "Sales".into(),
        &mut processor,
        false,
        true,
        false,
        false,
        &mut warnings,
    )
    .unwrap();

    assert_eq!(plan.SortMergeJoin.len(), 1);
    assert_eq!(plan.IndexHintList.len(), 1);
    assert_eq!(plan.TiFlashTables.len(), 1);
    assert_eq!(plan.PreferAggType, PreferHashAgg);
    assert!(plan.PreferLimitToCop);
    assert_eq!(flags, HintFlagSemiJoinRewrite);
    assert!(warnings.text.is_empty());
}

/// ParsePlanHints 警告边界：缺表名、NO_DECORRELATE 不适用、leading 至多一个。
#[test]
fn hint_2_parse_plan_hints_keeps_go_warning_boundaries() {
    let hints = vec![
        ast::TableOptimizerHint {
            HintName: ci(HintSMJ),
            ..Default::default()
        },
        table_hint(HintLeading, "t1"),
        table_hint(HintLeading, "t2"),
        ast::TableOptimizerHint {
            HintName: ci(HintNoDecorrelate),
            ..Default::default()
        },
    ];
    let mut processor = QBHintHandler::default();
    let mut warnings = Warnings::default();

    let (plan, flags) = ParsePlanHints(
        hints,
        1,
        "db".into(),
        &mut processor,
        false,
        false,
        false,
        true,
        &mut warnings,
    )
    .unwrap();

    assert!(plan.LeadingJoinOrder.is_empty());
    assert_eq!(flags, 0);
    assert_eq!(warnings.text.len(), 3);
    assert!(warnings.text[0].contains("Please specify the table names"));
    assert!(warnings.text[1].contains("NO_DECORRELATE() is inapplicable"));
    assert!(warnings.text[2].contains("one leading hint at most"));
}

/// CollectUnmatchedHintWarnings 文案与顺序（含 READ_FROM_STORAGE 合并）对齐 Go。
#[test]
fn hint_2_unmatched_warning_order_matches_go() {
    let plan = PlanHints {
        SortMergeJoin: vec![hinted_table("db", "T1", 0)],
        TiFlashTables: vec![hinted_table("db", "T2", 0)],
        TiKVTables: vec![HintedTable {
            Matched: true,
            ..hinted_table("db", "T3", 0)
        }],
        ..Default::default()
    };

    assert_eq!(
        CollectUnmatchedHintWarnings(&plan),
        vec![
            "There are no matching table names for (T1) in optimizer hint /*+ MERGE_JOIN(t1) */ or /*+ TIDB_SMJ(t1) */. Maybe you can use the table alias name",
            "There are no matching table names for (T2) in optimizer hint /*+ READ_FROM_STORAGE(tiflash[t2], tikv[t3]) */. Maybe you can use the table alias name",
        ]
    );
}

/// RemoveDuplicatedHints 按还原文本去重，保留首次出现。
#[test]
fn hint_2_remove_duplicates_keeps_first_restored_hint() {
    let first = table_hint(HintHJ, "T");
    let duplicate = first.clone();
    let second = table_hint(HintSMJ, "T");
    let result = RemoveDuplicatedHints(vec![first.clone(), duplicate, second.clone()]);

    assert_eq!(result, vec![first, second]);
}
