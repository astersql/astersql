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

//! 对应 Go `misc_test.go`：验证 TruncateTS / EncodeKeyPrefix 边界与分组语义。
//! 纯字节夹具，不依赖 TiKV/PD；断言依据与 Go 用例输出字节序列一致。

//! Go-equivalent tests for `br/pkg/restore/utils/misc_test.go`.

use crate::{EncodeKeyPrefix, TruncateTS};

/// 长键剥掉末尾 8 字节 TS；短于 8 字节的键保持不变。
/// TestTruncateTS — long key drops trailing 8-byte TS; short key unchanged.
#[test]
fn test_truncate_ts() {
    // 16 字节：后 8 字节视为 TS，前缀应剩 8 字节。
    let key_with_ts = vec![
        b'1', b'2', b'1', b'2', b'1', b'2', b'1', b'2', b'1', b'2', b'1', b'2', b'1', b'2', b'1',
        b'2',
    ];
    let ts = TruncateTS(&key_with_ts);
    assert_eq!(
        ts,
        Some(vec![b'1', b'2', b'1', b'2', b'1', b'2', b'1', b'2'])
    );

    // 不足 8 字节：不截断，原样返回。
    let key_with_ts = vec![b'1', b'2'];
    let ts = TruncateTS(&key_with_ts);
    assert_eq!(ts, Some(vec![b'1', b'2']));
}

/// 每满 8 字节组后插 0xff；不足一组的尾巴保持原始字节。
/// TestEncodeKeyPrefix — append 0xff after each 8-byte group; short tail kept raw.
#[test]
fn test_encode_key_prefix() {
    // 恰好两组 8 字节：两组各带 0xff 后缀。
    let key_prefix = vec![
        b'1', b'2', b'1', b'2', b'1', b'2', b'1', b'2', b'1', b'2', b'1', b'2', b'1', b'2', b'1',
        b'2',
    ];
    let encode_key = EncodeKeyPrefix(&key_prefix);
    assert_eq!(
        encode_key,
        vec![
            b'1', b'2', b'1', b'2', b'1', b'2', b'1', b'2', 0xff, b'1', b'2', b'1', b'2', b'1',
            b'2', b'1', b'2', 0xff
        ]
    );

    // 15 字节：第一组完整+0xff，尾巴 7 字节原样。
    let key_prefix = vec![
        b'1', b'2', b'1', b'2', b'1', b'2', b'1', b'2', b'1', b'2', b'1', b'2', b'1', b'2', b'1',
    ];
    let encode_key = EncodeKeyPrefix(&key_prefix);
    assert_eq!(
        encode_key,
        vec![
            b'1', b'2', b'1', b'2', b'1', b'2', b'1', b'2', 0xff, b'1', b'2', b'1', b'2', b'1',
            b'2', b'1'
        ]
    );

    // 仅 2 字节：无完整组，输出即输入。
    let key_prefix = vec![b'1', b'2'];
    let encode_key = EncodeKeyPrefix(&key_prefix);
    assert_eq!(encode_key, vec![b'1', b'2']);
}
