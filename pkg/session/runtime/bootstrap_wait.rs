// Copyright 2026 AsterSQL.
// Copyright 2015 PingCAP, Inc.
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

use astersql_kv as kv;
use astersql_util_logutil::log::{BgLogger, LogField, LogLevel};
use std::time::Duration;

// Bootstrap metadata is read in a fresh retryable internal transaction, as in Go.
pub(super) fn must_get_store_bootstrap_version(store: &dyn kv::Storage) -> i64 {
    let ctx = kv::WithInternalSourceType(kv::Context::default(), kv::InternalTxnBootstrap);
    let mut version = 0;
    kv::RunInNewTxn(&ctx, store, true, |ctx, txn| {
        version = match txn.Get(
            ctx,
            astersql_meta::transaction_meta_string_key(b"BootstrapKey"),
            &[],
        ) {
            Ok(value) => std::str::from_utf8(&value.Value)
                .map_err(|e| kv::errors::New(e.to_string()))?
                .parse::<i64>()
                .map_err(|e| kv::errors::New(e.to_string()))?,
            Err(e) if kv::IsErrNotFound(&e) => 0,
            Err(e) => return Err(e),
        };
        Ok(())
    })
    .unwrap_or_else(|error| {
        BgLogger().log(
            LogLevel::Fatal,
            "get store bootstrap version failed",
            [LogField::String("error".to_owned(), error.to_string())],
        );
        panic!("get store bootstrap version failed: {error}");
    });
    version
}

// Only time and logging are replaceable; every attempt reads the real metadata transaction.
pub(super) trait BootstrapWaitClock {
    fn sleep(&mut self, duration: Duration);
    fn log_wait(&mut self);
}
pub(super) struct SystemBootstrapClock(std::time::Instant);
impl SystemBootstrapClock {
    pub(super) fn new() -> Self {
        Self(std::time::Instant::now())
    }
}
impl BootstrapWaitClock for SystemBootstrapClock {
    fn sleep(&mut self, duration: Duration) {
        std::thread::sleep(duration);
    }
    fn log_wait(&mut self) {
        BgLogger().log(
            LogLevel::Info,
            "waiting for the SYSTEM keyspace bootstrap to complete",
            [LogField::String(
                "total-waited".to_owned(),
                format!("{:?}", self.0.elapsed()),
            )],
        );
    }
}
pub(super) fn wait_system_boot_version_with_clock(
    store: &dyn kv::Storage,
    clock: &mut impl BootstrapWaitClock,
) -> i64 {
    let mut version = 0;
    for attempt in 0..360 {
        version = must_get_store_bootstrap_version(store);
        if version != 0 {
            break;
        }
        if (attempt + 1) % 5 == 0 {
            clock.log_wait();
        }
        clock.sleep(Duration::from_secs(match attempt {
            0 => 1,
            1 => 2,
            2 => 4,
            _ => 5,
        }));
    }
    version
}

pub(super) fn check_system_bootstrap_version(
    store: &dyn kv::Storage,
    target: i64,
    current: i64,
    clock: &mut impl BootstrapWaitClock,
) {
    let version = wait_system_boot_version_with_clock(store, clock);
    if version == 0 {
        BgLogger().log(LogLevel::Fatal, "SYSTEM keyspace is not bootstrapped", []);
        panic!("SYSTEM keyspace is not bootstrapped");
    }
    if target > version {
        BgLogger().log(LogLevel::Fatal,
            "bootstrap version of user keyspace must be smaller or equal to that of SYSTEM keyspace. if you are upgrading user keyspace, please make sure to upgrade SYSTEM keyspace first",
            [LogField::I64("userCurr".to_owned(), current), LogField::I64("userTarget".to_owned(), target), LogField::I64("system".to_owned(), version)]);
        panic!(
            "bootstrap version of user keyspace must be smaller or equal to that of SYSTEM keyspace. userTarget={target} system={version}"
        );
    }
}

// The canonical bootstrap must publish the same key that the waiting reader observes.
pub(super) fn finish_store_bootstrap_version(store: &dyn kv::Storage, version: i64) {
    let ctx = kv::WithInternalSourceType(kv::Context::default(), kv::InternalTxnBootstrap);
    kv::RunInNewTxn(&ctx, store, true, |_, txn| {
        txn.Set(
            astersql_meta::transaction_meta_string_key(b"BootstrapKey"),
            version.to_string().into_bytes(),
        )
    })
    .unwrap_or_else(|error| panic!("finish bootstrap failed: {error}"));
}
