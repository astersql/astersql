// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
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

//! 远程 `LOAD DATA` 测试套件基础设施，对应 Go `util_test.go`。

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;

use astersql_testkit::mockstore::CreateMockStoreAndDomain;
use astersql_testkit::testkit::TestKit;

const GCS_HOST: &str = "127.0.0.1";
const GCS_ENDPOINT_PATH: &str = "/storage/v1/";

/// Rust 的 `TestKit` 自身持有 Go suite 中单列的 `kv.Storage`，因此无需重复字段。
struct MockGcsSuite {
    address: SocketAddr,
    endpoint: String,
    stop: Arc<AtomicBool>,
    server_thread: Option<thread::JoinHandle<()>>,
    testkit: TestKit,
}

impl MockGcsSuite {
    /// 对应 Go `SetupSuite`：启动仅绑定 loopback 的假 GCS，并创建 mock store/session。
    fn setup() -> Self {
        let listener = TcpListener::bind((GCS_HOST, 0)).expect("bind fake GCS server");
        let address = listener.local_addr().expect("read fake GCS address");
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);
        let server_thread = thread::spawn(move || {
            for incoming in listener.incoming() {
                let Ok(mut stream) = incoming else { break };
                if thread_stop.load(Ordering::Acquire) {
                    break;
                }
                let mut request = [0_u8; 1024];
                let _ = stream.read(&mut request);
                let _ = stream.write_all(
                    b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                );
            }
        });
        let (store, _domain) = CreateMockStoreAndDomain();

        Self {
            address,
            endpoint: format!("http://{address}{GCS_ENDPOINT_PATH}"),
            stop,
            server_thread: Some(server_thread),
            testkit: TestKit::new(store),
        }
    }

    /// 对应 Go `GetGCSEndpoint`，并保留 fake-gcs-server 要求的尾随 `/`。
    fn gcs_endpoint(&self) -> &str {
        &self.endpoint
    }

    /// 对应 Go `TearDownSuite`。
    fn teardown(&mut self) {
        let Some(server_thread) = self.server_thread.take() else {
            return;
        };
        self.stop.store(true, Ordering::Release);
        let _ = TcpStream::connect(self.address);
        server_thread.join().expect("join fake GCS server");
    }
}

impl Drop for MockGcsSuite {
    fn drop(&mut self) {
        self.teardown();
    }
}

#[test]
fn test_load_remote_suite_lifecycle() {
    let mut suite = MockGcsSuite::setup();

    assert!(suite.gcs_endpoint().starts_with("http://127.0.0.1:"));
    assert!(suite.gcs_endpoint().ends_with("/storage/v1/"));
    suite
        .testkit
        .MustExec("CREATE DATABASE load_remote_suite", Vec::new());

    let address = suite.address;
    assert!(TcpStream::connect(address).is_ok());
    suite.teardown();
    assert!(TcpStream::connect(address).is_err());
}
