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

use crate::backend_mgr::{decode_backend_tag, encode_backend_tag, generate_job_sort_path};

#[test]
fn backend_tags_match_go_parse_int_contract() {
    assert_eq!(encode_backend_tag(42, false), "42");
    assert_eq!(encode_backend_tag(42, true), "42-dup");
    assert_eq!(decode_backend_tag("42"), Ok(42));
    assert_eq!(decode_backend_tag("-7"), Ok(-7));
    assert_eq!(decode_backend_tag("+42"), Ok(42));

    for invalid in ["", "42-dup", "-7-dup", " 42", "42 "] {
        assert!(decode_backend_tag(invalid).is_err(), "{invalid:?}");
    }
}

#[test]
fn job_sort_path_matches_filepath_join_for_common_bases() {
    assert_eq!(generate_job_sort_path("", 42, false), "42");
    assert_eq!(
        generate_job_sort_path("/tmp/ingest", 42, false),
        "/tmp/ingest/42"
    );
    assert_eq!(
        generate_job_sort_path("/tmp/ingest/", 42, true),
        "/tmp/ingest/42-dup"
    );
}
