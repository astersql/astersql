// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
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

use crate::optimize_trace::{
    get_optimizer_trace_dir_name, optimizer_trace_dir_for_server_id_for_test,
};

#[test]
fn optimizer_trace_directory_owns_server_info_lookup_like_go() {
    let path = get_optimizer_trace_dir_name();

    assert_eq!(
        path.parent().and_then(|path| path.to_str()),
        Some("optimizer_trace")
    );
    assert!(path.file_name().is_some_and(|name| !name.is_empty()));
}

#[test]
fn optimizer_trace_directory_prefers_non_empty_server_id_and_falls_back_to_pid() {
    assert_eq!(
        optimizer_trace_dir_for_server_id_for_test(Some("server-7")),
        std::path::Path::new("optimizer_trace").join("server-7")
    );
    assert_eq!(
        optimizer_trace_dir_for_server_id_for_test(Some("")),
        std::path::Path::new("optimizer_trace").join(std::process::id().to_string())
    );
    assert_eq!(
        optimizer_trace_dir_for_server_id_for_test(None),
        std::path::Path::new("optimizer_trace").join(std::process::id().to_string())
    );
}
