// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
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

use util_collate::{Collator, gbkBinCollator};

// Go's custom GBK encoder deliberately rejects U+20AC instead of using the
// WHATWG/CP936 single-byte 0x80 mapping. gbk_bin replaces every encoding error
// with '?' for both keys and comparisons.
#[test]
fn euro_uses_tidb_custom_gbk_replacement() {
    let collator = gbkBinCollator;

    assert_eq!(collator.KeyWithoutTrimRightSpace("€"), vec![b'?']);
    assert_eq!(collator.Key("€ "), vec![b'?']);
    assert_eq!(collator.ImmutableKey("€ "), vec![b'?']);
    assert_eq!(collator.Compare("€", "?"), 0);
}
