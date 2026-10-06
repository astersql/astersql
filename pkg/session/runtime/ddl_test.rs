// Copyright 2026 AsterSQL.

use super::*;
use crate::runtime::{ConcreteSession, CreateAnalyzeSession};

#[test]
fn create_database_if_not_exists_honors_persisted_empty_schema() {
    let (domain, _) = CreateAnalyzeSession().unwrap();
    // An empty persisted schema has no stats-table entry and is absent from
    // this session's runtime-only CREATE DATABASE registry after reopening.
    domain
        .ddl_create_database("persisted_empty", false)
        .unwrap();
    let session = ConcreteSession::new(domain);
    session
        .execute("create database if not exists persisted_empty")
        .unwrap();
    session.execute("use persisted_empty").unwrap();
    assert!(session.execute("create database persisted_empty").is_err());
}

#[test]
fn pre_split_skips_global_temporary_tables_like_go() {
    struct SplitTableRegionReset(u32);

    impl Drop for SplitTableRegionReset {
        fn drop(&mut self) {
            astersql_ddl::EnableSplitTableRegion.store(self.0, Ordering::SeqCst);
        }
    }

    let previous = astersql_ddl::EnableSplitTableRegion.swap(1, Ordering::SeqCst);
    let _reset = SplitTableRegionReset(previous);
    let (domain, _) = CreateAnalyzeSession().unwrap();
    domain
        .ddl_create_table(
            "test",
            astersql_meta_model::TableInfo {
                Name: ast::NewCIStr("global_temp"),
                TempTableType: astersql_meta_model::TempTableGlobal,
                PreSplitRegions: 3,
                ..Default::default()
            },
            false,
        )
        .unwrap();
    let session = ConcreteSession::new(Arc::clone(&domain));

    session
        .pre_split_and_scatter("test", "global_temp")
        .unwrap();

    assert!(
        !RUNTIME_REGION_COUNTS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .contains_key(&(
                runtime_domain_id(&domain),
                "test".to_owned(),
                "global_temp".to_owned(),
                None,
            )),
        "Go skips region splitting for every temporary-table type"
    );
}

#[test]
fn explicit_pre_split_config_overrides_disabled_implicit_table_split() {
    struct SplitTableRegionReset(u32);

    impl Drop for SplitTableRegionReset {
        fn drop(&mut self) {
            astersql_ddl::EnableSplitTableRegion.store(self.0, Ordering::SeqCst);
        }
    }

    let previous = astersql_ddl::EnableSplitTableRegion.swap(0, Ordering::SeqCst);
    let _reset = SplitTableRegionReset(previous);
    let (domain, _) = CreateAnalyzeSession().unwrap();
    domain
        .ddl_create_table(
            "test",
            astersql_meta_model::TableInfo {
                Name: ast::NewCIStr("explicit_pre_split"),
                ShardRowIDBits: 2,
                PreSplitRegions: 2,
                ..Default::default()
            },
            false,
        )
        .unwrap();
    let session = ConcreteSession::new(Arc::clone(&domain));

    session
        .pre_split_and_scatter("test", "explicit_pre_split")
        .unwrap();

    assert_eq!(
        Some(&4),
        RUNTIME_REGION_COUNTS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&(
                runtime_domain_id(&domain),
                "test".to_owned(),
                "explicit_pre_split".to_owned(),
                None,
            ))
    );
}

#[test]
fn explicit_split_policy_overrides_disabled_implicit_table_split() {
    struct SplitTableRegionReset(u32);

    impl Drop for SplitTableRegionReset {
        fn drop(&mut self) {
            astersql_ddl::EnableSplitTableRegion.store(self.0, Ordering::SeqCst);
        }
    }

    let previous = astersql_ddl::EnableSplitTableRegion.swap(0, Ordering::SeqCst);
    let _reset = SplitTableRegionReset(previous);
    let (domain, _) = CreateAnalyzeSession().unwrap();
    domain
        .ddl_create_table(
            "test",
            astersql_meta_model::TableInfo {
                Name: ast::NewCIStr("explicit_policy"),
                TableSplitPolicy: Some(astersql_meta_model::RegionSplitPolicy {
                    Lower: vec!["0".to_owned()],
                    Upper: vec!["10000".to_owned()],
                    Regions: 4,
                    ..Default::default()
                }),
                ..Default::default()
            },
            false,
        )
        .unwrap();
    let session = ConcreteSession::new(Arc::clone(&domain));

    session
        .pre_split_and_scatter("test", "explicit_policy")
        .unwrap();

    assert_eq!(
        Some(&4),
        RUNTIME_REGION_COUNTS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&(
                runtime_domain_id(&domain),
                "test".to_owned(),
                "explicit_policy".to_owned(),
                None,
            ))
    );
}

#[test]
fn explicit_index_split_policy_overrides_disabled_implicit_table_split() {
    let table = astersql_meta_model::TableInfo {
        Indices: vec![astersql_meta_model::IndexInfo {
            RegionSplitPolicy: Some(astersql_meta_model::RegionSplitPolicy {
                Lower: vec!["0".to_owned()],
                Upper: vec!["10000".to_owned()],
                Regions: 4,
                ..Default::default()
            }),
            ..Default::default()
        }],
        ..Default::default()
    };

    assert!(has_explicit_region_split_config(&table));
    assert!(should_pre_split_after_create(
        astersql_domain::domain::StartMode::Normal,
        false,
        &table,
    ));
}

#[test]
fn restore_mode_skips_pre_split_even_with_explicit_configuration() {
    let table = astersql_meta_model::TableInfo {
        ShardRowIDBits: 2,
        PreSplitRegions: 2,
        ..Default::default()
    };

    assert!(!should_pre_split_after_create(
        astersql_domain::domain::StartMode::Restore,
        true,
        &table,
    ));
    assert!(should_pre_split_after_create(
        astersql_domain::domain::StartMode::Normal,
        false,
        &table,
    ));
}
