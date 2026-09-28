// Copyright 2026 AsterSQL.

use std::sync::Mutex;

use crate::{
    ExtractError, ExtractReader, ExtractResult, ExtractRuntime, ExtractTask, HttpResponseWriter,
    RequestContext, Timestamp, streamExtractResponse,
};

struct EmptyReader;

impl ExtractReader for EmptyReader {
    fn read(&mut self, _buffer: &mut [u8]) -> ExtractResult<usize> {
        Ok(0)
    }

    fn close(&mut self) -> ExtractResult<()> {
        Ok(())
    }
}

#[derive(Default)]
struct PathRuntime {
    opened_path: Mutex<Option<String>>,
}

impl ExtractRuntime for PathRuntime {
    fn now(&self) -> Timestamp {
        Timestamp(0)
    }

    fn parse_time(&self, _value: &str) -> ExtractResult<Timestamp> {
        unreachable!("not used by streamExtractResponse")
    }

    fn extract_task(&self, _context: &RequestContext, _task: ExtractTask) -> ExtractResult<String> {
        unreachable!("not used by streamExtractResponse")
    }

    fn extract_task_directory(&self) -> String {
        "/var/lib/astersql/extract".to_owned()
    }

    fn open_extract(
        &self,
        _context: &RequestContext,
        path: &str,
    ) -> ExtractResult<Box<dyn ExtractReader>> {
        *self.opened_path.lock().expect("opened path lock") = Some(path.to_owned());
        Ok(Box::new(EmptyReader))
    }

    fn failpoint_enabled(&self, _name: &str) -> bool {
        false
    }

    fn log_error(&self, _message: &str, _error: &ExtractError) {}

    fn log_warning(&self, _message: &str, _error: &ExtractError) {}
}

#[derive(Default)]
struct SinkWriter;

impl HttpResponseWriter for SinkWriter {
    fn set_header(&mut self, _name: &str, _value: &str) {}

    fn write_status(&mut self, _status: u16) {}

    fn write(&mut self, data: &[u8]) -> ExtractResult<usize> {
        Ok(data.len())
    }

    fn write_error(&mut self, _error: ExtractError) {}
}

#[test]
fn stream_extract_response_cleans_path_like_filepath_join() {
    let runtime = PathRuntime::default();
    streamExtractResponse(
        &RequestContext::default(),
        &mut SinkWriter,
        "nested/../task",
        &runtime,
    )
    .expect("stream response");

    assert_eq!(
        Some("/var/lib/astersql/extract/task"),
        runtime
            .opened_path
            .lock()
            .expect("opened path lock")
            .as_deref()
    );
}
