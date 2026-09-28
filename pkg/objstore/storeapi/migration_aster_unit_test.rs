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

// Aster 迁移单元测试：核对 Prefix / BucketPrefix 规范化与拼接、权限常量字符串，
// 以及 GetHTTPRange、GenPermCheckObjectKey 与 Go 行为一致。

use crate::*;

/// 验证 NewPrefix 去斜杠规范化、JoinStr / ObjectKey / ToPath 拼接语义。
#[test]
fn prefix_matches_go_normalization_and_join_behavior() {
    assert_eq!(NewPrefix(""), Prefix::default());
    for value in ["dir", "/dir", "dir/", "/dir/"] {
        assert_eq!(NewPrefix(value).as_str(), "dir/");
    }
    assert_eq!(
        NewPrefix("/dir/sub/sub2/sub3/").as_str(),
        "dir/sub/sub2/sub3/"
    );

    let empty = NewPrefix("");
    assert_eq!(empty.JoinStr("").as_str(), "");
    for value in ["dir", "/dir", "dir/", "/dir/"] {
        assert_eq!(empty.JoinStr(value).as_str(), "dir/");
    }

    let parent = NewPrefix("/parent");
    assert_eq!(parent.JoinStr("").as_str(), "parent/");
    assert_eq!(parent.JoinStr("/dir/").as_str(), "parent/dir/");
    assert_eq!(parent.ObjectKey("file.txt"), "parent/file.txt");
    assert_eq!(parent.ObjectKey("/file.txt"), "parent//file.txt");
    assert_eq!(parent.ToPath(), "/parent/");
}

/// 验证 BucketPrefix 字段与 Permission 常量展示值对齐 Go。
#[test]
fn bucket_prefix_and_permissions_match_go_values() {
    let bucket = NewBucketPrefix("bucket-a", "/base/");
    assert_eq!(bucket.Bucket, "bucket-a");
    assert_eq!(bucket.PrefixStr(), "base/");
    assert_eq!(bucket.ObjectKey("object"), "base/object");
    assert_eq!(AccessBuckets.as_str(), "AccessBucket");
    assert_eq!(ListObjects.as_str(), "ListObjects");
    assert_eq!(GetObject.as_str(), "GetObject");
    assert_eq!(PutObject.as_str(), "PutObject");
    assert_eq!(PutAndDeleteObject.as_str(), "PutAndDeleteObject");
    assert_eq!(AccessBuckets, Permission::AccessBuckets);
}

/// 验证半开区间 [start, end) 转成 HTTP Range 两端闭区间的规则。
#[test]
fn http_range_matches_go_half_open_offsets() {
    assert_eq!(GetHTTPRange(0, 0), (true, String::new()));
    assert_eq!(GetHTTPRange(0, 100), (false, "bytes=0-99".into()));
    assert_eq!(GetHTTPRange(50, 100), (false, "bytes=50-99".into()));
    assert_eq!(GetHTTPRange(50, 0), (false, "bytes=50-".into()));
    assert_eq!(GetHTTPRange(50, 50), (false, "bytes=50-".into()));
}

/// 验证权限探测对象 key 位于 perm-check/ 下且后缀为唯一 UUID。
#[test]
fn permission_check_keys_use_the_expected_unique_path() {
    let first = GenPermCheckObjectKey();
    let second = GenPermCheckObjectKey();
    assert_ne!(first, second);
    for key in [first, second] {
        let uuid = key
            .strip_prefix("perm-check/")
            .expect("permission-check directory");
        assert_eq!(uuid::Uuid::parse_str(uuid).unwrap().to_string(), uuid);
    }
}
