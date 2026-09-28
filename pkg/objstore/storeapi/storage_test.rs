// Copyright 2026 PingCAP, Inc.
// Copyright 2026 AsterSQL.
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

// 对应 Go `storage_test.go`：覆盖 Prefix 规范化/拼接与 GetHTTPRange 半开区间转换。

use crate::{GetHTTPRange, NewPrefix};

/// 校验空前缀、多斜杠输入规范化，以及 JoinStr / ObjectKey 拼接。
#[test]
fn test_prefix() {
    assert_eq!(NewPrefix("").as_str(), "");
    for value in ["dir", "/dir", "dir/", "/dir/"] {
        assert_eq!(NewPrefix(value).as_str(), "dir/");
    }
    assert_eq!(
        NewPrefix("/dir/sub/sub2/sub3/").as_str(),
        "dir/sub/sub2/sub3/"
    );

    let mut prefix = NewPrefix("");
    assert_eq!(prefix.JoinStr("").as_str(), "");
    for value in ["dir", "/dir", "dir/", "/dir/"] {
        assert_eq!(prefix.JoinStr(value).as_str(), "dir/");
    }

    prefix = NewPrefix("/parent");
    assert_eq!(prefix.JoinStr("").as_str(), "parent/");
    for value in ["dir", "/dir", "dir/", "/dir/"] {
        assert_eq!(prefix.JoinStr(value).as_str(), "parent/dir/");
    }
    assert_eq!(prefix.ObjectKey("file.txt"), "parent/file.txt");
    assert_eq!(prefix.ObjectKey("/file.txt"), "parent//file.txt");
}

/// 校验完整对象、闭区间 Range 与仅起点开区间三种 HTTP Range 输出。
#[test]
fn test_get_http_range() {
    let (full, value) = GetHTTPRange(0, 0);
    assert!(full);
    assert!(value.is_empty());

    let (full, value) = GetHTTPRange(0, 100);
    assert!(!full);
    assert_eq!(value, "bytes=0-99");

    let (full, value) = GetHTTPRange(50, 100);
    assert!(!full);
    assert_eq!(value, "bytes=50-99");

    let (full, value) = GetHTTPRange(50, 0);
    assert!(!full);
    assert_eq!(value, "bytes=50-");
}
