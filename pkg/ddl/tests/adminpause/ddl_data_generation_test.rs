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

use std::sync::Arc;

use astersql_domain_infosync::{Codec, GetMockTiFlash, GlobalInfoSyncerInit};

use crate::generate_tbl_user_with_vec;

#[test]
fn vector_generation_registers_the_go_mock_tiflash_side_effect() {
    GlobalInfoSyncerInit(
        "adminpause-data-generation-test".to_owned(),
        Arc::new(|| 1),
        None,
        None,
        None,
        Codec::default(),
        false,
        None,
    )
    .unwrap();
    let mut executor = RecordingExecutor::default();

    generate_tbl_user_with_vec(&mut executor, 1).unwrap();

    assert!(GetMockTiFlash().unwrap().is_some());
}

#[derive(Default)]
struct RecordingExecutor;

impl crate::SqlExecutor for RecordingExecutor {
    fn must_exec(&mut self, _sql: &str) -> Result<(), String> {
        Ok(())
    }

    fn exec(&mut self, _sql: &str) -> Result<(), String> {
        Ok(())
    }
}
