// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
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

// 访问统计单元测试：覆盖 HTTP 方法归类与 nil 请求分支。
//
// 对应 Go `TestRequestsRecording`：验证 GET/HEAD→get、PUT/POST→put，以及 DELETE
// 当前不计入计数的行为。

use super::AccessStats;
use http::{Method, Request};
use std::sync::atomic::Ordering;

/// 对应 `TestRequestsRecording`，覆盖 nil、GET、HEAD、PUT、POST 和 DELETE。
// test_requests_recording 对应 TestRequestsRecording，覆盖 nil、GET、HEAD、PUT、POST 和 DELETE。
#[test]
pub fn test_requests_recording() {
    let stats = AccessStats::default();

    let check_val_fn = |get: u64, put: u64| {
        assert_eq!(get, stats.requests.get.load(Ordering::Relaxed));
        assert_eq!(put, stats.requests.put.load(Ordering::Relaxed));
    };

    // nil 请求不应改变任何计数，保留 Go rec(nil) 的防御性分支。
    AccessStats::rec_request(Some(&stats), None::<&Request<()>>);
    check_val_fn(0, 0);

    // GET 和 HEAD 都计入 Get；PUT 和 POST 都计入 Put。
    let request = Request::builder().method(Method::GET).body(()).unwrap();
    AccessStats::rec_request(Some(&stats), Some(&request));
    check_val_fn(1, 0);
    let request = Request::builder().method(Method::HEAD).body(()).unwrap();
    AccessStats::rec_request(Some(&stats), Some(&request));
    check_val_fn(2, 0);
    let request = Request::builder().method(Method::PUT).body(()).unwrap();
    AccessStats::rec_request(Some(&stats), Some(&request));
    check_val_fn(2, 1);
    let request = Request::builder().method(Method::POST).body(()).unwrap();
    AccessStats::rec_request(Some(&stats), Some(&request));
    check_val_fn(2, 2);

    // not recorded now：DELETE 当前不记录，保持 Go 测试中的显式注释和期望。
    let request = Request::builder().method(Method::DELETE).body(()).unwrap();
    AccessStats::rec_request(Some(&stats), Some(&request));
    check_val_fn(2, 2);
}
