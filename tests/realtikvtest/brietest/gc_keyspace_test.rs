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

//! `TestKeyspaceBackupUsesGCBarrier`: real API v2 SQL/KV, production backup
//! orchestration, production GC manager and real TiKV SST backup RPCs.
use astersql_br_pkg_gc as gc;
use astersql_br_pkg_task::{
    backup::{BackupConfig, RunBackup},
    stubs::{MemGlue, MemMgr},
};
use astersql_session::runtime::{BootstrapCanonicalDomain, ConcreteSession};
use astersql_tests_realtikvtest_brietest::gc_keyspace_runtime::{
    LocalStorage, RealBackupClient, Rpc,
};
use std::path::PathBuf;
use std::process::Command;
use std::sync::{Arc, Mutex};

const DATABASE: &str = "br_gc_keyspace_full_backup";
struct StoreCleanup(astersql_store_driver::TikvStore);
impl Drop for StoreCleanup {
    fn drop(&mut self) {
        let result = self.0.Close();
        if !std::thread::panicking() {
            result.expect("close keyspace store");
        }
    }
}
struct Cleanup {
    session: ConcreteSession,
    domain: Arc<astersql_domain::Domain>,
}
impl Drop for Cleanup {
    fn drop(&mut self) {
        let result = self
            .session
            .execute(&format!("drop database if exists {DATABASE}"));
        self.domain.close();
        if !std::thread::panicking() {
            assert!(result.is_ok(), "database cleanup failed");
        }
    }
}

#[test]
fn test_keyspace_backup_uses_gc_barrier() {
    let (Ok(pd), Ok(backup_stores)) = (
        std::env::var("REAL_TIKV_PD"),
        std::env::var("REAL_TIKV_BACKUP_STORES"),
    ) else {
        eprintln!(
            "only run this test with real NextGen TiKV: set REAL_TIKV_PD and REAL_TIKV_BACKUP_STORES"
        );
        return;
    };
    // Set failpoints in a fresh child before any RPC/runtime threads exist.
    // Rust 2024 process environment mutation is otherwise unsafe.
    if std::env::var_os("ASTERSQL_GC_SCENARIO_CHILD").is_none() {
        let temp = std::env::temp_dir().join(format!("briesql-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&temp).unwrap();
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "--exact",
                "test_keyspace_backup_uses_gc_barrier",
                "--nocapture",
            ])
            .env("ASTERSQL_GC_SCENARIO_CHILD", "1")
            .env("ASTERSQL_GC_BACKUP_DIR", &temp);
        for (point, file) in [
            ("hint-gc-global-set-safepoint", "gc_global_set"),
            ("hint-gc-global-delete-safepoint", "gc_global_del"),
            ("hint-gc-keyspace-set-barrier", "gc_keyspace_set"),
            ("hint-gc-keyspace-delete-barrier", "gc_keyspace_del"),
        ] {
            command.env(point, temp.join(file));
        }
        let result = command.status();
        std::fs::remove_dir_all(&temp).unwrap();
        assert!(
            result
                .expect("start isolated GC integration process")
                .success()
        );
        return;
    }
    let temp = PathBuf::from(std::env::var_os("ASTERSQL_GC_BACKUP_DIR").unwrap());
    let rpc = Rpc::connect(&pd).expect("connect actual PD");
    let keyspace_id = rpc.keyspace_id("keyspace1").unwrap();
    let store = astersql_store_driver::TiKVDriver::default()
        .Open(&format!(
            "tikv://{pd}?disableGC=true&keyspaceName=keyspace1"
        ))
        .expect("open API v2 keyspace store");
    let _store_cleanup = StoreCleanup(store.clone());
    let domain = Arc::new(astersql_domain::Domain::new(
        store.clone(),
        Arc::new(astersql_domain::KvInfoSchemaLoader::new()),
        Default::default(),
    ));
    domain.init().unwrap();
    let session = BootstrapCanonicalDomain(domain.clone()).unwrap();
    let cleanup = Cleanup {
        session,
        domain: domain.clone(),
    };
    for sql in [
        format!("drop database if exists {DATABASE}"),
        format!("create database {DATABASE}"),
        format!("create table {DATABASE}.t(id int primary key, v int)"),
        format!("insert into {DATABASE}.t values (1, 10), (2, 20), (3, 30)"),
    ] {
        cleanup
            .session
            .execute(&sql)
            .unwrap_or_else(|error| panic!("{sql}: {error}"));
    }
    let table = domain
        .info_schema()
        .TableByName(&DATABASE.into(), &"t".into())
        .unwrap();
    let timestamp = store.CurrentVersion("global").unwrap().0;
    let client = RealBackupClient {
        rpc: rpc.clone(),
        keyspace_id,
        tikv: backup_stores.split(',').map(str::to_owned).collect(),
        timestamp,
        storage: Arc::new(LocalStorage(temp.join("backup"))),
        database: DATABASE.into(),
        table_id: table.Meta().id,
        files: Mutex::new(Vec::new()),
    };
    let mut cfg = BackupConfig::default();
    cfg.Config.Storage = format!("local://{}", temp.join("backup").display());
    cfg.Config.KeyspaceName = "keyspace1".into();
    cfg.Config.CheckRequirements = false;
    cfg.Config.FilterStr = vec![format!("{DATABASE}.*")];
    cfg.UseCheckpoint = false;
    cfg.GCTTL = 120;
    let manager = Arc::new(MemMgr {
        gc_manager: Some(gc::NewManager(Arc::new(rpc.clone()), keyspace_id)),
        cluster_version: "9.0.0".into(),
        region_count: 1,
        ..Default::default()
    });
    RunBackup(
        &MemGlue::default(),
        astersql_br_pkg_task::backup::FullBackupCmd,
        &mut cfg,
        manager,
        &client,
    )
    .unwrap();
    let signal = std::fs::read_to_string(temp.join("gc_keyspace_set")).unwrap();
    assert!(signal.contains("keyspace="));
    assert!(signal.contains("id="));
    assert!(signal.contains(&format!("keyspace={keyspace_id}\n")));
    std::fs::metadata(temp.join("gc_keyspace_del")).unwrap();
    for name in ["gc_global_set", "gc_global_del"] {
        assert_eq!(
            std::fs::metadata(temp.join(name)).unwrap_err().kind(),
            std::io::ErrorKind::NotFound
        );
    }
    let id = signal
        .lines()
        .find_map(|line| line.strip_prefix("id="))
        .unwrap();
    assert!(
        !rpc.state(keyspace_id)
            .unwrap()
            .GCBarriers
            .iter()
            .any(|barrier| barrier.BarrierID == id)
    );
    let files = client.files.lock().unwrap();
    assert_eq!(
        files.iter().map(|(_, count)| count).sum::<u64>(),
        3,
        "real SST backup must include all three inserted rows"
    );
    for (name, _) in files.iter() {
        assert!(
            temp.join("backup").join(name).is_file(),
            "TiKV did not produce SST {name}"
        );
    }
    drop(files);
    drop(cleanup);
}
