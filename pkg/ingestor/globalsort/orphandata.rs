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

//! Scan and publish global-sort data that has no active DXF producer.

use anyhow::{Context as _, Result};
use astersql_objstore::storage::{Context, Storage, StorageRef};
use std::sync::Arc;

pub const SAMPLE_OBJECT_LIMIT: usize = 10;

pub trait ActiveProducerChecker: Send + Sync {
    fn has_active_producers(&self) -> Result<bool>;
}

impl<F> ActiveProducerChecker for F
where
    F: Fn() -> Result<bool> + Send + Sync,
{
    fn has_active_producers(&self) -> Result<bool> {
        self()
    }
}

pub type StorageFactory = Arc<dyn Fn(&Context, &str) -> Result<StorageRef> + Send + Sync>;

pub struct Config {
    pub active_producer_checker: Arc<dyn ActiveProducerChecker>,
    pub get_storage_uri: Arc<dyn Fn() -> String + Send + Sync>,
    pub retained_prefixes: Vec<String>,
    pub store_factory: Option<StorageFactory>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ScanStats {
    pub size_bytes: i64,
    pub object_count: i64,
    pub sample_objects: Vec<String>,
    pub truncated: bool,
}

pub struct Monitor {
    config: Config,
    store_factory: StorageFactory,
}

impl Monitor {
    pub fn new(config: Config) -> Self {
        let store_factory = config.store_factory.clone().unwrap_or_else(|| {
            Arc::new(|context, uri| {
                astersql_objstore::storage::NewFromURL(context, uri)
                    .context("create global sort orphan-data store")
            })
        });
        Self {
            config,
            store_factory,
        }
    }

    /// Runs synchronously on the scheduler cleanup owner. Failures deliberately
    /// preserve the previously published value; cancellation is equally silent.
    pub fn trigger(&self, context: &Context) {
        let uri = (self.config.get_storage_uri)();
        if uri.is_empty() {
            return;
        }
        let Ok(false) = self.config.active_producer_checker.has_active_producers() else {
            return;
        };
        if context.is_cancelled() {
            return;
        }
        let Ok(storage) = (self.store_factory)(context, &uri) else {
            return;
        };
        let scanned = scan_orphan_data(context, storage.as_ref(), &self.config.retained_prefixes);
        storage.Close();
        let Ok(stats) = scanned else {
            return;
        };
        let Ok(false) = self.config.active_producer_checker.has_active_producers() else {
            return;
        };
        astersql_metrics::globalsort::GlobalSortOrphanDataSize
            .with_label_values(&[] as &[&str])
            .set(stats.size_bytes as f64);
    }

    pub fn reset(&self) {
        astersql_metrics::globalsort::GlobalSortOrphanDataSize.reset();
    }
}

pub fn scan_orphan_data(
    context: &Context,
    storage: &dyn Storage,
    retained_prefixes: &[String],
) -> Result<ScanStats> {
    let mut stats = ScanStats::default();
    storage
        .WalkDir(context, None, &mut |path, size| {
            if retained_prefixes
                .iter()
                .any(|prefix| path.starts_with(prefix))
            {
                return Ok(());
            }
            stats.object_count += 1;
            if stats.sample_objects.len() < SAMPLE_OBJECT_LIMIT {
                stats.sample_objects.push(path.to_owned());
            } else {
                stats.truncated = true;
            }
            if size >= 0 {
                stats.size_bytes += size;
            }
            Ok(())
        })
        .context("scan global sort orphan objects")?;
    Ok(stats)
}
