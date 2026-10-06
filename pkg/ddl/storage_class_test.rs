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

use super::storage_class::*;

#[test]
fn storage_class_admission_checks_requests_and_copied_metadata() {
    struct Restore(astersql_config::Config);
    impl Drop for Restore {
        fn drop(&mut self) {
            astersql_config::store_global_config(self.0.clone());
        }
    }
    let _restore = Restore(astersql_config::get_global_config().as_ref().clone());
    astersql_config::update_global(|config| config.enable_storage_class = false);

    assert!(CheckStorageClassAdmission("", None).is_ok());
    for attribute in [
        r#"{"storage_class":"IA"}"#,
        r#"{"storage_class":{"tier":"STANDARD","transitions":[{"tier":"IA","after_days":30}]}}"#,
        r#"{"storage_class":[{"tier":"STANDARD"},{"tier":"IA","names_in":["p1"]}]}"#,
    ] {
        assert!(
            CheckStorageClassAdmission(attribute, None)
                .unwrap_err()
                .contains("enable-storage-class")
        );
    }
    let table = model::TableInfo {
        StorageClassTier: "IA".into(),
        ..Default::default()
    };
    assert!(
        CheckStorageClassAdmission(&table.EngineAttribute, Some(&table))
            .unwrap_err()
            .contains("enable-storage-class")
    );

    astersql_config::update_global(|config| config.enable_storage_class = true);
    assert!(CheckStorageClassAdmission(r#"{"storage_class":"IA"}"#, None).is_ok());
    assert!(CheckStorageClassAdmission(&table.EngineAttribute, Some(&table)).is_ok());
}
#[test]
fn storage_class_json_and_table_settings() {
    for input in [
        "\"STANDARD\"",
        "{\"tier\":\"ia\"}",
        "[]",
        "[{\"tier\":\"IA\",\"names_in\":[\"P0\"]}]",
    ] {
        assert!(
            BuildStorageClassSettingsFromJSON(Some(input)).is_ok(),
            "{input}"
        );
    }
    for input in [
        "null",
        "[null]",
        "{\"tier\":\"STANDARD\",\"unknown\":1}",
        "{\"tier\":\"IA\",\"transitions\":[{\"tier\":\"STANDARD\",\"after_days\":1}]}",
        "{\"tier\":\"IA\"} {}",
        "\"INVALID\"",
    ] {
        assert!(
            BuildStorageClassSettingsFromJSON(Some(input)).is_err(),
            "{input}"
        );
    }
    let settings =
        BuildStorageClassSettingsFromJSON(Some("[{\"tier\":\"IA\",\"names_in\":[\"P0\"]}]"))
            .unwrap();
    let mut table = astersql_meta_model::TableInfo::default();
    BuildStorageClassForTable(&mut table, Some(&settings)).unwrap();
    assert_eq!(table.StorageClassTier, "STANDARD");
    assert_eq!(
        settings.Defs.unwrap()[0]
            .as_ref()
            .unwrap()
            .NamesIn
            .as_ref()
            .unwrap()[0],
        "p0"
    );
}

use astersql_meta_model as model;
use astersql_parser_ast as ast;
fn context() -> astersql_expression_exprstatic::ExprContext {
    astersql_planner_core::InstallPlannerExpressionFactory().unwrap();
    astersql_expression_exprstatic::NewExprContext(Vec::new())
}
fn build(sql: &str) -> model::TableInfo {
    context();
    let stmt = astersql_parser::New().ParseOneStmt(sql, "", "").unwrap();
    let stmt = stmt
        .as_any()
        .downcast_ref::<ast::CreateTableStmt>()
        .unwrap();
    let ctx = astersql_meta_metabuild::NewContext::<(), std::convert::Infallible>(Vec::new());
    crate::BuildTableInfoFromAST(&ctx, stmt).unwrap()
}
#[test]
fn storage_class_options_validate_all_occurrences_and_conflicts() {
    let opt = |tp, value: &str| ast::TableOption {
        Tp: tp,
        StrValue: value.into(),
        ..Default::default()
    };
    use ast::TableOptionType::{EngineAttribute, StorageClass};
    assert_eq!(
        GetEngineAttributeFromStorageClassTableOptions(&[]).unwrap(),
        None
    );
    assert_eq!(
        GetEngineAttributeFromStorageClassTableOptions(&[
            opt(StorageClass, "standard"),
            opt(StorageClass, "ia")
        ])
        .unwrap(),
        Some(r#"{"storage_class":"IA"}"#.into())
    );
    for options in [
        vec![opt(StorageClass, "cold"), opt(StorageClass, "IA")],
        vec![
            opt(EngineAttribute, "{"),
            opt(EngineAttribute, r#"{"storage_class":"IA"}"#),
        ],
        vec![opt(EngineAttribute, r#"{"future":true}"#)],
        vec![
            opt(EngineAttribute, r#"{"storage_class":"IA"}"#),
            opt(StorageClass, "IA"),
        ],
    ] {
        assert!(GetEngineAttributeFromStorageClassTableOptions(&options).is_err());
    }
    let specs = vec![
        ast::AlterTableSpec {
            Tp: ast::AlterTableType::Option,
            Options: vec![opt(StorageClass, "IA")],
            ..Default::default()
        },
        ast::AlterTableSpec {
            Tp: ast::AlterTableType::Option,
            Options: vec![opt(EngineAttribute, r#"{"storage_class":"IA"}"#)],
            ..Default::default()
        },
    ];
    assert!(CheckStorageClassConflictInAlterTableSpecs(&specs).is_err());
    assert!(CheckStorageClassConflictInAlterTableSpecs(&specs[..1]).is_ok());
}
#[test]
fn storage_class_show_create_preserves_scopes_transitions_and_other_fields() {
    for value in [
        r#"{"storage_class":"IA"}"#,
        r#"{"storage_class":{"tier":"ia"}}"#,
        r#"{"storage_class":[{"tier":"ia"}]}"#,
    ] {
        let t = model::TableInfo {
            EngineAttribute: value.into(),
            ..Default::default()
        };
        assert_eq!(
            GetSimpleTableStorageClassForShowCreate(&t).unwrap(),
            Some("IA".into())
        );
    }
    for value in [
        r#"{"storage_class":{"tier":"STANDARD","transitions":[{"tier":"IA","after_days":30}]}}"#,
        r#"{"storage_class":{"tier":"IA","names_in":["p0"]}}"#,
        r#"{"storage_class":"IA","future_field":true}"#,
    ] {
        let t = model::TableInfo {
            EngineAttribute: value.into(),
            ..Default::default()
        };
        assert_eq!(GetSimpleTableStorageClassForShowCreate(&t).unwrap(), None);
    }
    let t = model::TableInfo {
        StorageClassTier: "STANDARD".into(),
        StorageClassTransitions: vec![model::StorageClassTransitRule {
            Tier: "IA".into(),
            AfterDays: 30,
            ..Default::default()
        }],
        ..Default::default()
    };
    assert_eq!(
        t.StorageClassString(),
        r#"{"tier":"STANDARD","transitions":[{"tier":"IA","after_days":30}]}"#
    );
}
#[test]
fn storage_class_checked_add_and_reorganize_definitions() {
    let table = build(
        r#"create table t (id int) ENGINE_ATTRIBUTE='{"storage_class":{"tier":"IA","less_than":"300"}}' partition by range(id) (partition p0 values less than(100),partition p1 values less than(200))"#,
    );
    let mut added = table.Partition.as_ref().unwrap().clone();
    added.Definitions = vec![model::PartitionDefinition {
        Name: ast::NewCIStr("p2"),
        LessThan: vec!["100 + 200".into()],
        ..Default::default()
    }];
    CheckAndUpdateAddedPartitionDefinitions(&context(), &table, &mut added, 2).unwrap();
    assert_eq!(added.Definitions[0].LessThan, vec!["300"]);
    assert_eq!(added.Definitions[0].StorageClassTier, "IA");
    let mut reorganized = build(
        r#"create table t (id int) ENGINE_ATTRIBUTE='{"storage_class":{"tier":"IA","less_than":"200"}}' partition by range(id) (partition p0 values less than(100),partition p1 values less than(100+100),partition p2 values less than(300))"#,
    );
    let mut part_info = reorganized.Partition.as_ref().unwrap().clone();
    part_info.Definitions = part_info.Definitions[1..].to_vec();
    update_checked_definitions(&mut reorganized, &mut part_info, 1).unwrap();
    assert_eq!(
        part_info
            .Definitions
            .iter()
            .map(|d| d.StorageClassTier.as_str())
            .collect::<Vec<_>>(),
        vec!["IA", "STANDARD"]
    );
    assert!(update_checked_definitions(&mut reorganized, &mut part_info, 99).is_err());
}
#[test]
fn storage_class_list_names_defaults_and_remove_partitioning() {
    let mut table = build(
        r#"create table t (id int) ENGINE_ATTRIBUTE='{"storage_class":[{"tier":"IA","values_in":["4"]},{"tier":"STANDARD"}]}' partition by list(id) (partition p0 values in(1,2),partition p1 values in(2+2))"#,
    );
    assert_eq!(
        table.Partition.as_ref().unwrap().Definitions[1].InValues,
        vec![vec!["4"]]
    );
    assert_eq!(
        table.Partition.as_ref().unwrap().Definitions[1].StorageClassTier,
        "IA"
    );
    table.EngineAttribute =
        r#"{"storage_class":[{"tier":"IA","names_in":["P0"]},{"tier":"STANDARD"}]}"#.into();
    rebuild_partitions(&mut table).unwrap();
    assert_eq!(
        table.Partition.as_ref().unwrap().Definitions[0].StorageClassTier,
        "IA"
    );
    table.Partition.as_mut().unwrap().Type = model::ast::model::PartitionTypeNone;
    table.EngineAttribute = r#"{"storage_class":{"tier":"IA","less_than":"invalid"}}"#.into();
    assert!(rebuild_partitions(&mut table).is_ok());
    let before = table.clone();
    table.EngineAttribute = r#"{"future_field":true}"#.into();
    BuildStorageClassForTable(&mut table, None).unwrap();
    rebuild_partitions(&mut table).unwrap();
    assert_eq!(table.StorageClassTier, before.StorageClassTier);
}
#[test]
fn storage_class_range_columns_unsigned_maxvalue_and_invalid_scope() {
    let table = build(
        r#"create table t (s varchar(10) collate utf8mb4_general_ci) ENGINE_ATTRIBUTE='{"storage_class":{"tier":"IA","less_than":"C"}}' partition by range columns(s) (partition p0 values less than('b'),partition p1 values less than('d'))"#,
    );
    assert_eq!(
        table.Partition.as_ref().unwrap().Definitions[0].StorageClassTier,
        "IA"
    );
    assert_eq!(
        table.Partition.as_ref().unwrap().Definitions[1].StorageClassTier,
        "STANDARD"
    );
    let numeric = build(
        "create table t(id bigint unsigned) partition by range(id) (partition p0 values less than(18446744073709551614), partition p1 values less than(MAXVALUE))",
    );
    assert_eq!(
        compare_range(&numeric, "18446744073709551614", "18446744073709551615").unwrap(),
        std::cmp::Ordering::Less
    );
    assert!(compare_range(&numeric, "1", "invalid").is_err());
    let mut hash = numeric.clone();
    hash.Partition.as_mut().unwrap().Type = model::ast::model::PartitionTypeHash;
    let settings =
        BuildStorageClassSettingsFromJSON(Some(r#"{"tier":"IA","names_in":["p0"]}"#)).unwrap();
    let mut defs = hash.Partition.as_ref().unwrap().Definitions.clone();
    assert!(
        BuildStorageClassForPartitions(&mut defs, &hash, Some(&settings))
            .unwrap_err()
            .contains("HASH or KEY")
    );
}

#[test]
fn storage_class_json_go_table_cases() {
    for (name, input, valid) in [
        ("valid string tier", r###""STANDARD""###, true),
        ("invalid string tier", r###""INVALID""###, false),
        (
            "valid no scope",
            r###"{
				"tier": "STANDARD"
			}"###,
            true,
        ),
        (
            "valid names in",
            r###"{
				"tier": "STANDARD",
				"names_in": ["part1", "part2"]
			}"###,
            true,
        ),
        (
            "valid less than",
            r###"{
				"tier": "STANDARD",
				"less_than": "100"
			}"###,
            true,
        ),
        (
            "valid values in",
            r###"{
				"tier": "STANDARD",
				"values_in": ["100", "200"]
			}"###,
            true,
        ),
        (
            "invalid multiple scopes",
            r###"{
				"tier": "STANDARD",
				"names_in": ["part1", "part2"],
				"values_in": ["100", "200"]
			}"###,
            false,
        ),
        (
            "invalid unknown field",
            r###"{
				"tier": "STANDARD",
				"unknown": "100"
			}"###,
            false,
        ),
        (
            "invalid JSON",
            r###"{
				"tier": "STANDARD",
				"names_in": ["part1", "part2"
			}"###,
            false,
        ),
        (
            "invalid trailing JSON",
            r###"{"tier":"STANDARD"} {"tier":"IA"}"###,
            false,
        ),
        (
            "multiple tiers",
            r###"[
				{"tier": "IA", "names_in": ["part1", "part2"]},
				{"tier": "STANDARD"}
			]"###,
            true,
        ),
        (
            "multiple tiers normalized",
            r###"[
				{"tier": "ia", "names_in": ["Part1"]},
				{"tier": "standard", "transitions": [{"tier": "ia", "after_days": 30}]}
			]"###,
            true,
        ),
        (
            "invalid unknown field in list",
            r###"[{"tier": "STANDARD", "unknown": "100"}]"###,
            false,
        ),
        ("invalid null def in list", r###"[null]"###, false),
        (
            "invalid null def mixed in list",
            r###"[{"tier": "STANDARD"}, null]"###,
            false,
        ),
        (
            "valid transitions",
            r###"{
				"tier": "STANDARD",
				"transitions": [{"tier": "IA", "after_days": 30}]
			}"###,
            true,
        ),
        (
            "redundant transitions",
            r###"{
				"tier": "STANDARD",
				"transitions": [{"tier": "IA", "after_days": 30}, {"tier": "IA", "after_days": 60}]
			}"###,
            false,
        ),
        (
            "transitions from cold to hot",
            r###"{
				"tier": "IA",
				"transitions": [{"tier": "STANDARD", "after_days": 30}]
			}"###,
            false,
        ),
        (
            "transitions from cold to hot 2",
            r###"{
				"tier": "STANDARD",
				"transitions": [{"tier": "IA", "after_days": 15}, {"tier": "STANDARD", "after_days": 30}]
			}"###,
            false,
        ),
        (
            "transitions with transit time of 0",
            r###"{
				"tier": "STANDARD",
				"transitions": [{"tier": "IA", "after_days": 0}]
			}"###,
            false,
        ),
    ] {
        assert_eq!(
            BuildStorageClassSettingsFromJSON(Some(input)).is_ok(),
            valid,
            "{name}: {input}"
        );
    }
}

#[test]
fn storage_class_go_json_zero_values_duplicates_and_transition_boundaries() {
    let settings=BuildStorageClassSettingsFromJSON(Some(r#"{"TIER":"IA","tier":"standard","names_in":null,"transitions":[{"tier":"IA","after_days":null,"after_seconds":1}]}"#)).unwrap();
    let def = settings.Defs.as_ref().unwrap()[0].as_ref().unwrap();
    assert_eq!(def.Tier, "STANDARD");
    assert_eq!(def.Transitions.as_ref().unwrap()[0].TotalSeconds(), 1);
    for value in [
        r#"{"tier":"STANDARD","transitions":[{"tier":"IA"}]}"#,
        r#"{"tier":"STANDARD","transitions":[{"tier":"IA","after_days":-1}]}"#,
        r#"{"tier":"STANDARD","transitions":[{"tier":"IA","after_days":1,"extra":true}]}"#,
        r#"{"tier":"IA","less_than":"1","names_in":["p0"]}"#,
    ] {
        assert!(BuildStorageClassSettingsFromJSON(Some(value)).is_err());
    }
    let attribute = model::ParseEngineAttributeFromString(
        r#"{"Storage_Class":"STANDARD","STORAGE_CLASS": "IA", "future_field":true}"#,
    )
    .unwrap();
    assert_eq!(attribute.StorageClass.unwrap().get(), r#""IA""#);
    let mut table = model::TableInfo::default();
    handle_create(
        r#"{"storage_class":[{"tier":"IA"},{"tier":"STANDARD"}]}"#,
        &mut table,
    )
    .unwrap();
    assert_eq!(table.StorageClassTier, "IA");
    let mut parts = vec![
        model::PartitionDefinition {
            Name: ast::NewCIStr("p0"),
            ..Default::default()
        },
        model::PartitionDefinition {
            Name: ast::NewCIStr("p1"),
            ..Default::default()
        },
    ];
    let settings = BuildStorageClassSettingsFromJSON(Some(
        r#"[{"tier":"STANDARD"},{"tier":"IA","names_in":["p0"]}]"#,
    ))
    .unwrap();
    BuildStorageClassForPartitions(&mut parts, &table, Some(&settings)).unwrap();
    assert_eq!(
        parts
            .iter()
            .map(|d| d.StorageClassTier.as_str())
            .collect::<Vec<_>>(),
        vec!["IA", "STANDARD"]
    );
    let mut cloned = parts[0].clone();
    cloned
        .StorageClassTransitions
        .push(model::StorageClassTransitRule {
            Tier: "IA".into(),
            AfterDays: 1,
            ..Default::default()
        });
    assert!(parts[0].StorageClassTransitions.is_empty());
}
