// Copyright 2026 AsterSQL.

use std::collections::HashMap;
use std::sync::Arc;

use astersql_server_handler_extractorhandler::extractor::{
    ExtractError, ExtractReader, ExtractResult, ExtractRuntime, ExtractTask, RequestContext,
    Timestamp,
};

use crate::extract::ExtractTaskServeHandler;
use crate::http_status::{Method, Request};
use crate::server::Domain;

struct UnusedReader;

impl ExtractReader for UnusedReader {
    fn read(&mut self, _: &mut [u8]) -> ExtractResult<usize> {
        Ok(0)
    }

    fn close(&mut self) -> ExtractResult<()> {
        Ok(())
    }
}

struct FailpointRuntime;

impl ExtractRuntime for FailpointRuntime {
    fn now(&self) -> Timestamp {
        Timestamp(0)
    }

    fn parse_time(&self, _: &str) -> ExtractResult<Timestamp> {
        Err(ExtractError("unused".into()))
    }

    fn extract_task(&self, _: &RequestContext, _: ExtractTask) -> ExtractResult<String> {
        Err(ExtractError("failpoint must bypass extraction".into()))
    }

    fn extract_task_directory(&self) -> String {
        String::new()
    }

    fn open_extract(&self, _: &RequestContext, _: &str) -> ExtractResult<Box<dyn ExtractReader>> {
        Ok(Box::new(UnusedReader))
    }

    fn failpoint_enabled(&self, name: &str) -> bool {
        name == "extractTaskServeHandler"
    }

    fn log_error(&self, _: &str, _: &ExtractError) {}

    fn log_warning(&self, _: &str, _: &ExtractError) {}
}

struct ExtractDomain;

impl Domain for ExtractDomain {
    fn server_id(&self) -> u64 {
        1
    }

    fn start_timestamp(&self) -> i64 {
        0
    }

    fn extract_runtime(&self) -> Option<Arc<dyn ExtractRuntime>> {
        Some(Arc::new(FailpointRuntime))
    }
}

#[test]
fn server_extract_adapter_delegates_to_canonical_handler() {
    let handler = ExtractTaskServeHandler::new(Some(Arc::new(ExtractDomain)));
    let response = handler.handle(&Request {
        method: Method::Get,
        path: "/extract_task/dump".into(),
        query: HashMap::from([("type".into(), "plan".into())]),
        raw_query: "type=plan".into(),
        headers: HashMap::new(),
        body: Vec::new(),
    });

    assert_eq!(response.status, 200);
    assert_eq!(response.body, b"mock");
}
