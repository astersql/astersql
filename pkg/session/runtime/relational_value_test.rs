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

use super::relational_like;

#[test]
fn like_underscore_matches_one_unicode_character() {
    assert!(relational_like("_", "中"));
    assert!(relational_like("a_c", "a中c"));
    assert!(!relational_like("__", "中"));
}

#[test]
fn like_backslash_escapes_wildcards() {
    assert!(relational_like(r"a\_c", "a_c"));
    assert!(relational_like(r"a\%c", "a%c"));
    assert!(!relational_like(r"a\_c", "a中c"));
}
