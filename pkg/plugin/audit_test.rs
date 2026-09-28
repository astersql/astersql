// Copyright 2026 AsterSQL.
// Copyright 2019 PingCAP, Inc.
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

use std::time::SystemTime;

use crate::{
    Context, EXEC_START_TIME_CONTEXT_KEY, IS_RETRYING_CONTEXT_KEY, PREPARE_STATEMENT_ID_CONTEXT_KEY,
};

/// Go's exported audit context keys all support `context.WithValue` / `Value`.
#[test]
fn audit_context_keys_store_their_go_value_types() {
    let start_time = SystemTime::UNIX_EPOCH;
    let context = Context::default()
        .with_value(EXEC_START_TIME_CONTEXT_KEY, start_time)
        .with_value(PREPARE_STATEMENT_ID_CONTEXT_KEY, 42_u32)
        .with_value(IS_RETRYING_CONTEXT_KEY, true);

    assert_eq!(
        context.value(EXEC_START_TIME_CONTEXT_KEY).as_deref(),
        Some(&start_time)
    );
    assert_eq!(
        context.value(PREPARE_STATEMENT_ID_CONTEXT_KEY).as_deref(),
        Some(&42)
    );
    assert_eq!(
        context.value(IS_RETRYING_CONTEXT_KEY).as_deref(),
        Some(&true)
    );
}
