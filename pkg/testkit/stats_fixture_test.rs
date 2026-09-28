// Copyright 2026 AsterSQL.

use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

use super::{LoadTableStats, TestKit};

#[test]
fn load_table_stats_populates_real_histograms_and_indexes() {
    let (store, domain) = super::mockstore::CreateMockStoreAndDomain();
    let mut tk = TestKit::new(store);
    tk.MustExec("create database stats_fixture", Vec::new());
    tk.MustExec("use stats_fixture", Vec::new());
    tk.MustExec(
        "create table t (a int not null, b int, primary key (a, b), key idx_b (b))",
        Vec::new(),
    );

    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock after epoch")
        .as_nanos();
    let path = std::env::temp_dir().join(format!(
        "astersql-testkit-stats-{}-{suffix}.json",
        std::process::id()
    ));
    fs::write(
        &path,
        r#"{
          "database_name":"stats_fixture","table_name":"t","count":12,
          "modify_count":2,"version":42,"is_historical_stats":false,
          "columns":{
            "a":{"histogram":{"ndv":7,"buckets":[{"count":12,"repeats":2,"ndv":7,"lower_bound":"AQ==","upper_bound":"Ag=="}]},"stats_ver":2,"null_count":0,"tot_col_size":24,"last_update_version":42,"correlation":1.0},
            "b":{"histogram":null,"stats_ver":2,"null_count":1,"tot_col_size":12,"last_update_version":42,"correlation":0.0}
          },
          "indices":{
            "idx_b":{"histogram":{"ndv":5,"buckets":[]},"stats_ver":2,"null_count":1,"tot_col_size":12,"last_update_version":42,"correlation":0.0},
            "primary":{"histogram":{"ndv":7,"buckets":[]},"stats_ver":2,"null_count":0,"tot_col_size":24,"last_update_version":42,"correlation":0.0}
          }
        }"#,
    )
    .expect("write statistics fixture");

    LoadTableStats(&path, domain.as_ref()).expect("load statistics fixture");
    fs::remove_file(&path).expect("remove statistics fixture");

    let table = domain
        .table_by_name("stats_fixture", "t")
        .expect("stats_fixture.t metadata");
    let stats_handle = domain.stats_handle();
    let handle = stats_handle.lock().expect("statistics handle");
    let stats = handle
        .stats_meta(table.ID)
        .expect("loaded table statistics");
    assert!(!stats.pseudo);
    assert!(stats.initialized);
    assert_eq!(stats.realtime_count, 12);
    assert_eq!(stats.modify_count, 2);
    assert_eq!(stats.columns.len(), 2);
    assert_eq!(stats.indexes.len(), 2);
    let a = table
        .Columns
        .iter()
        .find(|column| column.Name.L == "a")
        .expect("column a");
    let a_stats = stats.columns.get(&a.ID).expect("column a statistics");
    assert_eq!(a_stats.ndv, 7);
    assert_eq!(a_stats.buckets[0].lower, vec![1]);
    assert_eq!(a_stats.buckets[0].upper, vec![2]);
}
