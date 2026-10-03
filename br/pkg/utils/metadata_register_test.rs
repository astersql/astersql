// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// http://www.apache.org/licenses/LICENSE-2.0
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.
use crate::MetadataRegisterClient;
use crate::register::{NewTaskRegisterWithTTL, RegisterTaskType};
use astersql_metaservice::{Context, MetaServiceError, MetadataPdClient, PdClient, PdMember};
use std::{sync::Arc, time::Duration};
struct Pd;
impl PdClient for Pd {
    fn get_all_members(&self, _: &Context) -> Result<Vec<PdMember>, MetaServiceError> {
        panic!("preserve caller endpoints")
    }
}
impl MetadataPdClient for Pd {
    fn load_keyspace(
        &self,
        _: &Context,
        _: &str,
    ) -> Result<Option<astersql_metaservice::DialKeyspaceMeta>, MetaServiceError> {
        panic!("existing client path")
    }
    fn close(&self) {
        panic!("borrowed PD must stay open")
    }
}
#[test]
#[ignore = "requires ASTER_ETCD_TEST_ENDPOINT for a real etcd server"]
fn existing_register_state_machine_reuses_lease_and_cleans_real_keys() {
    let endpoint = std::env::var("ASTER_ETCD_TEST_ENDPOINT").unwrap();
    let client = astersql_metaservice::NewEtcdClientFromPDClient(
        &Default::default(),
        &Pd,
        None,
        &[endpoint],
        Default::default(),
    )
    .unwrap();
    let mut register = NewTaskRegisterWithTTL(
        Arc::new(MetadataRegisterClient(client.clone())),
        Duration::from_secs(60),
        RegisterTaskType::RegisterRestore,
        "adapter-test",
    );
    register.RegisterTaskOnce(&Default::default()).unwrap();
    let key = "/tidb/brie/import/restore/adapter-test";
    let original = client.get_entries(key, false).unwrap();
    assert_eq!(original.len(), 1);
    assert!(original[0].lease > 0);
    register.RegisterTaskOnce(&Default::default()).unwrap();
    assert_eq!(
        client.get_entries(key, false).unwrap()[0].lease,
        original[0].lease
    );
    register.Close(&Default::default()).unwrap();
    assert!(client.get(key, false).unwrap().is_empty());
    register.Close(&Default::default()).unwrap();
    client.close().unwrap();
}
