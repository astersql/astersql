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

use super::precheck::{check_import_size_limit, display_bytes, is_supported_cloud_uri};

#[test]
fn starter_import_real_size_limit_matches_go_boundary() {
    assert!(check_import_size_limit(100, 200, false, 100).is_ok());
    assert!(check_import_size_limit(100, 200, true, 0).is_ok());
    assert!(check_import_size_limit(100, 100, true, 100).is_ok());
    let error = check_import_size_limit(50, 200, true, 100).unwrap_err();
    assert!(error.contains("200B exceeds maximum import size limit 100B"));
    assert!(error.contains("total file size 50B"));
}

#[test]
fn starter_limit_sizes_use_go_units_format() {
    assert_eq!(display_bytes(0), "0B");
    assert_eq!(display_bytes(2), "2B");
    assert_eq!(display_bytes(1024), "1KiB");
    assert_eq!(display_bytes(1536), "1.5KiB");
    assert_eq!(display_bytes(123_456), "120.6KiB");
    assert_eq!(display_bytes(1 << 20), "1MiB");
}

#[test]
fn global_sort_uri_accepts_only_go_cloud_backends_with_a_bucket() {
    for uri in [
        "s3://bucket/path",
        "gcs://bucket/path",
        "gs://bucket/path",
        "azure://container/path",
        "azblob://container/path",
    ] {
        assert!(is_supported_cloud_uri(uri), "expected supported URI: {uri}");
    }

    for uri in [
        ":",
        "s3://",
        "s3:///path",
        "local:///tmp",
        "unknown://bucket",
    ] {
        assert!(!is_supported_cloud_uri(uri), "expected rejected URI: {uri}");
    }
}

#[test]
fn global_sort_missing_bucket_propagates_redacted_invalid_uri() {
    let uri =
        "s3:///path?access-key=secret-id&secret-access-key=secret-key&session-token=secret-token";
    let error = super::precheck::validate_global_sort_uri(uri).unwrap_err();
    let reason = "please specify the bucket for s3 in s3:///path?access-key=xxxxxx&secret-access-key=xxxxxx&session-token=xxxxxx";
    let expected = astersql_util_dbterror_exeerrors::exeerrors::ErrLoadDataInvalidURI
        .GenWithStackByArgs(&["cloud storage".into(), reason.into()])
        .to_string();
    assert_eq!(error, expected);
    for secret in ["secret-id", "secret-key", "secret-token"] {
        assert!(!error.contains(secret));
    }
}
