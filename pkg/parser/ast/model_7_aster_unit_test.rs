// Copyright 2026 AsterSQL.

// `model` / `procedure` / `stats` / `sem` 关键行为的单元测试（对齐 Go）。
//
// 覆盖：枚举 Display、`CIStr` JSON 反序列化、存储过程 restore 间距与错误、
// 统计对象去重与 SEM 命令分类。

use crate::{model::*, procedure::*, sem::*, stats::*};

/// 验证表锁/索引/外键字符串、优先级映射与 CIStr JSON 形态。
#[test]
fn model_strings_and_cistr_json_match_go() {
    for (value, expected) in [
        (TableLockNone, "NONE"),
        (TableLockRead, "READ"),
        (TableLockReadLocal, "READ LOCAL"),
        (TableLockReadOnly, "READ ONLY"),
        (TableLockWrite, "WRITE"),
        (TableLockWriteLocal, "WRITE LOCAL"),
    ] {
        assert_eq!(value.to_string(), expected);
    }
    assert_eq!(TableLockType(255).to_string(), "");

    for (value, expected) in [
        (IndexTypeBtree, "BTREE"),
        (IndexTypeHash, "HASH"),
        (IndexTypeRtree, "RTREE"),
        (IndexTypeHypo, "HYPO"),
        (IndexTypeVector, "VECTOR"),
        (IndexTypeInverted, "INVERTED"),
        (IndexTypeHNSW, "HNSW"),
        (IndexTypeFulltext, "FULLTEXT"),
    ] {
        assert_eq!(value.to_string(), expected);
    }
    assert_eq!(IndexTypeInvalid.to_string(), "");

    for (value, expected) in [
        (ReferOptionNoOption, ""),
        (ReferOptionRestrict, "RESTRICT"),
        (ReferOptionCascade, "CASCADE"),
        (ReferOptionSetNull, "SET NULL"),
        (ReferOptionNoAction, "NO ACTION"),
        (ReferOptionSetDefault, "SET DEFAULT"),
    ] {
        assert_eq!(value.to_string(), expected);
    }
    assert_eq!(PriorityValueToName(LowPriorityValue), "LOW");
    assert_eq!(PriorityValueToName(MediumPriorityValue), "MEDIUM");
    assert_eq!(PriorityValueToName(HighPriorityValue), "HIGH");
    assert_eq!(PriorityValueToName(999), "MEDIUM");

    // CIStr：原始大小写进 O，小写进 L；JSON 支持字符串与对象两种输入。
    let mixed = NewCIStr("AbC");
    assert_eq!((mixed.O.as_str(), mixed.L.as_str()), ("AbC", "abc"));
    let from_string: CIStr = serde_json::from_str(r#""TeSt""#).unwrap();
    let from_object: CIStr = serde_json::from_str(r#"{"O":"X","L":"persisted"}"#).unwrap();
    assert_eq!(from_string, NewCIStr("TeSt"));
    assert_eq!(
        (from_object.O.as_str(), from_object.L.as_str()),
        ("X", "persisted")
    );
    assert!(serde_json::from_str::<CIStr>("42").is_err());
}

/// 验证过程参数/跳转 restore，以及标签首尾名不一致时报错。
#[test]
fn procedure_restore_matches_go_spacing_and_errors() {
    for (mode, expected) in [
        (MODE_IN, " IN `Arg` BIGINT"),
        (MODE_OUT, " OUT `Arg` BIGINT"),
        (MODE_INOUT, " INOUT `Arg` BIGINT"),
        (999, "`Arg` BIGINT"),
    ] {
        assert_eq!(
            StoreParameter::new(mode, "Arg", "BIGINT").restore(),
            expected
        );
    }

    assert_eq!(
        ProcedureJump::new("loop_a", false).restore(),
        "ITERATE 'loop_a'"
    );
    assert_eq!(
        ProcedureJump::new("loop_a", true).restore(),
        "LEAVE 'loop_a'"
    );
    let good = ProcedureLabel::new("label", "label", "BEGIN  END");
    assert_eq!(good.restore().unwrap(), "`label`: BEGIN  END `label`");
    // begin/end 标签名不同时应失败。
    let bad = ProcedureLabel::new("begin_label", "end_label", "BEGIN  END");
    assert!(bad.restore().unwrap_err().contains("different names"));
}

/// 验证 REFRESH STATS 去重、库级覆盖表级，以及 SEM 动态命令分类。
#[test]
fn stats_dedup_restore_and_sem_classification_match_go() {
    assert_eq!(
        StatsObject::table("db", "t1").restore().unwrap(),
        "`db`.`t1`"
    );
    // 表对象被同名库对象吸收后，只剩 `DB`.*。
    let mut stmt = RefreshStatsStmt::new(vec![
        StatsObject::table("db", "t1"),
        StatsObject::database("DB"),
        StatsObject::table("db", "t2"),
    ]);
    stmt.dedup();
    assert_eq!(stmt.refresh_objects.len(), 1);
    assert_eq!(stmt.restore().unwrap(), "REFRESH STATS `DB`.*");

    let mut tables = RefreshStatsStmt::new(vec![
        StatsObject::table("db1", "t1"),
        StatsObject::table("db1", "T1"),
        StatsObject::table("db2", "t1"),
    ]);
    tables.dedup();
    assert_eq!(
        tables.restore().unwrap(),
        "REFRESH STATS `db1`.`t1`, `db2`.`t1`"
    );

    let mut global = RefreshStatsStmt::new(vec![
        StatsObject::table("db", "t"),
        StatsObject::global(),
        StatsObject::database("other"),
    ]);
    global.dedup();
    global.refresh_mode = Some(RefreshStatsModeFull);
    global.is_cluster_wide = true;
    assert_eq!(global.restore().unwrap(), "REFRESH STATS *.* FULL CLUSTER");

    let mut invalid_mode = RefreshStatsStmt::new(vec![StatsObject::global()]);
    invalid_mode.refresh_mode = Some(99);
    assert_eq!(
        invalid_mode.restore().unwrap_err(),
        "invalid refresh stats mode: 99"
    );
    let invalid_scope = StatsObject {
        stats_object_scope: 99,
        ..StatsObject::default()
    };
    assert_eq!(
        invalid_scope.restore().unwrap_err(),
        "invalid stats object scope: 99"
    );

    // Insert.replace / DropTable.view / Explain.analyze 改变命令字符串。
    assert_eq!(
        SemStatement::Insert { replace: true }.sem_command(),
        ReplaceCommand
    );
    assert_eq!(
        SemStatement::DropTable { view: true }.sem_command(),
        DropViewCommand
    );
    assert_eq!(
        SemStatement::Explain { analyze: true }.sem_command(),
        ExplainAnalyzeCommand
    );
    assert_eq!(
        SemStatement::Insert { replace: false }.sem_command(),
        InsertCommand
    );
    assert_eq!(
        SemStatement::DropTable { view: false }.sem_command(),
        DropTableCommand
    );
    assert_eq!(
        SemStatement::Explain { analyze: false }.sem_command(),
        ExplainCommand
    );
    assert_eq!(
        SemStatement::Fixed(RefreshStatsCommand).sem_command(),
        RefreshStatsCommand
    );
}
