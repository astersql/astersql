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

use super::funcdep_misc::hashCodeKey;

/// Go string map keys preserve arbitrary HashCode bytes; distinct invalid UTF-8
/// byte sequences must therefore remain distinct in the Rust FDSet key space.
#[test]
fn test_hash_code_key_preserves_arbitrary_bytes() {
    assert_ne!(hashCodeKey(&[0x80]), hashCodeKey(&[0x81]));
    assert_ne!(hashCodeKey(&[0xff, 0x00]), hashCodeKey(&[0xfe, 0x00]));
}
