// Copyright 2026 AsterSQL.

use super::*;
use std::sync::atomic::{AtomicU64, Ordering};

struct GroupRuntime {
    jobs: Vec<ImportJobInfo>,
    import_runtime: Option<ImportRuntimeInfo>,
    limit: AtomicU64,
}

impl ShowRuntimeContext for GroupRuntime {
    fn MaxChunkSize(&self) -> usize {
        32
    }
    fn SelectLimit(&self) -> u64 {
        self.limit.load(Ordering::Relaxed)
    }
    fn SetSelectLimit(&self, value: u64) {
        self.limit.store(value, Ordering::Relaxed);
    }
    fn FetchRows(&self, _: ShowOperation, _: &ShowRequest) -> ShowResult<Vec<Vec<ShowValue>>> {
        unreachable!()
    }
    fn AllSchemaNames(&self) -> ShowResult<Vec<String>> {
        unreachable!()
    }
    fn DatabaseVisible(&self, _: &str) -> ShowResult<bool> {
        unreachable!()
    }
    fn SchemaExists(&self, _: &str) -> ShowResult<bool> {
        unreachable!()
    }
    fn Tables(&self, _: &str, _: bool) -> ShowResult<Vec<TableInfo>> {
        unreachable!()
    }
    fn TableByName(&self, _: &str, _: &str) -> ShowResult<Option<TableInfo>> {
        unreachable!()
    }
    fn TableVisible(&self, _: &str, _: &str) -> ShowResult<bool> {
        unreachable!()
    }
    fn TableRegions(&self, _: &TableName, _: &[i64]) -> ShowResult<Vec<regionMeta>> {
        unreachable!()
    }
    fn IndexRegions(&self, _: &TableName, _: &str, _: &[i64]) -> ShowResult<Vec<regionMeta>> {
        unreachable!()
    }
    fn SchedulingInfo(
        &self,
        _: &[regionMeta],
        _: &TableName,
    ) -> ShowResult<Vec<showTableRegionRowItem>> {
        unreachable!()
    }
    fn ImportJobs(&self, _: &ShowRequest) -> ShowResult<Vec<ImportJobInfo>> {
        Ok(self.jobs.clone())
    }
    fn ImportRuntime(&self, _: i64) -> ShowResult<Option<ImportRuntimeInfo>> {
        Ok(self.import_runtime.clone())
    }
    fn DistributionJobs(&self, _: &ShowRequest) -> ShowResult<Vec<DistributionJob>> {
        unreachable!()
    }
    fn RunWithSystemSession(
        &self,
        action: &mut dyn FnMut(&dyn ShowRuntimeContext) -> ShowResult,
    ) -> ShowResult {
        action(self)
    }
    fn FillViewColumnType(&self, _: &str, _: &TableInfo) -> ShowResult {
        unreachable!()
    }
    fn FormatSplitExpression(&self, _: &str) -> ShowResult<String> {
        unreachable!()
    }
}

#[test]
fn import_groups_match_go_empty_and_named_group_queries() {
    let runtime = Arc::new(GroupRuntime {
        jobs: Vec::new(),
        import_runtime: None,
        limit: AtomicU64::new(1),
    });
    let mut show = ShowExec::new(runtime.clone(), ShowStmtType::ImportGroups);
    show.fetchShowImportGroups().unwrap();
    assert_eq!(show.result.as_ref().map_or(0, Vec::len), 0);

    let created = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_000);
    let jobs = [("group1", 1), ("group1", 2), ("group2", 3), ("", 4)]
        .into_iter()
        .map(|(group_key, id)| ImportJobInfo {
            id,
            group_key: group_key.into(),
            status: "running".into(),
            create_time: Some(created),
            ..Default::default()
        })
        .collect();
    let runtime = Arc::new(GroupRuntime {
        jobs,
        import_runtime: None,
        limit: AtomicU64::new(1),
    });
    let mut show = ShowExec::new(runtime.clone(), ShowStmtType::ImportGroups);
    show.fetchShowImportGroups().unwrap();
    let rows = show.result.as_ref().unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0][0], ShowValue::String("group1".into()));
    assert_eq!(rows[0][1], ShowValue::Int64(2));
    assert_ne!(rows[0][7], ShowValue::Null);
    assert_eq!(rows[1][0], ShowValue::String("group2".into()));
    assert_eq!(rows[1][1], ShowValue::Int64(1));
    assert_ne!(rows[1][7], ShowValue::Null);

    let mut missing = ShowExec::new(runtime.clone(), ShowStmtType::ImportGroups);
    missing.ImportGroupKey = "nonexist".into();
    missing.fetchShowImportGroups().unwrap();
    assert_eq!(missing.result.as_ref().map_or(0, Vec::len), 0);
    let mut named = ShowExec::new(runtime, ShowStmtType::ImportGroups);
    named.ImportGroupKey = "group2".into();
    named.fetchShowImportGroups().unwrap();
    let rows = named.result.as_ref().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0][0], ShowValue::String("group2".into()));
    assert_eq!(rows[0][1], ShowValue::Int64(1));
    assert_ne!(rows[0][7], ShowValue::Null);
}

#[test]
fn import_job_rows_keep_go_progress_column_order() {
    let cases = [
        ("init", "0B", "0B", "N/A", "0B/s", "N/A", 0),
        ("encode", "500B", "1000B", "50", "100B/s", "00:00:05", 0),
        ("merge-sort", "0B", "0B", "0", "0B/s", "N/A", 0),
        ("ingest", "500B", "1000B", "50", "50B/s", "00:00:10", 50),
        (
            "collect-conflicts",
            "500 conflicts",
            "1000 conflicts",
            "50",
            "50 conflicts/s",
            "00:00:10",
            0,
        ),
        (
            "conflict-resolution",
            "500 conflicts",
            "500 conflicts",
            "100",
            "50 conflicts/s",
            "00:00:00",
            0,
        ),
        ("post-process", "0B", "0B", "N/A", "0B/s", "N/A", 100),
    ];
    for (step, processed, total, percent, speed, eta, imported_rows) in cases {
        let runtime = Arc::new(GroupRuntime {
            jobs: vec![ImportJobInfo {
                id: 11,
                status: "running".into(),
                ..Default::default()
            }],
            import_runtime: Some(ImportRuntimeInfo {
                step: step.into(),
                processed_size: processed.into(),
                total_size: total.into(),
                percent: percent.into(),
                speed: speed.into(),
                eta: eta.into(),
                import_rows: imported_rows,
                ..Default::default()
            }),
            limit: AtomicU64::new(1),
        });
        let mut show = ShowExec::new(runtime, ShowStmtType::ImportJobs);
        show.ImportJobID = Some(11);
        show.fetchShowImportJobs().unwrap();
        let rows = show.result.as_ref().unwrap();
        assert_eq!(rows.len(), 1);
        let row = &rows[0];
        assert_eq!(row[8], ShowValue::Uint64(imported_rows), "step {step}");
        assert_eq!(row[15], ShowValue::String(step.into()));
        assert_eq!(row[16], ShowValue::String(processed.into()));
        assert_eq!(row[17], ShowValue::String(total.into()));
        assert_eq!(row[18], ShowValue::String(percent.into()));
        assert_eq!(row[19], ShowValue::String(speed.into()));
        assert_eq!(row[20], ShowValue::String(eta.into()));
    }
}
