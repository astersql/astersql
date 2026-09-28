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

use std::collections::BTreeMap;
use std::sync::Arc;
use std::thread;

use super::reorg::{
    ReorgContext, ReorgElement, ReorgInfo, encode_temporary_index_range,
    split_keys_for_temporary_index_ranges, update_backfill_progress,
};

#[test]
fn test_reorg_context_set_max_progress() {
    let context = ReorgContext::default();
    for (new_progress, expected) in [
        (0.5, 0.5),
        (0.7, 0.7),
        (0.3, 0.7),
        (0.7, 0.7),
        (0.9, 0.9),
        (1.25, 1.25),
    ] {
        assert_eq!(context.set_max_progress(new_progress), expected);
    }
}

#[test]
fn test_reorg_context_set_max_progress_concurrently() {
    let context = Arc::new(ReorgContext::default());
    let handles: Vec<_> = (0..100)
        .map(|index| {
            let context = context.clone();
            thread::spawn(move || context.set_max_progress(index as f64 / 100.0))
        })
        .collect();
    for handle in handles {
        handle.join().expect("progress worker panicked");
    }
    assert_eq!(context.set_max_progress(0.0), 0.99);
}

#[test]
fn merge_warnings_requires_both_go_maps_to_be_non_empty() {
    let context = ReorgContext::default();
    let warnings = BTreeMap::from([("code".to_owned(), "warning".to_owned())]);
    context.merge_warnings(&warnings, &BTreeMap::new());
    assert!(context.take_warnings().is_empty());

    context.merge_warnings(&warnings, &BTreeMap::from([("code".to_owned(), 2)]));
    context.merge_warnings(
        &BTreeMap::from([("code".to_owned(), "replacement".to_owned())]),
        &BTreeMap::from([("code".to_owned(), 3)]),
    );
    assert_eq!(
        context.take_warnings(),
        BTreeMap::from([("code".to_owned(), ("warning".to_owned(), 5))])
    );
}

#[test]
fn element_ids_include_only_index_elements_like_go() {
    let info = ReorgInfo {
        element: ReorgElement {
            id: 99,
            element_type: b"_idx_".to_vec(),
        },
        elements: vec![
            ReorgElement {
                id: 3,
                element_type: b"_idx_".to_vec(),
            },
            ReorgElement {
                id: 4,
                element_type: b"_col_".to_vec(),
            },
        ],
        ..ReorgInfo::default()
    };
    assert_eq!(info.element_ids(), vec![3]);
    assert!(ReorgInfo::default().element_ids().is_empty());
}

#[test]
fn temporary_index_keys_match_go_tablecodec_encoding_and_filtering() {
    fn go_index_seek_key(table_id: i64, index_id: i64, suffix: &[u8]) -> Vec<u8> {
        let mut key = b"t".to_vec();
        key.extend_from_slice(&((table_id as u64) ^ (1_u64 << 63)).to_be_bytes());
        key.extend_from_slice(b"_i");
        key.extend_from_slice(&((index_id as u64) ^ (1_u64 << 63)).to_be_bytes());
        key.extend_from_slice(suffix);
        key
    }
    let elements = [
        ReorgElement {
            id: 3,
            element_type: b"_idx_".to_vec(),
        },
        ReorgElement {
            id: 4,
            element_type: b"_col_".to_vec(),
        },
        ReorgElement {
            id: 5,
            element_type: b"_idx_".to_vec(),
        },
    ];
    let expected = [3, 5]
        .into_iter()
        .map(|id| go_index_seek_key(42, 0x7fff_0000_0000_0000 | id, &[]))
        .collect::<Vec<_>>();
    assert_eq!(
        split_keys_for_temporary_index_ranges(42, &elements),
        expected
    );

    let (start, end) = encode_temporary_index_range(42, 3, 5);
    assert_eq!(start, expected[0]);
    assert_eq!(
        end,
        go_index_seek_key(42, 0x7fff_0000_0000_0000 | 5, &[u8::MAX])
    );
}

#[test]
fn merging_temporary_index_uses_the_same_progress_ratio_as_go() {
    assert_eq!(update_backfill_progress(50, 100, false), 0.5);
    assert_eq!(update_backfill_progress(50, 100, true), 0.5);
}
