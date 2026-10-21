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

//! Read-only Domain startup used by diagnostic servers.

use crate::{Domain, DomainError};

impl Domain {
    /// Starts the query-side schema reader without starting DDL execution or
    /// server/topology maintenance.
    pub fn start_diagnostic(&self) -> Result<(), DomainError> {
        self.begin_start()?;
        self.start_schema_reload_worker(true);
        Ok(())
    }

    /// Loads the existing statistics cache without enabling any writer,
    /// ownership, GC, usage-history, or auto-analyze path.
    pub fn load_stats_diagnostic(&self) -> Result<(), DomainError> {
        self.initialize_stats()
    }
}
