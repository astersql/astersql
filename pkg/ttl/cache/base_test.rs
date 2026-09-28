// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

// `baseCache` 刷新判定的单元测试。
//
// 覆盖：新建后应立即需要更新；`MarkUpdated` 后在长间隔内不应再触发刷新；
// `GetInterval` / `SetInterval` 读写一致性。

use crate::base::newBaseCache;
use std::time::Duration;

// 对应 Go TestBaseCache：新建的缓存立即需要刷新；标记更新后，长间隔应认为不需要刷新。
#[test]
fn test_base_cache() {
    let mut base_cache = newBaseCache(Duration::from_nanos(1));
    std::thread::sleep(Duration::from_micros(1));

    assert!(base_cache.ShouldUpdate());

    base_cache.MarkUpdated();
    base_cache.SetInterval(Duration::from_secs(3600));
    assert!(!base_cache.ShouldUpdate());
}

/// 验证 `GetInterval` / `SetInterval` 能正确读写刷新间隔。
#[test]
fn test_base_cache_get_interval() {
    let mut base_cache = newBaseCache(Duration::from_secs(1));
    assert_eq!(base_cache.GetInterval(), Duration::from_secs(1));
    base_cache.SetInterval(Duration::from_secs(2));
    assert_eq!(base_cache.GetInterval(), Duration::from_secs(2));
}
