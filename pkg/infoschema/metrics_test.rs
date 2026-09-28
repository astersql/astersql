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

use crate::metrics::newSieveStatusHookImpl;
use crate::sieve::SieveStatusHook;
use astersql_infoschema_metrics::{
    InfoSchemaV2CacheCounter, InfoSchemaV2CacheMemLimit, InfoSchemaV2CacheMemUsage,
    InfoSchemaV2CacheObjCnt,
};

#[test]
fn sieve_status_hook_updates_shared_prometheus_metrics() {
    let hook = newSieveStatusHookImpl();
    let evict = InfoSchemaV2CacheCounter.with_label_values(&["evict"]);
    let hit = InfoSchemaV2CacheCounter.with_label_values(&["hit"]);
    let miss = InfoSchemaV2CacheCounter.with_label_values(&["miss"]);
    let before = (evict.get(), hit.get(), miss.get());

    hook.on_evict();
    hook.on_hit();
    hook.on_miss();
    hook.on_update(123, 7);
    hook.on_update_limit(456);

    assert_eq!(evict.get(), before.0 + 1.0);
    assert_eq!(hit.get(), before.1 + 1.0);
    assert_eq!(miss.get(), before.2 + 1.0);
    assert_eq!(InfoSchemaV2CacheMemUsage.get(), 123.0);
    assert_eq!(InfoSchemaV2CacheObjCnt.get(), 7.0);
    assert_eq!(InfoSchemaV2CacheMemLimit.get(), 456.0);
}
