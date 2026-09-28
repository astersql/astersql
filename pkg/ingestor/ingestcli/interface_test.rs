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

use crate::{Error, RequestContext, SplitClient, Store};

struct CancelledContext;

impl RequestContext for CancelledContext {
    fn is_cancelled(&self) -> bool {
        true
    }
}

struct ContextAwareSplitClient;

impl SplitClient for ContextAwareSplitClient {
    fn get_store(&self, context: &dyn RequestContext, _store_id: u64) -> Result<Store, Error> {
        assert!(context.is_cancelled());
        Ok(Store::default())
    }
}

#[test]
fn split_client_get_store_preserves_request_context() {
    let client = ContextAwareSplitClient;
    let context = CancelledContext;

    client
        .get_store(&context, 42)
        .expect("the exact request context reaches the split client");
}
