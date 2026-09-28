// Copyright 2026 AsterSQL.

use std::cell::Cell;
use std::collections::HashSet;

use crate::plan_replayer::{
    PlanReplayerBackend, PlanReplayerCaptureInfo, PlanReplayerDumpInfo, PlanReplayerExec,
    PlanReplayerLoadExec, PlanReplayerLoadInfo,
};

#[derive(Default)]
struct Backend {
    parsed: Vec<String>,
    resets: Cell<usize>,
}

impl PlanReplayerBackend for Backend {
    type Context = ();
    type Request = Vec<String>;
    type Statement = String;
    type File = ();
    type Archive = ();
    type Error = String;

    fn grow_and_reset(&self, request: &mut Self::Request) {
        request.clear();
        self.resets.set(self.resets.get() + 1);
    }

    fn append_string(&self, request: &mut Self::Request, _column: usize, value: &str) {
        request.push(value.to_owned());
    }

    fn presigned_url_expiration(&self) -> String {
        String::new()
    }

    fn remove_capture_task(
        &mut self,
        _context: &mut Self::Context,
        _capture: &PlanReplayerCaptureInfo,
    ) -> Result<(), Self::Error> {
        Ok(())
    }

    fn register_capture_task(
        &mut self,
        _context: &mut Self::Context,
        _capture: &PlanReplayerCaptureInfo,
    ) -> Result<(), Self::Error> {
        Ok(())
    }

    fn create_dump_file(
        &mut self,
        _context: &mut Self::Context,
    ) -> Result<(Self::File, String), Self::Error> {
        Ok(((), String::new()))
    }

    fn close_dump_file(&mut self, _file: Self::File) {}

    fn statement_read_timestamp(
        &mut self,
        _context: &mut Self::Context,
    ) -> Result<u64, Self::Error> {
        Ok(0)
    }

    fn prepare_dump_file_transfer(
        &mut self,
        _dump: &PlanReplayerDumpInfo<Self::Statement, Self::File>,
    ) -> Result<(), Self::Error> {
        Ok(())
    }

    fn dump(
        &mut self,
        _context: &mut Self::Context,
        _dump: &mut PlanReplayerDumpInfo<Self::Statement, Self::File>,
    ) -> Result<String, Self::Error> {
        Ok(String::new())
    }

    fn empty_sql_error(&self) -> Self::Error {
        "empty SQL".to_owned()
    }

    fn parse_sql(
        &mut self,
        _context: &mut Self::Context,
        sql: &str,
    ) -> Result<Self::Statement, Self::Error> {
        self.parsed.push(sql.to_owned());
        Ok(sql.to_owned())
    }

    fn prepare_load_file_transfer(
        &mut self,
        _load: &PlanReplayerLoadInfo,
    ) -> Result<(), Self::Error> {
        Ok(())
    }

    fn empty_path_error(&self) -> Self::Error {
        "empty path".to_owned()
    }

    fn read_file(
        &mut self,
        _context: &mut Self::Context,
        _path: &str,
    ) -> Result<Vec<u8>, Self::Error> {
        Ok(Vec::new())
    }

    fn open_archive(&mut self, _data: &[u8]) -> Result<Self::Archive, Self::Error> {
        Ok(())
    }

    fn target_sql(&mut self, _archive: &mut Self::Archive) -> Result<String, Self::Error> {
        Ok(String::new())
    }

    fn load_variables(
        &mut self,
        _context: &mut Self::Context,
        _archive: &mut Self::Archive,
    ) -> Result<(), Self::Error> {
        Ok(())
    }

    fn disable_auto_analyze(&mut self, _context: &mut Self::Context) -> Result<(), Self::Error> {
        Ok(())
    }

    fn create_tables(
        &mut self,
        _context: &mut Self::Context,
        _archive: &mut Self::Archive,
    ) -> Result<HashSet<String>, Self::Error> {
        Ok(HashSet::new())
    }

    fn load_tiflash_replicas(
        &mut self,
        _context: &mut Self::Context,
        _archive: &mut Self::Archive,
    ) -> Result<(), Self::Error> {
        Ok(())
    }

    fn create_views(
        &mut self,
        _context: &mut Self::Context,
        _archive: &mut Self::Archive,
    ) -> Result<(), Self::Error> {
        Ok(())
    }

    fn load_statistics(
        &mut self,
        _context: &mut Self::Context,
        _archive: &mut Self::Archive,
    ) -> Result<(), Self::Error> {
        Ok(())
    }

    fn load_bindings(
        &mut self,
        _context: &mut Self::Context,
        _archive: &mut Self::Archive,
        _databases: &HashSet<String>,
    ) -> Result<(), Self::Error> {
        Ok(())
    }

    fn append_binding_warning(&mut self, _error: &Self::Error) {}

    fn append_auto_analyze_warning(&mut self) {}

    fn load_stats_bytes(
        &mut self,
        _context: &mut Self::Context,
        _data: &[u8],
    ) -> Result<(), Self::Error> {
        Ok(())
    }
}

fn dump_info() -> PlanReplayerDumpInfo<String, ()> {
    PlanReplayerDumpInfo {
        statements: Vec::new(),
        analyze: false,
        historical_stats_timestamp: 0,
        start_timestamp: 0,
        path: String::new(),
        file: None,
        file_name: String::new(),
    }
}

#[test]
fn dump_sql_file_only_trims_newlines_like_go() {
    let mut exec = PlanReplayerExec {
        backend: Backend::default(),
        capture_info: None,
        dump_info: Some(dump_info()),
        end: false,
    };

    exec.DumpSQLsFromFile(&mut (), b"  select 1  ;\n").unwrap();

    assert_eq!(exec.backend.parsed, ["  select 1  "]);
}

#[test]
fn load_next_resets_the_result_chunk_before_validation() {
    let mut exec = PlanReplayerLoadExec {
        backend: Backend::default(),
        info: PlanReplayerLoadInfo {
            path: "capture.zip".to_owned(),
        },
    };
    let mut request = vec!["stale row".to_owned()];

    exec.Next(&mut (), &mut request).unwrap();

    assert!(request.is_empty());
    assert_eq!(exec.backend.resets.get(), 1);
}

#[test]
fn capture_helpers_mark_the_executor_complete_after_success() {
    let capture_info = PlanReplayerCaptureInfo {
        sql_digest: "sql".to_owned(),
        plan_digest: "plan".to_owned(),
        remove: false,
    };
    let mut register = PlanReplayerExec {
        backend: Backend::default(),
        capture_info: Some(capture_info.clone()),
        dump_info: None,
        end: false,
    };
    register.registerCaptureTask(&mut ()).unwrap();
    assert!(register.end);

    let mut remove = PlanReplayerExec {
        backend: Backend::default(),
        capture_info: Some(capture_info),
        dump_info: None,
        end: false,
    };
    remove.removeCaptureTask(&mut ()).unwrap();
    assert!(remove.end);
}
