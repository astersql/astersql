// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

use crate::show_bdr_role::{AdminShowBDRRoleExec, ShowBdrRoleRuntime};
use astersql_util_chunk::Chunk;

#[derive(Debug, PartialEq)]
struct CommitError;

struct CommitFailingRuntime;

impl ShowBdrRoleRuntime for CommitFailingRuntime {
    type Context = ();
    type Error = CommitError;

    fn reset_chunk(&self, request: &mut Chunk) {
        request.Reset();
    }

    fn run_in_new_admin_transaction(
        &mut self,
        _context: &mut Self::Context,
        request: &mut Chunk,
        done: &mut bool,
    ) -> Result<(), Self::Error> {
        // Model the Go callback completing before RunInNewTxn reports a commit error.
        request.numVirtualRows += 1;
        *done = true;
        Err(CommitError)
    }
}

#[test]
fn commit_failure_preserves_callback_side_effects_like_go() {
    let mut executor = AdminShowBDRRoleExec {
        runtime: CommitFailingRuntime,
        done: false,
    };
    let mut request = Chunk::default();

    assert_eq!(executor.Next(&mut (), &mut request), Err(CommitError));
    assert!(executor.done);
    assert_eq!(request.NumRows(), 1);
}
