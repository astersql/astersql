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

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use astersql_infoschema::{self as infoschema, SchemaRef};
use astersql_kv as kv;

use crate::canonical_domain_test::TestStorage;
use crate::{Domain, DomainConfig, InfoSchemaLoader, LoadedInfoSchema};

struct DiagnosticSchemaLoader;

impl InfoSchemaLoader for DiagnosticSchemaLoader {
    fn load_info_schema(
        &self,
        _store: &dyn kv::Storage,
        _keyspace: &str,
    ) -> Result<LoadedInfoSchema, kv::errors::SharedError> {
        let schema: SchemaRef = infoschema::infoschema::MockInfoSchemaWithSchemaVer(Vec::new(), 7);
        Ok(LoadedInfoSchema::new(schema, 70))
    }

    fn load_snapshot_info_schema(
        &self,
        store: &dyn kv::Storage,
        keyspace: &str,
        _timestamp: u64,
    ) -> Result<LoadedInfoSchema, kv::errors::SharedError> {
        self.load_info_schema(store, keyspace)
    }

    fn keyspace_exists(
        &self,
        _store: &dyn kv::Storage,
        _keyspace: &str,
    ) -> Result<bool, kv::errors::SharedError> {
        Ok(true)
    }
}

struct RecordingDdl {
    starts: Arc<AtomicUsize>,
}

impl crate::domain::DdlService for RecordingDdl {
    fn start(&self, _mode: crate::domain::StartMode) -> Result<(), String> {
        self.starts.fetch_add(1, Ordering::AcqRel);
        Ok(())
    }

    fn stop(&self) -> Result<(), String> {
        Ok(())
    }

    fn owner_id(&self) -> Option<String> {
        None
    }

    fn alter_table_mode(&self, _target: &str) -> Result<(), String> {
        Ok(())
    }
}

#[test]
fn start_diagnostic_bypasses_ddl_and_starts_only_metadata_readers() {
    let domain = Domain::new(
        TestStorage::new(),
        Arc::new(DiagnosticSchemaLoader),
        DomainConfig {
            schema_lease: Duration::from_secs(60),
            ..DomainConfig::default()
        },
    );
    domain.init().expect("initialize diagnostic Domain");
    let starts = Arc::new(AtomicUsize::new(0));
    domain.set_ddl(Arc::new(RecordingDdl {
        starts: Arc::clone(&starts),
    }));

    domain.start_diagnostic().expect("start diagnostic readers");
    assert_eq!(starts.load(Ordering::Acquire), 0, "DDL must remain stopped");
    assert_eq!(domain.info_schema().SchemaMetaVersion(), 7);
    assert_eq!(domain.background_worker_names(), vec!["schema-reload"]);

    domain.close();
}
