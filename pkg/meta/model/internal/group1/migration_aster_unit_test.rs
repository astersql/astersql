// Copyright 2026 AsterSQL.

// 本文件验证 group1 元数据模型与 Go 版本之间的迁移兼容性。
// 这些编号、JSON 字段名和文本表示会进入持久化数据或跨语言边界，不能随 Rust 实现重构而漂移。

use super::*;

// 枚举按底层数字编码；放置策略字段则必须沿用 Go JSON tag 的小写名称。
#[test]
fn persisted_enum_and_policy_json_matches_go_tags() {
    assert_eq!(ast::metadata_json::encode(&StatePublic).unwrap(), b"5");
    assert_eq!(
        ast::metadata_json::encode(&BackfillState::ReadyToMerge).unwrap(),
        b"2"
    );

    let policy = PolicyRefInfo {
        ID: 7,
        Name: ast::NewCIStr("p"),
    };
    let encoded = String::from_utf8(ast::metadata_json::encode(&policy).unwrap()).unwrap();
    assert!(encoded.contains("\"id\":7"), "{encoded}");
    assert!(encoded.contains("\"name\":"), "{encoded}");
    assert!(!encoded.contains("\"ID\":"), "{encoded}");
    assert!(!encoded.contains("\"Name\":"), "{encoded}");
}

// 状态的可读文本和 PascalCase 别名同样属于 Go 兼容接口。
#[test]
fn schema_and_backfill_strings_match_go() {
    assert_eq!(
        SchemaState::WriteReorganization.String(),
        "write reorganization"
    );
    assert_eq!(BackfillState::Running.String(), "backfill state running");
    assert_eq!(StateGlobalTxnOnly.0, 7);
}

// 透明数字类型允许读入未知字节，展示时再分别落到各自的 Go 兼容兜底文本。
#[test]
fn unknown_persisted_states_keep_go_byte_behavior() {
    let schema: SchemaState = ast::metadata_json::decode(b"255").unwrap();
    let backfill: BackfillState = ast::metadata_json::decode(b"255").unwrap();

    assert_eq!(schema.String(), "none");
    assert_eq!(backfill.String(), "backfill state unknown");
}

// 已废弃动作仍占据原持久化编号，防止后续动作误用历史数据中的槽位。
#[test]
fn deprecated_action_tombstones_keep_go_persisted_numbers() {
    assert_eq!(DEPRECATED_ACTION_ADD_COLUMNS, 37);
    assert_eq!(DEPRECATED_ACTION_DROP_COLUMNS, 38);
    assert_eq!(DEPRECATED_ACTION_DROP_INDEXES, 48);
}

// 元数据层复用 parser AST 的标准数字类型，类型身份与序列化结果必须同时一致。
#[test]
fn metadata_ast_types_use_the_canonical_numeric_model_identity() {
    let partition: ast::PartitionType = ast::PartitionType::Range;
    let algorithm: ast::ViewAlgorithm = ast::ViewAlgorithm::Merge;
    let security: ast::ViewSecurity = ast::ViewSecurity::Invoker;
    let check_option: ast::ViewCheckOption = ast::ViewCheckOption::Local;
    let index_type: ast::IndexType = ast::IndexType::Vector;
    let column_choice: ast::ColumnChoice = ast::ColumnChoice::List;

    assert_eq!(partition, ast::model::PartitionTypeRange);
    assert_eq!(algorithm, ast::model::AlgorithmMerge);
    assert_eq!(security, ast::model::SecurityInvoker);
    assert_eq!(check_option, ast::model::CheckOptionLocal);
    assert_eq!(index_type, ast::model::IndexTypeVector);
    assert_eq!(column_choice, ast::model::ColumnList);

    assert_eq!(ast::metadata_json::encode(&partition).unwrap(), b"1");
    assert_eq!(ast::metadata_json::encode(&algorithm).unwrap(), b"1");
    assert_eq!(ast::metadata_json::encode(&security).unwrap(), b"1");
    assert_eq!(ast::metadata_json::encode(&check_option).unwrap(), b"0");
    assert_eq!(ast::metadata_json::encode(&index_type).unwrap(), b"5");
    assert_eq!(ast::metadata_json::encode(&column_choice).unwrap(), b"3");
}

// 时长适配器仅接受模型层约定的日、时、分组合，不能悄然放宽为秒单位。
#[test]
fn duration_adapter_uses_the_go_parser_duration_grammar() {
    assert_eq!(
        duration::ParseDuration("1d").unwrap(),
        std::time::Duration::from_secs(24 * 60 * 60)
    );
    assert_eq!(
        duration::ParseDuration("1h30m").unwrap(),
        std::time::Duration::from_secs(90 * 60)
    );
    assert!(duration::ParseDuration("1s").is_err());
}
