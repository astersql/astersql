// Copyright 2026 AsterSQL.
// Copyright 2018 PingCAP, Inc.
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

use std::cell::Cell;
use std::io;

use crate::meminfo::{MemoryHookKind, MemoryInfoResult, select_memory_hook};

fn ok(value: u64) -> MemoryInfoResult {
    Ok(value)
}

fn error(message: &'static str) -> MemoryInfoResult {
    Err(io::Error::other(message).into())
}

#[test]
fn container_selection_matches_go_and_skips_fallible_probes() {
    let limit_called = Cell::new(false);
    let physical_called = Cell::new(false);

    let selected = select_memory_hook(
        true,
        || {
            limit_called.set(true);
            error("must not read the limit in the container branch")
        },
        || {
            physical_called.set(true);
            error("must not read physical memory in the container branch")
        },
    )
    .expect("Go's in-container branch returns without probing");

    assert_eq!(selected, MemoryHookKind::CGroup);
    assert!(!limit_called.get());
    assert!(!physical_called.get());
}

#[test]
fn non_container_selection_propagates_errors_and_compares_nonzero_limit() {
    let limit_error = select_memory_hook(false, || error("limit failed"), || ok(1024))
        .expect_err("Go propagates GetMemoryLimit errors");
    assert!(limit_error.to_string().contains("limit failed"));

    assert_eq!(
        select_memory_hook(false, || ok(512), || ok(1024)).unwrap(),
        MemoryHookKind::CGroup
    );
    assert_eq!(
        select_memory_hook(false, || ok(0), || ok(1024)).unwrap(),
        MemoryHookKind::Normal
    );
    assert_eq!(
        select_memory_hook(false, || ok(2048), || ok(1024)).unwrap(),
        MemoryHookKind::Normal
    );
}
