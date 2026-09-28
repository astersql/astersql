// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

// Traffic 表单编码单元测试。
//
// 校验 `getForm`：按 key 排序并用 Go `url.QueryEscape` 等价规则转义
//（空格为 `+`，其余非安全字符为 `%XX`）。

use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt;
use std::rc::Rc;

use crate::traffic::{
    HttpMethod, HttpResponse, TiProxyNode, TrafficBackend, TrafficCancelExec, TrafficCaptureExec,
    TrafficChunk, TrafficJob, TrafficReplayExec, TrafficShowExec, TrafficStorageHandle,
    capturePath, formReader4Capture, formReader4Replay, getForm, inputKey, outputKey, request,
};

#[derive(Clone, Debug, Eq, PartialEq)]
struct TestError(String);

impl fmt::Display for TestError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

#[derive(Clone, Debug)]
struct TestUrl(String);

#[derive(Clone, Debug)]
struct TestStorageBackend {
    local: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct RecordedRequest {
    method: HttpMethod,
    url: String,
    body: Option<String>,
    content_type: Option<String>,
}

#[derive(Default)]
struct BackendState {
    now: String,
    max_chunk_size: usize,
    nodes: Vec<TiProxyNode>,
    responses: HashMap<String, Result<HttpResponse, TestError>>,
    requests: Vec<RecordedRequest>,
    request_failures: Vec<(String, String, String)>,
    request_successes: Vec<(String, Vec<String>)>,
    storage_objects: Vec<String>,
    storage_owned: bool,
    storage_closed: usize,
    walk_error: Option<TestError>,
    privileges: (bool, bool),
    warnings: Vec<String>,
    path_mismatches: Vec<(bool, usize, usize)>,
    decoded_jobs: HashMap<String, Result<Vec<TrafficJob>, TestError>>,
    decode_errors: Vec<(String, String)>,
    statement_errors: Vec<String>,
    time_errors: Vec<String>,
    opened: usize,
    finished_timeouts: usize,
}

#[derive(Clone, Default)]
struct MockBackend(Rc<RefCell<BackendState>>);

impl MockBackend {
    fn with_nodes(count: usize) -> Self {
        let backend = Self::default();
        {
            let mut state = backend.0.borrow_mut();
            state.now = "2026-01-02T03:04:05Z".to_owned();
            state.max_chunk_size = 2;
            state.privileges = (true, true);
            state.nodes = (0..count)
                .map(|index| TiProxyNode {
                    ip: "127.0.0.1".to_owned(),
                    status_port: (4000 + index).to_string(),
                })
                .collect();
        }
        backend
    }

    fn add_response(&self, address: &str, path: &str, response: Result<HttpResponse, TestError>) {
        self.0
            .borrow_mut()
            .responses
            .insert(format!("http://{address}{path}"), response);
    }
}

impl TrafficBackend for MockBackend {
    type Context = ();
    type Error = TestError;
    type Time = String;
    type TimeoutContext = ();
    type Url = TestUrl;
    type StorageBackend = TestStorageBackend;
    type Storage = Vec<String>;

    fn error(&self, message: String) -> Self::Error {
        TestError(message)
    }

    fn now_rfc3339(&self) -> String {
        self.0.borrow().now.clone()
    }

    fn max_chunk_size(&self) -> usize {
        self.0.borrow().max_chunk_size
    }

    fn open_base(&mut self, _context: &Self::Context) -> Result<(), Self::Error> {
        self.0.borrow_mut().opened += 1;
        Ok(())
    }

    fn tiproxy_nodes(&self, _context: &Self::Context) -> Result<Vec<TiProxyNode>, Self::Error> {
        Ok(self.0.borrow().nodes.clone())
    }

    fn join_host_port(&self, host: &str, port: &str) -> String {
        format!("{host}:{port}")
    }

    fn internal_http_schema(&self) -> &str {
        "http"
    }

    fn http_request(
        &self,
        method: HttpMethod,
        url: &str,
        body: Option<&str>,
        content_type: Option<&str>,
    ) -> Result<HttpResponse, Self::Error> {
        let mut state = self.0.borrow_mut();
        state.requests.push(RecordedRequest {
            method,
            url: url.to_owned(),
            body: body.map(str::to_owned),
            content_type: content_type.map(str::to_owned),
        });
        state
            .responses
            .get(url)
            .cloned()
            .unwrap_or_else(|| Err(TestError(format!("no response for {url}"))))
    }

    fn log_request_failure(
        &self,
        _context: &Self::Context,
        path: &str,
        address: &str,
        response: &str,
        _error: &Self::Error,
    ) {
        self.0.borrow_mut().request_failures.push((
            path.to_owned(),
            address.to_owned(),
            response.to_owned(),
        ));
    }

    fn log_request_success(&self, _context: &Self::Context, path: &str, addresses: &[String]) {
        self.0
            .borrow_mut()
            .request_successes
            .push((path.to_owned(), addresses.to_vec()));
    }

    fn parse_url(&self, value: &str) -> Result<Self::Url, Self::Error> {
        if value.contains(' ') {
            Err(TestError("invalid URL".to_owned()))
        } else {
            Ok(TestUrl(value.to_owned()))
        }
    }

    fn url_is_local(&self, url: &Self::Url) -> bool {
        url.0.starts_with('/') || url.0.starts_with("file:")
    }

    fn join_url_path(&self, url: &Self::Url, path: &str) -> String {
        let (base, query) = url.0.split_once('?').unwrap_or((&url.0, ""));
        let joined = format!("{}/{path}", base.trim_end_matches('/'));
        if query.is_empty() {
            joined
        } else {
            format!("{joined}?{query}")
        }
    }

    fn parse_storage_backend(&self, value: &str) -> Result<Self::StorageBackend, Self::Error> {
        if value.contains(' ') {
            Err(TestError("invalid backend".to_owned()))
        } else {
            Ok(TestStorageBackend {
                local: value.starts_with('/') || value.starts_with("file:"),
            })
        }
    }

    fn storage_backend_is_local(&self, backend: &Self::StorageBackend) -> bool {
        backend.local
    }

    fn timeout_context(&self, _context: &Self::Context, _timeout: std::time::Duration) {}

    fn finish_timeout_context(&self, _context: Self::TimeoutContext) {
        self.0.borrow_mut().finished_timeouts += 1;
    }

    fn traffic_storage(
        &self,
        _context: &Self::TimeoutContext,
        _backend: Self::StorageBackend,
    ) -> Result<TrafficStorageHandle<Self::Storage>, Self::Error> {
        let state = self.0.borrow();
        Ok(TrafficStorageHandle {
            storage: state.storage_objects.clone(),
            owned: state.storage_owned,
        })
    }

    fn walk_storage(
        &self,
        _context: &Self::TimeoutContext,
        storage: &mut Self::Storage,
        object_prefix: &str,
    ) -> Result<Vec<String>, Self::Error> {
        if let Some(error) = self.0.borrow().walk_error.clone() {
            return Err(error);
        }
        Ok(storage
            .iter()
            .filter(|name| name.starts_with(object_prefix))
            .cloned()
            .collect())
    }

    fn close_storage(&self, _storage: Self::Storage) {
        self.0.borrow_mut().storage_closed += 1;
    }

    fn parse_raw_url(&self, value: &str) -> Result<Self::Url, Self::Error> {
        self.parse_url(value)
    }

    fn traffic_privileges(&self) -> (bool, bool) {
        self.0.borrow().privileges
    }

    fn append_warning(&mut self, error: &Self::Error) {
        self.0.borrow_mut().warnings.push(error.0.clone());
    }

    fn log_replay_path_mismatch(
        &self,
        _context: &Self::Context,
        too_many_paths: bool,
        proxies: usize,
        paths: usize,
    ) {
        self.0
            .borrow_mut()
            .path_mismatches
            .push((too_many_paths, proxies, paths));
    }

    fn decode_jobs(&self, response: &str) -> Result<Vec<TrafficJob>, Self::Error> {
        self.0
            .borrow()
            .decoded_jobs
            .get(response)
            .cloned()
            .unwrap_or_else(|| Err(TestError("invalid jobs".to_owned())))
    }

    fn log_job_decode_error(
        &self,
        _context: &Self::Context,
        address: &str,
        response: &str,
        _error: &Self::Error,
    ) {
        self.0
            .borrow_mut()
            .decode_errors
            .push((address.to_owned(), response.to_owned()));
    }

    fn parse_rfc3339(&self, value: &str) -> Result<Self::Time, Self::Error> {
        if value.ends_with('Z') {
            Ok(value.to_owned())
        } else {
            Err(TestError(format!("invalid time: {value}")))
        }
    }

    fn zero_time(&self) -> Self::Time {
        "ZERO".to_owned()
    }

    fn append_statement_error(&mut self, error: &Self::Error) {
        self.0.borrow_mut().statement_errors.push(error.0.clone());
    }

    fn log_time_parse_error(&self, _context: &Self::Context, value: &str, _error: &Self::Error) {
        self.0.borrow_mut().time_errors.push(value.to_owned());
    }
}

#[derive(Default)]
struct TestChunk {
    capacity: usize,
    columns: [Vec<Option<String>>; 8],
}

impl TrafficChunk for TestChunk {
    type Time = String;

    fn grow_and_reset(&mut self, capacity: usize) {
        self.capacity = capacity;
        for column in &mut self.columns {
            column.clear();
        }
    }

    fn append_time(&mut self, column: usize, time: &Self::Time) {
        self.columns[column].push(Some(time.clone()));
    }

    fn append_null(&mut self, column: usize) {
        self.columns[column].push(None);
    }

    fn append_string(&mut self, column: usize, value: &str) {
        self.columns[column].push(Some(value.to_owned()));
    }
}

#[test]
/// 表单字段排序且按 Go 查询串规则转义。
fn traffic_form_is_sorted_and_uses_go_query_escaping() {
    // 含空格、斜杠与冒号的键值，验证排序与百分号编码。
    let arguments = HashMap::from([
        ("z key".to_owned(), "a/b".to_owned()),
        ("output".to_owned(), "s3://bucket/prefix".to_owned()),
        ("duration".to_owned(), "10m".to_owned()),
    ]);
    assert_eq!(
        getForm(&arguments),
        "duration=10m&output=s3%3A%2F%2Fbucket%2Fprefix&z+key=a%2Fb"
    );
}

#[test]
fn capture_forms_match_go_local_remote_and_error_paths() {
    let backend = MockBackend::with_nodes(3);
    let local = HashMap::from([(outputKey.to_owned(), "/tmp/traffic".to_owned())]);
    assert_eq!(
        formReader4Capture(&backend, &local, 3).unwrap(),
        vec!["output=%2Ftmp%2Ftraffic"; 3]
    );

    let remote = HashMap::from([(outputKey.to_owned(), "s3://bucket/tmp?secret=x".to_owned())]);
    let forms = formReader4Capture(&backend, &remote, 2).unwrap();
    assert_eq!(forms.len(), 2);
    assert!(forms[0].contains("tiproxy-0%3Fsecret%3Dx"));
    assert!(forms[1].contains("tiproxy-1%3Fsecret%3Dx"));

    assert_eq!(
        formReader4Capture(&backend, &HashMap::new(), 1)
            .unwrap_err()
            .0,
        "the output path for capture must be specified"
    );
    let invalid = HashMap::from([(outputKey.to_owned(), "bad path".to_owned())]);
    assert_eq!(
        formReader4Capture(&backend, &invalid, 1).unwrap_err().0,
        "parse output path failed: invalid URL"
    );
}

#[test]
fn replay_forms_match_go_storage_walk_and_ownership_contract() {
    let backend = MockBackend::with_nodes(3);
    let local = HashMap::from([(inputKey.to_owned(), "/tmp/traffic".to_owned())]);
    assert_eq!(
        formReader4Replay(&backend, &(), &local, 3).unwrap(),
        vec!["input=%2Ftmp%2Ftraffic"; 3]
    );

    {
        let mut state = backend.0.borrow_mut();
        state.storage_objects = vec![
            "tiproxy-1/meta".to_owned(),
            "tiproxy-0/log".to_owned(),
            "tiproxy-0/meta".to_owned(),
            "unrelated/path".to_owned(),
        ];
        state.storage_owned = true;
    }
    let remote = HashMap::from([(inputKey.to_owned(), "s3://bucket/tmp?secret=x".to_owned())]);
    let mut forms = formReader4Replay(&backend, &(), &remote, 3).unwrap();
    forms.sort();
    assert_eq!(forms.len(), 2);
    assert!(forms[0].contains("tiproxy-0%3Fsecret%3Dx"));
    assert!(forms[1].contains("tiproxy-1%3Fsecret%3Dx"));
    assert_eq!(backend.0.borrow().storage_closed, 1);

    backend.0.borrow_mut().storage_objects.clear();
    assert_eq!(
        formReader4Replay(&backend, &(), &remote, 3).unwrap_err().0,
        "no replay files found in the input path"
    );
    assert_eq!(backend.0.borrow().storage_closed, 2);

    backend.0.borrow_mut().storage_owned = false;
    let missing = HashMap::new();
    assert_eq!(
        formReader4Replay(&backend, &(), &missing, 1).unwrap_err().0,
        "the input path for replay must be specified"
    );
}

#[test]
fn request_preserves_partial_responses_and_stops_at_first_error() {
    let backend = MockBackend::with_nodes(3);
    backend.add_response(
        "127.0.0.1:4000",
        capturePath,
        Ok(HttpResponse {
            status_code: 200,
            body: b"first".to_vec(),
        }),
    );
    backend.add_response(
        "127.0.0.1:4001",
        capturePath,
        Ok(HttpResponse {
            status_code: 500,
            body: b"mock error".to_vec(),
        }),
    );
    let addresses = vec![
        "127.0.0.1:4000".to_owned(),
        "127.0.0.1:4001".to_owned(),
        "127.0.0.1:4002".to_owned(),
    ];
    let readers = vec!["a=1".to_owned(), "a=2".to_owned()];
    let failure = request(
        &backend,
        &(),
        &addresses,
        Some(&readers),
        HttpMethod::Post,
        capturePath,
    )
    .unwrap_err();
    assert_eq!(
        failure.responses.get("127.0.0.1:4000"),
        Some(&"first".to_owned())
    );
    assert_eq!(
        failure.error.0,
        "request to tiproxy '127.0.0.1:4001' failed: mock error"
    );
    let state = backend.0.borrow();
    assert_eq!(state.requests.len(), 2);
    assert_eq!(
        state.requests[0].content_type.as_deref(),
        Some("application/x-www-form-urlencoded")
    );
    assert_eq!(state.requests[1].body.as_deref(), Some("a=2"));
    assert_eq!(state.request_failures.len(), 1);
    assert!(state.request_successes.is_empty());
}

#[test]
fn capture_replay_and_cancel_executors_match_go_forms_warnings_and_privileges() {
    let backend = MockBackend::with_nodes(2);
    for address in ["127.0.0.1:4000", "127.0.0.1:4001"] {
        for path in [
            capturePath,
            crate::traffic::replayPath,
            crate::traffic::cancelPath,
        ] {
            backend.add_response(
                address,
                path,
                Ok(HttpResponse {
                    status_code: 200,
                    body: Vec::new(),
                }),
            );
        }
    }

    let mut capture = TrafficCaptureExec {
        BaseExecutor: backend.clone(),
        Args: HashMap::from([(outputKey.to_owned(), "/tmp".to_owned())]),
    };
    capture.Next(&(), &mut ()).unwrap();
    assert_eq!(
        capture.Args.get("start-time").unwrap(),
        "2026-01-02T03:04:05Z"
    );

    backend.0.borrow_mut().storage_objects = vec!["tiproxy-0/meta".to_owned()];
    let mut replay = TrafficReplayExec {
        BaseExecutor: backend.clone(),
        Args: HashMap::from([(inputKey.to_owned(), "s3://bucket/tmp".to_owned())]),
    };
    replay.Next(&(), &mut ()).unwrap();
    let state = backend.0.borrow();
    assert_eq!(state.finished_timeouts, 1);
    assert_eq!(state.warnings.len(), 1);
    assert!(state.warnings[0].contains("greater than input paths number (1)"));
    assert_eq!(state.path_mismatches, vec![(false, 2, 1)]);
    drop(state);

    backend.0.borrow_mut().privileges = (true, false);
    let mut cancel = TrafficCancelExec {
        BaseExecutor: backend.clone(),
    };
    cancel.Next(&(), &mut ()).unwrap();
    let state = backend.0.borrow();
    let cancel_requests: Vec<_> = state
        .requests
        .iter()
        .filter(|request| request.url.ends_with(crate::traffic::cancelPath))
        .collect();
    assert_eq!(cancel_requests.len(), 2);
    assert!(
        cancel_requests
            .iter()
            .all(|request| request.body.as_deref() == Some("type=capture"))
    );
}

#[test]
fn replay_rejects_more_paths_than_proxies_before_http_requests() {
    let backend = MockBackend::with_nodes(1);
    backend.0.borrow_mut().storage_objects =
        vec!["tiproxy-0/meta".to_owned(), "tiproxy-1/meta".to_owned()];
    let mut replay = TrafficReplayExec {
        BaseExecutor: backend.clone(),
        Args: HashMap::from([(inputKey.to_owned(), "s3://bucket/tmp".to_owned())]),
    };
    let error = replay.Next(&(), &mut ()).unwrap_err();
    assert_eq!(
        error.0,
        "tiproxy instances number (1) is less than input paths number (2)"
    );
    let state = backend.0.borrow();
    assert_eq!(state.path_mismatches, vec![(true, 1, 2)]);
    assert!(state.requests.is_empty());
}

#[test]
fn show_filters_sorts_pages_and_reports_time_errors_like_go() {
    let backend = MockBackend::with_nodes(2);
    backend.add_response(
        "127.0.0.1:4000",
        crate::traffic::showPath,
        Ok(HttpResponse {
            status_code: 200,
            body: b"jobs-a".to_vec(),
        }),
    );
    backend.add_response(
        "127.0.0.1:4001",
        crate::traffic::showPath,
        Ok(HttpResponse {
            status_code: 200,
            body: b"jobs-b".to_vec(),
        }),
    );
    {
        let mut state = backend.0.borrow_mut();
        state.privileges = (true, false);
        state.decoded_jobs.insert(
            "jobs-a".to_owned(),
            Ok(vec![
                TrafficJob {
                    job_type: "capture".to_owned(),
                    start_time: "2020-01-01T00:00:00Z".to_owned(),
                    end_time: "bad-time".to_owned(),
                    output: "/tmp".to_owned(),
                    duration: "1m".to_owned(),
                    compress: true,
                    ..TrafficJob::default()
                },
                TrafficJob {
                    job_type: "replay".to_owned(),
                    start_time: "2020-01-01T03:00:00Z".to_owned(),
                    ..TrafficJob::default()
                },
            ]),
        );
        state.decoded_jobs.insert(
            "jobs-b".to_owned(),
            Ok(vec![TrafficJob {
                job_type: "capture".to_owned(),
                start_time: "2020-01-01T01:00:00Z".to_owned(),
                ..TrafficJob::default()
            }]),
        );
    }

    let mut show = TrafficShowExec {
        BaseExecutor: backend.clone(),
        jobs: Vec::new(),
        cursor: 0,
    };
    show.Open(&()).unwrap();
    assert_eq!(backend.0.borrow().opened, 1);
    assert_eq!(show.jobs.len(), 2);
    assert_eq!(show.jobs[0].instance, "127.0.0.1:4001");

    let mut chunk = TestChunk::default();
    show.Next(&(), &mut chunk).unwrap();
    assert_eq!(chunk.capacity, 2);
    assert_eq!(chunk.columns[1], vec![None, Some("ZERO".to_owned())]);
    assert_eq!(chunk.columns[3], vec![Some("capture".to_owned()); 2]);
    assert_eq!(
        chunk.columns[7][1].as_deref(),
        Some("OUTPUT=\"/tmp\", DURATION=\"1m\", COMPRESS=true, ENCRYPTION_METHOD=\"\"")
    );
    let state = backend.0.borrow();
    assert_eq!(state.statement_errors.len(), 1);
    assert_eq!(state.time_errors, vec!["bad-time"]);
}
