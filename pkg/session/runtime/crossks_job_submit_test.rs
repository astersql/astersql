// Copyright 2026 AsterSQL.

use std::sync::Arc;

use astersql_ddl_systable as systable;
use astersql_domain_crossks::{AlterTableModeJob, SessionPool, TableMode};

use super::{
    CreateAnalyzeSession,
    crossks_job_submit::CrossKSJobSubmitter,
    crossks_session_pool::{
        CrossKSFlashbackGuard, CrossKSMinJobId, CrossKSSessionPool, CrossKSSystemTablePool,
    },
};

#[test]
fn go_merge_43_crossks_submits_durable_table_mode_job_with_target_metadata() {
    let (domain, _) = CreateAnalyzeSession().expect("canonical SQL session");
    let pool = CrossKSSessionPool::new(Arc::clone(&domain));
    let table_pool: Arc<dyn systable::SessionPool> =
        Arc::new(CrossKSSystemTablePool::new(Arc::clone(&pool)));
    let manager = systable::new_manager(table_pool);
    let guard = Arc::new(CrossKSFlashbackGuard::new(Arc::clone(&manager)));
    let refresher = Arc::new(systable::new_min_job_id_refresher(Arc::clone(&manager)));
    let min_id = Arc::new(CrossKSMinJobId::new(refresher));
    let submitter = CrossKSJobSubmitter::new(Arc::clone(&pool), guard, min_id, None);
    let mut job = AlterTableModeJob {
        id: 0,
        schema_id: 100,
        table_id: 200,
        schema_name: "testdb".into(),
        table_name: "t1".into(),
        target_mode: TableMode::Import,
        query: "skip".into(),
        cdc_write_source: 0,
        sql_mode: 0,
    };
    submitter
        .submit_table_mode(&mut job)
        .expect("persist DDL job");
    assert!(job.id > 0);
    let stored = manager
        .get_job_by_id(&systable::Context::default(), job.id)
        .expect("decode queued DDL job");
    assert_eq!(stored.job.table_id, 200);
    assert_eq!(stored.job.schema_id, 100);
    let lease = pool.acquire().unwrap();
    let rows = lease
        .query(format!(
            "SELECT schema_ids, table_ids, type, processing FROM mysql.tidb_ddl_job WHERE job_id = {}",
            job.id
        ))
        .unwrap();
    assert_eq!(
        rows,
        vec![vec![
            "100".to_owned(),
            "200".to_owned(),
            "75".to_owned(),
            "0".to_owned()
        ]]
    );
    drop(lease);
    pool.close();
    domain.close();
}
