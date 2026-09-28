// Copyright 2026 AsterSQL.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use crate::rpc_server::{
    BatchCommandStream, BatchRequest, BatchResponse, CoprocessorExecutor, CoprocessorRequest,
    CoprocessorResponse, CoprocessorStream, MppCoordinator, MppTaskStatusRequest,
    MppTaskStatusResponse, RpcServer, RpcSession, RpcSessionFactory,
};
use crate::server::{Domain, StatusConfig};

#[derive(Default)]
struct TestDomain;

impl Domain for TestDomain {
    fn server_id(&self) -> u64 {
        1
    }

    fn start_timestamp(&self) -> i64 {
        2
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum SessionEvent {
    Peer(Option<String>),
    InitializeMemory,
    DetachMemory,
    Close,
}

struct TestSession(Arc<Mutex<Vec<SessionEvent>>>);

impl RpcSession for TestSession {
    fn set_peer_address(&mut self, address: Option<String>) {
        self.0.lock().unwrap().push(SessionEvent::Peer(address));
    }

    fn initialize_memory_tracker(&mut self) {
        self.0.lock().unwrap().push(SessionEvent::InitializeMemory);
    }

    fn detach_memory_tracker(&mut self) {
        self.0.lock().unwrap().push(SessionEvent::DetachMemory);
    }

    fn close(&mut self) {
        self.0.lock().unwrap().push(SessionEvent::Close);
    }
}

struct TestSessionFactory {
    events: Arc<Mutex<Vec<SessionEvent>>>,
    error: Option<String>,
}

impl RpcSessionFactory for TestSessionFactory {
    fn create(&self, _domain: Arc<dyn Domain>) -> Result<Box<dyn RpcSession>, String> {
        match &self.error {
            Some(error) => Err(error.clone()),
            None => Ok(Box::new(TestSession(Arc::clone(&self.events)))),
        }
    }
}

struct TestExecutor;

impl CoprocessorExecutor for TestExecutor {
    fn execute(
        &self,
        _session: &mut dyn RpcSession,
        request: &CoprocessorRequest,
    ) -> Result<CoprocessorResponse, String> {
        Ok(CoprocessorResponse {
            payload: request.payload.clone(),
            other_error: None,
        })
    }
}

struct TestMpp;

impl MppCoordinator for TestMpp {
    fn report_status(&self, _request: &MppTaskStatusRequest) -> MppTaskStatusResponse {
        MppTaskStatusResponse::default()
    }
}

fn server(factory: TestSessionFactory) -> Arc<RpcServer> {
    RpcServer::new(
        &StatusConfig::default(),
        None,
        Arc::new(TestDomain),
        Arc::new(factory),
        Arc::new(TestExecutor),
        Arc::new(TestMpp),
    )
}

#[derive(Default)]
struct RecordingStream(Vec<CoprocessorResponse>);

impl CoprocessorStream for RecordingStream {
    fn send(&mut self, response: CoprocessorResponse) -> Result<(), String> {
        self.0.push(response);
        Ok(())
    }
}

#[test]
fn unary_detaches_memory_tracker_before_closing_session() {
    let events = Arc::new(Mutex::new(Vec::new()));
    let server = server(TestSessionFactory {
        events: Arc::clone(&events),
        error: None,
    });

    let response = server.coprocessor(&CoprocessorRequest {
        peer_address: Some("127.0.0.1:4000".into()),
        ..CoprocessorRequest::default()
    });

    assert_eq!(response.other_error, None);
    assert_eq!(
        *events.lock().unwrap(),
        vec![
            SessionEvent::InitializeMemory,
            SessionEvent::Peer(Some("127.0.0.1:4000".into())),
            SessionEvent::DetachMemory,
            SessionEvent::Close,
        ]
    );
}

#[test]
fn stream_session_creation_error_is_sent_as_other_error() {
    let server = server(TestSessionFactory {
        events: Arc::new(Mutex::new(Vec::new())),
        error: Some("create failed".into()),
    });
    let mut stream = RecordingStream::default();

    assert_eq!(
        server.coprocessor_stream(&CoprocessorRequest::default(), &mut stream),
        Ok(())
    );
    assert_eq!(stream.0.len(), 1);
    assert_eq!(stream.0[0].other_error.as_deref(), Some("create failed"));
}

#[test]
fn stream_closes_session_without_unary_peer_or_detach_side_effects() {
    let events = Arc::new(Mutex::new(Vec::new()));
    let server = server(TestSessionFactory {
        events: Arc::clone(&events),
        error: None,
    });
    let mut stream = RecordingStream::default();

    assert_eq!(
        server.coprocessor_stream(
            &CoprocessorRequest {
                peer_address: Some("127.0.0.1:4000".into()),
                ..CoprocessorRequest::default()
            },
            &mut stream,
        ),
        Ok(())
    );
    assert_eq!(
        *events.lock().unwrap(),
        vec![SessionEvent::InitializeMemory, SessionEvent::Close]
    );
}

struct PanickingBatchStream {
    requests: VecDeque<Result<Option<BatchRequest>, String>>,
}

impl BatchCommandStream for PanickingBatchStream {
    fn receive(&mut self) -> Result<Option<BatchRequest>, String> {
        self.requests
            .pop_front()
            .unwrap_or_else(|| panic!("recv panic"))
    }

    fn send(&mut self, _response: BatchResponse) -> Result<(), String> {
        Ok(())
    }
}

#[test]
fn batch_command_panic_is_recovered_without_transport_error() {
    let server = server(TestSessionFactory {
        events: Arc::new(Mutex::new(Vec::new())),
        error: None,
    });
    let mut stream = PanickingBatchStream {
        requests: VecDeque::new(),
    };

    assert_eq!(server.batch_commands(&mut stream), Ok(()));
}
