// Copyright 2026 AsterSQL.

use std::path::PathBuf;

use crate::optimize_trace::{DownloadFileRequest, NewOptimizeTraceHandler, OptimizeTraceRuntime};

#[derive(Default)]
struct Runtime {
    request: Option<DownloadFileRequest>,
}

impl OptimizeTraceRuntime for Runtime {
    type Error = String;

    fn route_file_name(&self) -> String {
        "/trace.zip".to_owned()
    }

    fn optimizer_trace_directory(&self) -> PathBuf {
        PathBuf::from("/tmp/optimizer")
    }

    fn internal_http_scheme(&self) -> String {
        "https".to_owned()
    }

    fn download_file(&mut self, request: DownloadFileRequest) -> Result<(), Self::Error> {
        self.request = Some(request);
        Ok(())
    }

    fn write_error(&mut self, error: Self::Error) {
        panic!("unexpected download error: {error}");
    }
}

#[test]
fn filepath_join_keeps_directory_for_rooted_route_name() {
    let mut runtime = Runtime::default();

    NewOptimizeTraceHandler("db.internal".to_owned(), 10080).ServeHTTP(&mut runtime);

    let request = runtime.request.expect("download request");
    assert_eq!(request.file_path, PathBuf::from("/tmp/optimizer/trace.zip"));
    assert_eq!(request.file_name, "/trace.zip");
    assert_eq!(request.url_path, "optimize_trace/dump//trace.zip");
}
