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

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use astersql_parser_mysql::r#const::{ComSleep, Command2Str, ServerStatusInTrans};
use astersql_server::server::{
    ManagedConnection, ProcessInfo, Server, ServerConfig, ServerDriver, TransactionInfo,
};

use crate::{IdleWatcherConfig, LoadKeyspaceController, RecordingExitSignaler, start_idle_watcher};

struct Driver;

impl ServerDriver for Driver {
    fn name(&self) -> &str {
        "tidb"
    }
}

struct InTransactionSleepConnection;

impl ManagedConnection for InTransactionSleepConnection {
    fn id(&self) -> u64 {
        1
    }

    fn capability(&self) -> u32 {
        0
    }

    fn process_info(&self) -> Option<ProcessInfo> {
        Some(ProcessInfo {
            connection_id: self.id(),
            command: Command2Str
                .iter()
                .find_map(|&(value, name)| (value == ComSleep).then_some(name))
                .expect("ComSleep command name")
                .to_owned(),
            state: ServerStatusInTrans,
            ..Default::default()
        })
    }

    // Real ClientConn currently exposes transaction state through ProcessInfo.state,
    // exactly like Go GetUserProcessList, rather than this optional summary.
    fn transaction_info(&self) -> Option<TransactionInfo> {
        None
    }

    fn connection_attributes(&self) -> HashMap<String, String> {
        HashMap::new()
    }

    fn status_variables(&self) -> HashMap<String, String> {
        HashMap::new()
    }

    fn update_cpu_time(&self, _sql_id: u64, _cpu_time: Duration) {}

    fn kill_query(&self, _max_execution_time: bool, _runaway: bool) {}

    fn close(&self) {}
}

#[test]
fn in_transaction_sleep_connection_prevents_idle_exit() {
    let exit = Arc::new(RecordingExitSignaler::default());
    let controller = LoadKeyspaceController::with_exit_signaler(None, exit.clone());
    let server = Server::new_test(ServerConfig::default(), Arc::new(Driver));
    server
        .register_connection(Arc::new(InTransactionSleepConnection))
        .expect("register connection");

    let watcher = start_idle_watcher(
        controller,
        Arc::clone(&server),
        IdleWatcherConfig {
            max_idle: Duration::from_nanos(1),
            check_interval: Duration::from_millis(5),
            ..Default::default()
        },
    )
    .expect("watcher enabled");

    std::thread::sleep(Duration::from_millis(1_100));
    server.set_force_shutdown();
    watcher.join().expect("watcher thread");

    assert_eq!(exit.take(), None);
}
