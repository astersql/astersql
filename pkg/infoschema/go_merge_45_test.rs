// Copyright 2026 AsterSQL.

use std::collections::HashMap;
use std::sync::Arc;

use crate::builder::{
    ActionType, AffectedOption, MetadataReader, NewBuilder, SchemaDiff,
    needRefreshMaskingPoliciesForTableDiff,
};
use crate::cluster::{ClusterTableCopDestination, GetClusterTableCopDestination};
use crate::infoschema::{
    CiString, DBInfo, MaskingPolicyInfo, PolicyInfo, Table, TableInfo, infoSchema,
};
use crate::infoschema_v2::NewData;
use crate::metric_table_def::MetricTableMap;
use crate::tables::{
    GetStorageClassTransitionsTableColumns, TableSlowQuery, TableStatementsSummary,
    TableStorageClassTransitions, TableTables, buildTableMeta,
    information_schema_db_with_storage_class, table_registry,
};

struct Reader {
    tables: HashMap<i64, TableInfo>,
}

impl MetadataReader for Reader {
    fn database(&self, _id: i64) -> Result<Option<DBInfo>, String> {
        Ok(None)
    }

    fn table(&self, _schema_id: i64, table_id: i64) -> Result<Option<TableInfo>, String> {
        Ok(self.tables.get(&table_id).cloned())
    }
}

fn table(id: i64, name: &str) -> TableInfo {
    TableInfo {
        id,
        db_id: 1,
        name: CiString::new(name),
        ..Default::default()
    }
}

fn modeled_table(id: i64, name: &str, mut model: astersql_meta_model::TableInfo) -> TableInfo {
    model.ID = id;
    model.DBID = 1;
    model.Name = astersql_parser_ast::NewCIStr(name);
    (*Table::from_model(model).0).clone()
}

#[test]
fn go_merge_45_mview_cutover_replaces_shadow_name_in_both_schemas() {
    for use_v2 in [false, true] {
        let mut db = DBInfo {
            id: 1,
            name: CiString::new("db"),
            tables: vec![
                Arc::new(table(10, "mv")),
                Arc::new(table(20, "__mv_shadow")),
            ],
            ..Default::default()
        };
        let mut builder = NewBuilder(0, NewData(), use_v2);
        builder.InitWithDBInfos(std::slice::from_mut(&mut db), vec![], vec![], 1);
        let reader = Reader {
            tables: [(20, table(20, "mv"))].into_iter().collect(),
        };
        let affected = builder
            .ApplyDiff(
                &reader,
                &SchemaDiff {
                    version: 2,
                    action_type: ActionType::MViewRefreshOutOfPlaceCutover,
                    schema_id: 1,
                    table_id: 20,
                    old_table_id: 10,
                    ..Default::default()
                },
            )
            .unwrap();
        assert!(affected.contains(&10));
        assert!(affected.contains(&20));
        let schema = builder.Build(2);
        assert!(schema.TableByID(10).is_none());
        assert_eq!(schema.TableByID(20).unwrap().Meta().name.original, "mv");
        assert!(
            schema
                .TableByName(&CiString::new("db"), &CiString::new("__mv_shadow"))
                .is_err()
        );
        assert_eq!(
            schema
                .TableByName(&CiString::new("db"), &CiString::new("mv"))
                .unwrap()
                .Meta()
                .id,
            20,
        );
    }
}

#[test]
fn go_merge_45_mview_drop_reloads_related_table_in_both_schemas() {
    for use_v2 in [false, true] {
        let mut db = DBInfo {
            id: 1,
            name: CiString::new("db"),
            tables: vec![Arc::new(table(10, "mv")), Arc::new(table(30, "base_old"))],
            ..Default::default()
        };
        let mut builder = NewBuilder(0, NewData(), use_v2);
        builder.InitWithDBInfos(std::slice::from_mut(&mut db), vec![], vec![], 1);
        let reader = Reader {
            tables: [(30, table(30, "base_new"))].into_iter().collect(),
        };
        let affected = builder
            .ApplyDiff(
                &reader,
                &SchemaDiff {
                    version: 2,
                    action_type: ActionType::DropMaterializedView,
                    schema_id: 1,
                    table_id: 10,
                    affected_options: vec![AffectedOption {
                        schema_id: 1,
                        table_id: 30,
                        old_schema_id: 1,
                        old_table_id: 30,
                    }],
                    ..Default::default()
                },
            )
            .unwrap();
        assert!(affected.contains(&10));
        assert!(affected.contains(&30));
        let schema = builder.Build(2);
        assert!(schema.TableByID(10).is_none());
        assert_eq!(
            schema.TableByID(30).unwrap().Meta().name.original,
            "base_new"
        );
    }
}

#[test]
fn go_merge_45_mview_drop_preserves_base_placement_bundle() {
    for use_v2 in [false, true] {
        let mut base = table(30, "base");
        base.model_meta = Some(Arc::new(astersql_meta_model::TableInfo {
            ID: 30,
            PlacementPolicyRef: Some(astersql_meta_model::PolicyRefInfo {
                ID: 7,
                ..Default::default()
            }),
            ..Default::default()
        }));
        let mut db = DBInfo {
            id: 1,
            name: CiString::new("db"),
            tables: vec![Arc::new(base.clone()), Arc::new(table(10, "mv"))],
            ..Default::default()
        };
        let data = NewData();
        let mut builder = NewBuilder(0, data.clone(), use_v2);
        builder.InitWithDBInfos(
            std::slice::from_mut(&mut db),
            vec![PolicyInfo {
                id: 7,
                name: CiString::new("p"),
            }],
            vec![],
            1,
        );
        let old = builder.Build(1);
        assert!(old.PlacementBundleByPhysicalTableID(30).is_some());
        let mut builder = NewBuilder(0, data, use_v2);
        builder.InitWithOldInfoSchema(old.as_ref());
        let reader = Reader {
            tables: [(30, base)].into_iter().collect(),
        };
        builder
            .ApplyDiff(
                &reader,
                &SchemaDiff {
                    version: 2,
                    action_type: ActionType::DropMaterializedView,
                    schema_id: 1,
                    table_id: 10,
                    affected_options: vec![AffectedOption {
                        schema_id: 1,
                        table_id: 30,
                        old_schema_id: 1,
                        old_table_id: 30,
                    }],
                    ..Default::default()
                },
            )
            .unwrap();
        let schema = builder.Build(2);
        assert!(schema.PlacementBundleByPhysicalTableID(30).is_some());
    }
}

#[test]
fn go_merge_45_cutover_reloads_base_log_and_mview_metadata() {
    for use_v2 in [false, true] {
        let old_base = modeled_table(
            30,
            "base",
            astersql_meta_model::TableInfo {
                MaterializedViewBase: Some(astersql_meta_model::MaterializedViewBaseInfo {
                    MLogID: 40,
                    MViewIDs: vec![10],
                }),
                ..Default::default()
            },
        );
        let old_log = modeled_table(
            40,
            "$mlog$base",
            astersql_meta_model::TableInfo {
                MaterializedViewLog: Some(astersql_meta_model::MaterializedViewLogInfo {
                    BaseTableID: 30,
                    DependentMViewIDs: vec![10],
                    ..Default::default()
                }),
                ..Default::default()
            },
        );
        let old_mv = modeled_table(
            10,
            "mv",
            astersql_meta_model::TableInfo {
                MaterializedView: Some(astersql_meta_model::MaterializedViewInfo {
                    BaseTableIDs: vec![30],
                    ..Default::default()
                }),
                ..Default::default()
            },
        );
        let shadow = modeled_table(
            20,
            "__mv_shadow",
            astersql_meta_model::TableInfo {
                MaterializedViewShadow: Some(astersql_meta_model::MaterializedViewShadowInfo {
                    SourceMViewID: 10,
                }),
                ..Default::default()
            },
        );
        let mut db = DBInfo {
            id: 1,
            name: CiString::new("db"),
            tables: vec![
                Arc::new(old_base),
                Arc::new(old_log),
                Arc::new(old_mv),
                Arc::new(shadow),
            ],
            ..Default::default()
        };
        let data = NewData();
        let mut builder = NewBuilder(0, data.clone(), use_v2);
        builder.InitWithDBInfos(std::slice::from_mut(&mut db), vec![], vec![], 1);
        let old = builder.Build(1);
        let mut builder = NewBuilder(0, data, use_v2);
        builder.InitWithOldInfoSchema(old.as_ref());
        let new_base = modeled_table(
            30,
            "base",
            astersql_meta_model::TableInfo {
                MaterializedViewBase: Some(astersql_meta_model::MaterializedViewBaseInfo {
                    MLogID: 40,
                    MViewIDs: vec![20],
                }),
                ..Default::default()
            },
        );
        let new_log = modeled_table(
            40,
            "$mlog$base",
            astersql_meta_model::TableInfo {
                MaterializedViewLog: Some(astersql_meta_model::MaterializedViewLogInfo {
                    BaseTableID: 30,
                    DependentMViewIDs: vec![20],
                    ..Default::default()
                }),
                ..Default::default()
            },
        );
        let new_mv = modeled_table(
            20,
            "mv",
            astersql_meta_model::TableInfo {
                MaterializedView: Some(astersql_meta_model::MaterializedViewInfo {
                    BaseTableIDs: vec![30],
                    ..Default::default()
                }),
                ..Default::default()
            },
        );
        let reader = Reader {
            tables: [(20, new_mv), (30, new_base), (40, new_log)]
                .into_iter()
                .collect(),
        };
        builder
            .ApplyDiff(
                &reader,
                &SchemaDiff {
                    version: 2,
                    action_type: ActionType::MViewRefreshOutOfPlaceCutover,
                    schema_id: 1,
                    table_id: 20,
                    old_table_id: 10,
                    affected_options: vec![
                        AffectedOption {
                            schema_id: 1,
                            old_schema_id: 1,
                            table_id: 30,
                            old_table_id: 30,
                        },
                        AffectedOption {
                            schema_id: 1,
                            old_schema_id: 1,
                            table_id: 40,
                            old_table_id: 40,
                        },
                    ],
                    ..Default::default()
                },
            )
            .unwrap();
        let schema = builder.Build(2);
        assert!(schema.TableByID(10).is_none());
        let mv = schema.TableByID(20).unwrap().ModelMeta().unwrap();
        assert!(mv.MaterializedView.is_some() && mv.MaterializedViewShadow.is_none());
        assert_eq!(
            schema
                .TableByID(30)
                .unwrap()
                .ModelMeta()
                .unwrap()
                .MaterializedViewBase
                .as_ref()
                .unwrap()
                .MViewIDs,
            vec![20]
        );
        assert_eq!(
            schema
                .TableByID(40)
                .unwrap()
                .ModelMeta()
                .unwrap()
                .MaterializedViewLog
                .as_ref()
                .unwrap()
                .DependentMViewIDs,
            vec![20]
        );
    }
}

#[test]
fn go_merge_45_drop_mview_log_clears_base_relationship() {
    for use_v2 in [false, true] {
        let base = modeled_table(
            30,
            "base",
            astersql_meta_model::TableInfo {
                MaterializedViewBase: Some(astersql_meta_model::MaterializedViewBaseInfo {
                    MLogID: 40,
                    MViewIDs: vec![],
                }),
                PlacementPolicyRef: Some(astersql_meta_model::PolicyRefInfo {
                    ID: 7,
                    ..Default::default()
                }),
                ..Default::default()
            },
        );
        let log = modeled_table(
            40,
            "$mlog$base",
            astersql_meta_model::TableInfo {
                MaterializedViewLog: Some(astersql_meta_model::MaterializedViewLogInfo {
                    BaseTableID: 30,
                    ..Default::default()
                }),
                ..Default::default()
            },
        );
        let mut db = DBInfo {
            id: 1,
            name: CiString::new("db"),
            tables: vec![Arc::new(base), Arc::new(log)],
            ..Default::default()
        };
        let data = NewData();
        let mut builder = NewBuilder(0, data.clone(), use_v2);
        builder.InitWithDBInfos(
            std::slice::from_mut(&mut db),
            vec![PolicyInfo {
                id: 7,
                name: CiString::new("p"),
            }],
            vec![],
            1,
        );
        let old = builder.Build(1);
        let mut builder = NewBuilder(0, data, use_v2);
        builder.InitWithOldInfoSchema(old.as_ref());
        let new_base = modeled_table(
            30,
            "base",
            astersql_meta_model::TableInfo {
                PlacementPolicyRef: Some(astersql_meta_model::PolicyRefInfo {
                    ID: 7,
                    ..Default::default()
                }),
                ..Default::default()
            },
        );
        let reader = Reader {
            tables: [(30, new_base)].into_iter().collect(),
        };
        builder
            .ApplyDiff(
                &reader,
                &SchemaDiff {
                    version: 2,
                    action_type: ActionType::DropMaterializedViewLog,
                    schema_id: 1,
                    table_id: 40,
                    affected_options: vec![AffectedOption {
                        schema_id: 1,
                        old_schema_id: 1,
                        table_id: 30,
                        old_table_id: 30,
                    }],
                    ..Default::default()
                },
            )
            .unwrap();
        let schema = builder.Build(2);
        assert!(schema.TableByID(40).is_none());
        assert!(
            schema
                .TableByID(30)
                .unwrap()
                .ModelMeta()
                .unwrap()
                .MaterializedViewBase
                .is_none()
        );
        assert!(schema.PlacementBundleByPhysicalTableID(30).is_some());
    }
}

#[test]
fn go_merge_45_intermediate_mview_drop_keeps_non_none_table() {
    for use_v2 in [false, true] {
        let old_mv = modeled_table(
            10,
            "mv",
            astersql_meta_model::TableInfo {
                State: astersql_meta_model::StatePublic,
                ..Default::default()
            },
        );
        let mut db = DBInfo {
            id: 1,
            name: CiString::new("db"),
            tables: vec![Arc::new(old_mv)],
            ..Default::default()
        };
        let mut builder = NewBuilder(0, NewData(), use_v2);
        builder.InitWithDBInfos(std::slice::from_mut(&mut db), vec![], vec![], 1);
        let current = modeled_table(
            10,
            "mv",
            astersql_meta_model::TableInfo {
                State: astersql_meta_model::StatePublic,
                ..Default::default()
            },
        );
        let reader = Reader {
            tables: [(10, current)].into_iter().collect(),
        };
        builder
            .ApplyDiff(
                &reader,
                &SchemaDiff {
                    version: 2,
                    action_type: ActionType::DropMaterializedView,
                    schema_id: 1,
                    table_id: 10,
                    ..Default::default()
                },
            )
            .unwrap();
        assert!(builder.Build(2).TableByID(10).is_some());
    }
}

#[test]
fn go_merge_45_create_mview_without_new_id_drops_old_entry() {
    for use_v2 in [false, true] {
        let mut db = DBInfo {
            id: 1,
            name: CiString::new("db"),
            tables: vec![Arc::new(table(10, "mv"))],
            ..Default::default()
        };
        let mut builder = NewBuilder(0, NewData(), use_v2);
        builder.InitWithDBInfos(std::slice::from_mut(&mut db), vec![], vec![], 1);
        let affected = builder
            .ApplyDiff(
                &Reader {
                    tables: HashMap::new(),
                },
                &SchemaDiff {
                    version: 2,
                    action_type: ActionType::CreateMaterializedView,
                    schema_id: 1,
                    table_id: 0,
                    old_table_id: 10,
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(affected, vec![10]);
        assert!(builder.Build(2).TableByID(10).is_none());
    }
}

#[test]
fn go_merge_45_mview_drop_invalidates_masking_cache() {
    assert!(needRefreshMaskingPoliciesForTableDiff(
        ActionType::DropMaterializedView
    ));
    assert!(needRefreshMaskingPoliciesForTableDiff(
        ActionType::DropMaterializedViewLog
    ));
    assert!(!needRefreshMaskingPoliciesForTableDiff(
        ActionType::DropMaterializedViewShadow
    ));
    let old = infoSchema::new(1);
    old.put_masking_policy(MaskingPolicyInfo {
        id: 1,
        table_id: 10,
        column_id: 1,
        ..Default::default()
    });
    old.set_masking_policies_loaded(true);
    for use_v2 in [false, true] {
        let mut builder = NewBuilder(0, NewData(), use_v2);
        builder.InitWithOldInfoSchema(&old);
        builder
            .ApplyDiff(
                &Reader {
                    tables: HashMap::new(),
                },
                &SchemaDiff {
                    version: 2,
                    action_type: ActionType::DropMaterializedView,
                    schema_id: 1,
                    table_id: 10,
                    ..Default::default()
                },
            )
            .unwrap();
        let schema = builder.Build(2);
        let (cache, loaded) = schema.MaskingCacheSnapshot();
        assert!(cache.is_empty());
        assert!(!loaded);
    }
}

#[test]
fn go_merge_45_metrics_and_storage_class_metadata() {
    let ideal = MetricTableMap.get("tidb_qps_ideal").unwrap();
    assert!(
        ideal
            .PromQL
            .contains("tidb_server_handle_command_duration_seconds_count")
    );
    for name in [
        "tidb_ia_remote_read_segment_count",
        "tidb_ia_remote_read_segment_size",
        "tidb_ia_remote_read_segment_wait_time_histogram",
    ] {
        assert!(MetricTableMap.contains_key(name), "missing {name}");
    }
    let storage = table_registry().get(TableStorageClassTransitions).unwrap();
    assert_eq!(
        storage.id,
        astersql_meta_autoid::INFORMATION_SCHEMA_DB_ID + 102
    );
    assert_eq!(storage.columns.len(), 12);
    assert_eq!(storage.columns[0].name, "TABLE_SCHEMA");
    assert_eq!(storage.columns[11].name, "LAST_UPDATE_TIME");
    let storage_model = buildTableMeta(TableStorageClassTransitions, &storage.columns)
        .model_meta
        .unwrap();
    assert_eq!(
        storage_model.Columns[9].GetType(),
        astersql_parser_mysql::r#type::TypeDatetime
    );
    assert_eq!(storage_model.Columns[9].GetDecimal(), 6);
    assert_ne!(
        storage_model.Columns[6].GetFlag() & astersql_parser_mysql::r#type::UnsignedFlag,
        0
    );
    let mut first_columns = GetStorageClassTransitionsTableColumns();
    assert_eq!(first_columns.len(), 12);
    first_columns[0].name = CiString::new("changed");
    assert_eq!(
        GetStorageClassTransitionsTableColumns()[0].name.original,
        "TABLE_SCHEMA"
    );
    assert_eq!(
        GetClusterTableCopDestination(TableStorageClassTransitions),
        ClusterTableCopDestination::DDLOwner
    );
    let tables = table_registry().get(TableTables).unwrap();
    assert!(
        tables
            .columns
            .iter()
            .any(|c| c.name == "TIDB_STORAGE_CLASS")
    );
    for enabled in [true, false, true, false] {
        let db = information_schema_db_with_storage_class(enabled);
        assert_eq!(
            db.tables
                .iter()
                .any(|table| table.name.original == TableStorageClassTransitions),
            enabled,
        );
        assert!(
            db.tables
                .iter()
                .any(|table| table.name.original == TableTables)
        );
    }
    let slow = table_registry().get(TableSlowQuery).unwrap();
    assert_eq!(slow.columns.len(), 97);
    assert_eq!(slow.columns[0].name, "Time");
    assert_eq!(slow.columns[96].name, "Query");
    let slow_model = buildTableMeta(TableSlowQuery, &slow.columns)
        .model_meta
        .unwrap();
    assert_eq!(
        slow_model.Columns[41].GetType(),
        astersql_parser_mysql::r#type::TypeLonglong
    );
    assert_eq!(slow_model.Columns[41].GetFlen(), 20);
    assert_ne!(
        slow_model.Columns[0].GetFlag() & astersql_parser_mysql::r#type::PriKeyFlag,
        0
    );
    for name in [
        "IA_remote_read_segment_count",
        "IA_remote_read_segment_size",
        "IA_remote_read_segment_wait_time",
        "Read_pool_task_details",
    ] {
        assert!(slow.columns.iter().any(|column| column.name == name));
    }
    let summary = table_registry().get(TableStatementsSummary).unwrap();
    assert_eq!(summary.columns.len(), 127);
    assert_eq!(summary.columns[45].name, "IA_EXEC_COUNT");
    assert!(summary.columns[45].unsigned && summary.columns[45].not_null);
    let summary_model = buildTableMeta(TableStatementsSummary, &summary.columns)
        .model_meta
        .unwrap();
    assert_eq!(
        summary_model.Columns[45].GetType(),
        astersql_parser_mysql::r#type::TypeLonglong
    );
    assert_ne!(
        summary_model.Columns[45].GetFlag() & astersql_parser_mysql::r#type::NotNullFlag,
        0
    );
    for name in [
        "IA_EXEC_COUNT",
        "AVG_IA_REMOTE_READ_SEGMENT_COUNT",
        "MAX_IA_REMOTE_READ_SEGMENT_COUNT",
        "AVG_IA_REMOTE_READ_SEGMENT_SIZE",
        "MAX_IA_REMOTE_READ_SEGMENT_SIZE",
        "AVG_IA_REMOTE_READ_SEGMENT_WAIT_TIME",
        "MAX_IA_REMOTE_READ_SEGMENT_WAIT_TIME",
    ] {
        assert!(summary.columns.iter().any(|column| column.name == name));
    }
}

#[test]
fn go_merge_45_storage_class_visibility_through_builder_v1_v2() {
    let name = CiString::new(TableStorageClassTransitions);
    let schema_name = CiString::new("INFORMATION_SCHEMA");
    let stable_id = astersql_meta_autoid::INFORMATION_SCHEMA_DB_ID + 102;
    for use_v2 in [false, true] {
        for enabled in [true, false, true, false] {
            let mut builder = NewBuilder(0, NewData(), use_v2).WithStorageClassEnabled(enabled);
            builder.InitWithDBInfos(&mut [], vec![], vec![], 1);
            let schema = builder.Build(1);
            assert!(schema.SchemaByName(&schema_name).is_some());
            assert_eq!(schema.TableByName(&schema_name, &name).is_ok(), enabled);
            assert_eq!(schema.TableByID(stable_id).is_some(), enabled);
            assert_eq!(
                schema
                    .SchemaTableInfos(&schema_name)
                    .unwrap()
                    .iter()
                    .any(|table| table.name == name),
                enabled,
            );
            assert_eq!(
                schema
                    .SchemaSimpleTableInfos(&schema_name)
                    .unwrap()
                    .iter()
                    .any(|table| table.Name.O == TableStorageClassTransitions),
                enabled,
            );
            assert!(
                schema
                    .TableByName(&schema_name, &CiString::new(TableTables))
                    .is_ok()
            );
        }
        let mut cross_keyspace = NewBuilder(0, NewData(), use_v2).WithCrossKS(true);
        cross_keyspace.InitWithDBInfos(&mut [], vec![], vec![], 1);
        assert!(cross_keyspace.Build(1).SchemaByName(&schema_name).is_none());
    }
}
