// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
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

use astersql_br_pkg_checkpoint::CheckpointMetadataForLogRestore;
use astersql_br_pkg_restore::nameroute::{ObjectName, Router};

use crate::restore::{FlagRename, PointRestoreCmd, RestoreConfig};
use crate::restore_name_route::build_restore_name_plan;
use crate::stubs::{FlagSet, FlagValue};
use crate::{DefineRawRestoreFlags, RestoreRawConfig};
use astersql_br_pkg_registry::{NormalizeRegistrationRoutes, RegistrationInfo};
use astersql_br_pkg_restore_log_client::PitrIDMapsFilenameForTask;
use astersql_br_pkg_stream::stubs::{NewDBReplace, NewTableReplace};
use astersql_br_pkg_stream::table_mapping::{FromDBMapProto, NewTableMappingManager};
use std::collections::HashMap;

#[test]
fn name_route_parses_routes_with_table_precedence() {
    let router = Router::parse(&[
        "src:dst".to_string(),
        "src.orders:archive.orders_copy".to_string(),
    ])
    .expect("valid routes");

    assert_eq!(
        router.route("SRC", "orders"),
        ("archive".to_string(), "orders_copy".to_string(), true)
    );
    assert_eq!(
        router.route("src", "customers"),
        ("dst".to_string(), "customers".to_string(), true)
    );
    assert_eq!(
        router.route("other", "orders"),
        ("other".to_string(), "orders".to_string(), false)
    );
}

#[test]
fn name_route_canonicalizes_quoted_identifiers_and_fingerprint() {
    let first = Router::parse(&[
        "a:x".to_string(),
        "`db.1`.`t:a``b`:`db:2`.`t.2``x`".to_string(),
    ])
    .expect("valid routes");
    let second = Router::parse(&[
        "`db.1`.`t:a``b`:`db:2`.`t.2``x`".to_string(),
        "a:x".to_string(),
    ])
    .expect("valid routes");

    assert_eq!(first.canonical_rules(), second.canonical_rules());
    assert_eq!(first.fingerprint(), second.fingerprint());
    assert_eq!(
        first.canonical_rules(),
        &[
            "`a`:`x`".to_string(),
            "`db.1`.`t:a``b`:`db:2`.`t.2``x`".to_string(),
        ]
    );
}

#[test]
fn name_route_rejects_invalid_and_conflicting_rules() {
    for (rules, expected) in [
        (vec!["a:b.t"], "must both name"),
        (vec!["a:b", "A:c"], "duplicate source"),
        (vec!["a:x", "b:X"], "duplicate target"),
        (vec!["a:A"], "maps `a` to itself"),
        (vec!["a-b:c"], "inside backticks"),
    ] {
        let error = Router::parse(&rules.into_iter().map(str::to_string).collect::<Vec<_>>())
            .expect_err("invalid route must fail");
        assert!(error.to_string().contains(expected), "{error}");
    }

    let router = Router::parse(&["a:b".to_string()]).expect("valid route");
    let error = router
        .validate_targets(&[ObjectName::table("a", "t"), ObjectName::table("b", "t")])
        .expect_err("routed and unchanged object must conflict");
    assert!(error.to_string().contains("conflict at target `b`.`t`"));
}

#[test]
fn name_route_restore_config_validation_and_hash() {
    let mut flags = FlagSet::new();
    crate::restore::DefineRestoreFlags(&mut flags);
    flags.Set(
        FlagRename,
        FlagValue::StringArray(vec!["src:dst".to_string()]),
    );
    flags.Set("with-sys-table", FlagValue::Bool(false));

    let mut cfg = RestoreConfig::default();
    cfg.ParseFromFlags(&flags, true).expect("parse routes");
    cfg.validate_name_routing(PointRestoreCmd)
        .expect("PiTR supports routes");

    let first = cfg.Hash(PointRestoreCmd).expect("hash config");
    cfg.Rename = vec!["src:other".to_string()];
    let second = cfg.Hash(PointRestoreCmd).expect("hash changed config");
    assert_ne!(first, second, "route identity participates in hash");
}

#[test]
fn name_route_checkpoint_fingerprint_round_trips_and_is_optional() {
    let legacy: CheckpointMetadataForLogRestore =
        serde_json::from_str(r#"{"restored-ts":7}"#).expect("legacy checkpoint");
    assert!(legacy.RouteFingerprint.is_empty());

    let metadata = CheckpointMetadataForLogRestore {
        RouteFingerprint: "0123456789abcdef".to_string(),
        ..Default::default()
    };
    let json = serde_json::to_string(&metadata).expect("serialize checkpoint");
    assert!(json.contains(r#""route-fingerprint":"0123456789abcdef""#));
    let decoded: CheckpointMetadataForLogRestore =
        serde_json::from_str(&json).expect("deserialize checkpoint");
    assert_eq!(decoded.RouteFingerprint, metadata.RouteFingerprint);
}

#[test]
fn name_route_pitr_id_map_preserves_source_and_target_identity() {
    let mut database = NewDBReplace("target".to_string(), 100);
    database.SourceName = "source".to_string();
    database.SchemaRouted = true;
    let mut table = NewTableReplace("copy".to_string(), 101);
    table.SourceName = "original".to_string();
    table.TargetDBName = "archive".to_string();
    table.TargetDBID = 200;
    table.HasForeignKeys = true;
    table.IsView = true;
    database.TableMap.insert(10, table);

    let mut manager = NewTableMappingManager();
    manager
        .FromDBReplaceMap(Some(HashMap::from([(1, database.clone())])))
        .expect("load map");
    let round_trip = FromDBMapProto(manager.ToProto());
    assert_eq!(round_trip[&1], database);
    assert_eq!(
        round_trip[&1].TableMap[&10].EffectiveDBName(&round_trip[&1]),
        "archive"
    );
    assert_eq!(
        round_trip[&1].TableMap[&10].EffectiveDBID(&round_trip[&1]),
        200
    );
    assert!(round_trip[&1].RestoresDatabaseMetadata());
}

#[test]
fn name_route_registry_identity_is_canonical_and_hashed() {
    let mut first = RegistrationInfo {
        RouteStrings: vec!["b:y".to_string(), "a:x".to_string()],
        ..Default::default()
    };
    let mut second = RegistrationInfo {
        RouteStrings: vec!["a:x".to_string(), "b:y".to_string()],
        ..Default::default()
    };
    NormalizeRegistrationRoutes(&mut first).expect("normalize first");
    NormalizeRegistrationRoutes(&mut second).expect("normalize second");
    assert_eq!(first.RouteStrings, second.RouteStrings);
    assert_eq!(first.RouteHash, second.RouteHash);
    assert_eq!(first.RouteHash.len(), 64);

    second.RouteHash = "wrong".to_string();
    assert!(NormalizeRegistrationRoutes(&mut second).is_err());
}

#[test]
fn name_route_pitr_id_map_filename_is_task_scoped() {
    let first = PitrIDMapsFilenameForTask(1, 2, 3, 4);
    let second = PitrIDMapsFilenameForTask(1, 2, 3, 5);
    assert_ne!(first, second);
    assert_eq!(
        first,
        "pitr_id_maps/v2/cluster-1/upstream-2/restored-3/restore-4.meta"
    );
}

#[test]
fn name_route_table_mapping_binds_shared_target_database_identity() {
    let mut first = NewDBReplace("source_a".to_string(), -1);
    first.SourceName = "source_a".to_string();
    first
        .TableMap
        .insert(11, NewTableReplace("orders".to_string(), -2));
    let mut second = NewDBReplace("source_b".to_string(), -3);
    second.SourceName = "source_b".to_string();
    second
        .TableMap
        .insert(12, NewTableReplace("customers".to_string(), -4));
    let router = Router::parse(&[
        "source_a.orders:archive.orders_copy".to_string(),
        "source_b.customers:archive.customers_copy".to_string(),
    ])
    .unwrap();
    let mut manager = NewTableMappingManager();
    manager
        .FromDBReplaceMap(Some(HashMap::from([(1, first), (2, second)])))
        .unwrap();
    manager
        .BindUnresolvedTableRoutes(|schema, table| router.route(schema, table))
        .unwrap();
    let first_target = &manager.DBReplaceMap[&1].TableMap[&11];
    let second_target = &manager.DBReplaceMap[&2].TableMap[&12];
    assert_eq!(first_target.TargetDBName, "archive");
    assert_eq!(first_target.TargetDBID, second_target.TargetDBID);
    assert!(first_target.TargetDBID < 0);

    manager
        .ReplaceTemporaryIDs(|count| Ok((100..100 + count as i64).collect()))
        .unwrap();
    assert_eq!(
        manager.DBReplaceMap[&1].TableMap[&11].TargetDBID,
        manager.DBReplaceMap[&2].TableMap[&12].TargetDBID
    );
}

#[test]
fn name_route_restore_plan_uses_source_names_and_rejects_collisions() {
    let router = Router::parse(&["src:dst".to_string(), "src.orders:archive.copy".to_string()])
        .expect("router");
    let plan = build_restore_name_plan(
        &router,
        &[
            ObjectName::schema("src"),
            ObjectName::table("src", "orders"),
            ObjectName::table("src", "customers"),
        ],
    )
    .expect("plan");
    assert_eq!(plan.entries[1].target, ObjectName::table("archive", "copy"));
    assert_eq!(
        plan.entries[2].target,
        ObjectName::table("dst", "customers")
    );

    let colliding = Router::parse(&["src:dst".to_string()]).expect("router");
    let error = build_restore_name_plan(
        &colliding,
        &[ObjectName::table("src", "t"), ObjectName::table("dst", "t")],
    )
    .expect_err("target collision");
    assert!(error.to_string().contains("conflict at target"));
}

#[test]
fn name_route_raw_restore_rejects_inherited_rename_flag() {
    let mut flags = FlagSet::new();
    DefineRawRestoreFlags(&mut flags);
    flags.DefineStringArray(FlagRename, Vec::new());
    flags.Set(
        FlagRename,
        FlagValue::StringArray(vec!["source:target".to_string()]),
    );
    let error = RestoreRawConfig::default()
        .ParseFromFlags(&flags)
        .expect_err("raw restore must reject rename");
    assert!(error.to_string().contains("not supported"));
}
