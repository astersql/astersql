// Copyright 2024 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//      http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.
// Copyright 2026 AsterSQL.

use crate::{InternalKey, MvccPropCollector};
use std::collections::HashMap;

/// Go slices `UserKey[:len(UserKey)-8]`, so malformed MVCC keys panic.
#[test]
fn mvcc_collector_panics_for_key_without_timestamp_suffix() {
    let mut collector = MvccPropCollector::new(42);
    let key = InternalKey::new(b"short".to_vec());

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _ = collector.Add(&key, b"");
    }));

    assert!(result.is_err());
}

/// Go retains `curIndexSize`, so each repeated Finish appends the tail anchor.
#[test]
fn mvcc_collector_repeated_finish_reappends_tail_anchor() {
    let mut collector = MvccPropCollector::new(42);
    collector
        .Add(&InternalKey::new(b"first-key.......".to_vec()), b"")
        .unwrap();
    collector
        .Add(&InternalKey::new(b"second-key......".to_vec()), b"")
        .unwrap();

    let mut properties = HashMap::new();
    collector.Finish(&mut properties).unwrap();
    let first_len = properties["tikv.rows_index"].len();
    collector.Finish(&mut properties).unwrap();

    assert!(properties["tikv.rows_index"].len() > first_len);
}
