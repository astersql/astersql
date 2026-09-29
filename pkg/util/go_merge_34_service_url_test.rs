// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
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

use crate::service_url::{NormalizeServiceURL, ParseServiceURL};

#[test]
fn go_merge_34_service_url_parsing_and_normalization() {
    for (raw, scheme, address, unix) in [
        ("http://127.0.0.1:2379", "http://", "127.0.0.1:2379", false),
        (
            "https://127.0.0.1:2379",
            "https://",
            "127.0.0.1:2379",
            false,
        ),
        ("unix://localhost:m0", "unix://", "localhost:m0", true),
        ("unix:///tmp/etcd.sock", "unix://", "/tmp/etcd.sock", true),
    ] {
        let parsed = ParseServiceURL(raw).unwrap();
        assert_eq!(parsed.SchemePrefix(), scheme);
        assert_eq!(parsed.Address(), address);
        assert_eq!(parsed.IsUnixFamily(), unix);
        assert_eq!(parsed.Endpoint(false), if unix { raw } else { address });
        assert_eq!(parsed.Endpoint(true), raw);
        assert_eq!(parsed.to_string(), raw);
    }
    assert_eq!(
        NormalizeServiceURL("127.0.0.1:2379", "http").unwrap(),
        "http://127.0.0.1:2379"
    );
    assert_eq!(
        NormalizeServiceURL("https://127.0.0.1:2379", "http").unwrap(),
        "https://127.0.0.1:2379"
    );
    assert_eq!(
        NormalizeServiceURL(" [::1]:2379 ", "https").unwrap(),
        "https://[::1]:2379"
    );
    assert_eq!(
        NormalizeServiceURL("unixs:///tmp/etcd.sock", "http").unwrap(),
        "unixs:///tmp/etcd.sock"
    );
    assert_eq!(
        NormalizeServiceURL("http://host:service?x=1", "http").unwrap(),
        "http://host:service"
    );
}

#[test]
fn go_merge_34_service_url_rejects_invalid_input() {
    for raw in [
        "",
        "ftp://127.0.0.1:2379",
        "http://127.0.0.1",
        "http://127.0.0.1:2379/path",
        "unix://",
    ] {
        assert!(ParseServiceURL(raw).is_err(), "{raw}");
    }
    assert!(NormalizeServiceURL("invalid_pd_address", "http").is_err());
    assert!(NormalizeServiceURL("127.0.0.1:2379", "ftp").is_err());
    assert_eq!(
        ParseServiceURL("unix://").unwrap_err().to_string(),
        "URL address must not be empty: unix://"
    );
}
