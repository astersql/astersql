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

use super::*;
use std::sync::Arc;

#[test]
fn question_mark_follows_go_byte_index_semantics_for_utf8() {
    let selector = NewTrieSelector();
    selector
        .Insert("?", "", Some(Arc::new("rule")), Insert)
        .unwrap();

    // Go's `for i := range s` visits rune starts but compares s[i] as a byte.
    // A two-byte rune therefore does not satisfy a one-byte `?` pattern.
    assert!(selector.Match("é", "").0.is_empty());
}
