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

use parser_charset::*;

/// Go's UTF-8 fast path returns `src` directly and leaves the caller's buffer untouched.
#[test]
fn utf8_valid_transform_preserves_destination_buffer() {
    for encoding in [FindEncoding(CharsetUTF8MB4), EncodingUTF8MB3StrictImpl()] {
        let mut destination = b"caller-owned-capacity".to_vec();
        let output = encoding
            .Transform(&mut destination, "合法 UTF-8".as_bytes(), OpEncode)
            .unwrap();

        assert_eq!(output, "合法 UTF-8".as_bytes());
        assert_eq!(destination, b"caller-owned-capacity");
    }
}
