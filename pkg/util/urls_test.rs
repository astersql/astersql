// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

// urls 单元测试：ParseHostPortAddr 合法/非法用例。
//
// 对应 Go `urls_test.go`：覆盖裸 host:port、多地址、http(s)/unix URL 及错误输入。

use crate::urls::ParseHostPortAddr;

#[test]
fn go_merge_40_parse_host_port_uses_service_url_endpoint() {
    assert_eq!(
        ParseHostPortAddr("unix://localhost:m0, unix:///home/tidb/tidb.sock").unwrap(),
        ["unix://localhost:m0", "unix:///home/tidb/tidb.sock"]
    );
    assert!(ParseHostPortAddr("unix://").is_err());
    assert!(ParseHostPortAddr("http://:2379").is_err());
    assert!(ParseHostPortAddr("http://localhost:").is_err());
    assert_eq!(
        ParseHostPortAddr("http://host:2379?x=1").unwrap(),
        ["http://host:2379"]
    );
}

/// 表驱动校验合法地址原样返回，非法地址返回 Err。
#[test]
fn test_parse_host_port_addr() {
    // 合法输入：裸地址、逗号列表、带 scheme 的 URL（含 unix socket）。
    let urls = [
        "127.0.0.1:2379",
        "127.0.0.1:2379,127.0.0.2:2379",
        "localhost:2379",
        "pump-1:8250,pump-2:8250",
        "http://127.0.0.1:2379",
        "https://127.0.0.1:2379",
        "http://127.0.0.1:2379,http://127.0.0.2:2379",
        "https://127.0.0.1:2379,https://127.0.0.2:2379",
        "unix://localhost:m0",
        "unix:///home/tidb/tidb.sock",
        "http://localhost:mysql",
        "http://localhost:65536",
        "http://localhost:2379?redirect=/health#ready",
        ":2379",
        "localhost:",
        ":",
    ];

    let expect_urls: [&[&str]; 16] = [
        &["127.0.0.1:2379"],
        &["127.0.0.1:2379", "127.0.0.2:2379"],
        &["localhost:2379"],
        &["pump-1:8250", "pump-2:8250"],
        &["http://127.0.0.1:2379"],
        &["https://127.0.0.1:2379"],
        &["http://127.0.0.1:2379", "http://127.0.0.2:2379"],
        &["https://127.0.0.1:2379", "https://127.0.0.2:2379"],
        &["unix://localhost:m0"],
        &["unix:///home/tidb/tidb.sock"],
        &["http://localhost:mysql"],
        &["http://localhost:65536"],
        &["http://localhost:2379"],
        &[":2379"],
        &["localhost:"],
        &[":"],
    ];

    for (i, url) in urls.iter().enumerate() {
        let url_list = ParseHostPortAddr(url).expect("valid host:port list");
        assert_eq!(expect_urls[i].len(), url_list.len());
        for (j, u) in url_list.iter().enumerate() {
            assert_eq!(expect_urls[i][j], u);
        }
    }

    // 缺 port、空 host、非法 scheme 均应失败。
    let invalid_urls = [
        "127.0.0.1",
        "http:///127.0.0.1:2379",
        "htt://127.0.0.1:2379",
        "unix://",
        "[::1]:2379:2380",
        "host]:2379",
    ];
    for url in invalid_urls {
        assert!(ParseHostPortAddr(url).is_err(), "expected error for {url}");
    }
}
