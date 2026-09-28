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

use crate::encoding_ascii::{ENCODING_ASCII_IMPL, init_encoding_ascii};
use crate::{EncodingTp, OpReplaceNoErr, TransformResult};

fn ascii() -> &'static crate::encoding_ascii::EncodingAscii {
    init_encoding_ascii();
    ENCODING_ASCII_IMPL.get().expect("ASCII initialized")
}

#[test]
fn ascii_explicit_and_promoted_methods_match_go() {
    let ascii = ascii();

    assert_eq!(ascii.name(), "ascii");
    assert_eq!(ascii.tp(), EncodingTp::Ascii);
    assert_eq!(ascii.peek(b""), b"");
    assert_eq!(ascii.peek(b"ab"), b"a");
    assert_eq!(ascii.mb_len("中"), 0);
    assert_eq!(ascii.to_upper("az"), "AZ");
    assert_eq!(ascii.to_lower("AZ"), "az");
}

#[test]
fn ascii_validation_transform_and_foreach_match_go_boundaries() {
    let ascii = ascii();
    assert!(ascii.is_valid(b"\0\x7f"));
    assert!(!ascii.is_valid(&[0x80]));

    let input = b"plain";
    match ascii.transform(None, input, OpReplaceNoErr).unwrap() {
        TransformResult::Borrowed(output) => assert!(std::ptr::eq(output.as_ptr(), input.as_ptr())),
        TransformResult::Owned(_) => panic!("valid ASCII must return the original input"),
    }
    assert_eq!(
        ascii
            .transform(None, "a中b".as_bytes(), OpReplaceNoErr)
            .unwrap()
            .as_slice(),
        b"a?b"
    );

    let mut chunks = Vec::new();
    ascii.foreach("a中b".as_bytes(), OpReplaceNoErr, |from, to, ok| {
        chunks.push((from.to_vec(), to.to_vec(), ok));
        chunks.len() < 2
    });
    assert_eq!(
        chunks,
        vec![
            (b"a".to_vec(), b"a".to_vec(), true),
            ("中".as_bytes().to_vec(), "中".as_bytes().to_vec(), false),
        ]
    );
}
