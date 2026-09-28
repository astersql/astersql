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

use util_collate::{Collator, gb18030BinCollator};

/// GB18030-2022 remaps selected private-use runes to four-byte sequences.
#[test]
fn gb18030_2022_private_use_keys_match_go_custom_encoder() {
    let collator = gb18030BinCollator::default();

    assert_eq!(
        collator.KeyWithoutTrimRightSpace("\u{e78d}"),
        [0x84, 0x31, 0x82, 0x36]
    );
    assert_eq!(
        collator.KeyWithoutTrimRightSpace("\u{e796}"),
        [0x84, 0x31, 0x83, 0x35]
    );
}
