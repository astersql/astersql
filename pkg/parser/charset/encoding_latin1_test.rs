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

use crate::encoding::{Encoding, OpEncode, OpReplaceNoErr};
use crate::encoding_latin1::ENCODING_LATIN1_IMPL;

#[test]
fn latin1_promoted_utf8_methods_match_go() {
    assert_eq!(ENCODING_LATIN1_IMPL.MbLen("中".as_bytes()), 3);
    assert_eq!(ENCODING_LATIN1_IMPL.MbLen(b"a"), 0);

    let input = "A中😂".as_bytes();
    let mut chunks = Vec::new();
    ENCODING_LATIN1_IMPL.Foreach(input, OpReplaceNoErr, &mut |from, to, ok| {
        chunks.push((from.to_vec(), to.to_vec(), ok));
        true
    });
    assert_eq!(
        chunks,
        vec![
            (b"A".to_vec(), b"A".to_vec(), true),
            ("中".as_bytes().to_vec(), "中".as_bytes().to_vec(), true),
            ("😂".as_bytes().to_vec(), "😂".as_bytes().to_vec(), true),
        ]
    );

    let malformed = b"\xe2(\xa1";
    let mut malformed_chunks = Vec::new();
    ENCODING_LATIN1_IMPL.Foreach(malformed, OpReplaceNoErr, &mut |from, _, ok| {
        malformed_chunks.push((from.to_vec(), ok));
        true
    });
    assert_eq!(
        malformed_chunks,
        vec![
            (b"\xe2".to_vec(), false),
            (b"(".to_vec(), true),
            (b"\xa1".to_vec(), false),
        ]
    );
}

#[test]
fn latin1_transform_is_noop_and_preserves_destination() {
    let mut dest = b"caller-owned-capacity".to_vec();
    let input = b"\xff\x00latin1";
    let output = ENCODING_LATIN1_IMPL
        .Transform(&mut dest, input, OpEncode)
        .unwrap();
    assert_eq!(output, input);
    assert_eq!(dest, b"caller-owned-capacity");
}
