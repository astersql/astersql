// Copyright 2026 PingCAP, Inc.
// Copyright 2026 AsterSQL.
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

use crate::{WildcardPattern, convertRuneUnicodeCI0900, ucadata, unicode0900AICICollator};

#[test]
fn unicode_0900_boundary_rune_uses_implicit_weight() {
    let collator = unicode0900AICICollator::default();
    let last_table_raw = (ucadata::DUCET0900Table.map_table4.len() - 1) as u32;
    let boundary_raw = last_table_raw + 1;
    let last_table_rune = char::from_u32(last_table_raw).expect("table boundary must be a rune");
    let boundary_rune = char::from_u32(boundary_raw).expect("table boundary must be a rune");
    let next_rune = char::from_u32(boundary_raw + 1).expect("next boundary must be a rune");

    let (last_first, last_second) = convertRuneUnicodeCI0900(last_table_rune);
    assert_eq!(
        last_first,
        ucadata::DUCET0900Table.map_table4[last_table_raw as usize]
    );
    assert_eq!(last_second, 0);

    let (first, second) = convertRuneUnicodeCI0900(boundary_rune);
    assert_ne!(first, 0);
    assert_eq!(second, 0);
    assert!(first > last_first);
    assert!(!collator.Key(&boundary_rune.to_string()).is_empty());
    assert_eq!(
        collator.Compare(&last_table_rune.to_string(), &boundary_rune.to_string()),
        -1
    );
    assert_eq!(
        collator.Compare(&boundary_rune.to_string(), &boundary_rune.to_string()),
        0
    );
    assert_eq!(
        collator.Compare(&boundary_rune.to_string(), &next_rune.to_string()),
        -1
    );

    let mut pattern = collator.Pattern();
    pattern.Compile(&boundary_rune.to_string(), b'\\');
    assert!(pattern.DoMatch(&boundary_rune.to_string()));
}
