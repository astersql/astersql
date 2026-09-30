// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
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

use astersql_sessionctx_variable::session::SessionVars;
use std::collections::BTreeSet;

/// Returns the database labels for SQL metrics, honoring RecordDBLabel.
#[allow(non_snake_case)]
pub fn GetDBNames(vars: Option<&SessionVars>) -> Vec<String> {
    let Some(vars) = vars else {
        return vec![String::new()];
    };
    if !astersql_config::get_global_config().status.record_db_label {
        return vec![String::new()];
    }
    let mut names = vars
        .StmtCtx
        .LogicalPlanTables()
        .into_iter()
        .map(|table| table.DB)
        .collect::<BTreeSet<_>>();
    if names.is_empty() {
        names.insert(vars.CurrentDB().to_lowercase());
    }
    names.into_iter().collect()
}
