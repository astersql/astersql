// Copyright 2015 PingCAP, Inc.
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

// Bytes 编解码的表驱动单元测试。
//
// 对应 Go `pkg/util/codec/bytes_test.go`：覆盖快速/安全反转一致性、
// 正序/倒序 memcomparable 向量，以及 `EncodeBytesExt` 的 RawKV 分支。

// 本文件由 pkg/util/codec/bytes_test.go 迁移而来，保留 Go 表驱动测试结构。
//

use super::*;

// TestFastSlowFastReverse 对应 Go 的 unaligned 快速反转与普通反转一致性测试。
/// 在支持非对齐架构上验证 fastReverse 与 reverse 往返一致。
#[test]
pub fn TestFastSlowFastReverse() {
    if !supportsUnaligned {
        return;
    }
    let mut b = vec![1, 2, 3, 4, 5, 6, 7, 8, 255, 0, 0, 0, 0, 0, 0, 0, 0, 247];
    let r1 = b.clone();
    fastReverseBytes(&mut b);
    let mut r2 = b;
    reverseBytes(&mut r2);
    assert_eq!(r1, r2);
}

// BytesCodecCase 对应 Go TestBytesCodec 中的匿名 struct。
/// 单条 bytes 编解码用例：明文、期望编码、是否降序。
struct BytesCodecCase {
    enc: Vec<u8>,
    dec: Vec<u8>,
    desc: bool,
}

// TestBytesCodec 对应 Go 的主表驱动测试：覆盖正序/倒序编码长度、编码结果、解码结果和错误输入。
/// 表驱动验证正序/倒序编码长度、结果、解码与畸形输入拒绝。
#[test]
pub fn TestBytesCodec() {
    let inputs = vec![
        BytesCodecCase {
            enc: vec![],
            dec: vec![0, 0, 0, 0, 0, 0, 0, 0, 247],
            desc: false,
        },
        BytesCodecCase {
            enc: vec![],
            dec: vec![255, 255, 255, 255, 255, 255, 255, 255, 8],
            desc: true,
        },
        BytesCodecCase {
            enc: vec![0],
            dec: vec![0, 0, 0, 0, 0, 0, 0, 0, 248],
            desc: false,
        },
        BytesCodecCase {
            enc: vec![0],
            dec: vec![255, 255, 255, 255, 255, 255, 255, 255, 7],
            desc: true,
        },
        BytesCodecCase {
            enc: vec![1, 2, 3],
            dec: vec![1, 2, 3, 0, 0, 0, 0, 0, 250],
            desc: false,
        },
        BytesCodecCase {
            enc: vec![1, 2, 3],
            dec: vec![254, 253, 252, 255, 255, 255, 255, 255, 5],
            desc: true,
        },
        BytesCodecCase {
            enc: vec![1, 2, 3, 0],
            dec: vec![1, 2, 3, 0, 0, 0, 0, 0, 251],
            desc: false,
        },
        BytesCodecCase {
            enc: vec![1, 2, 3, 0],
            dec: vec![254, 253, 252, 255, 255, 255, 255, 255, 4],
            desc: true,
        },
        BytesCodecCase {
            enc: vec![1, 2, 3, 4, 5, 6, 7],
            dec: vec![1, 2, 3, 4, 5, 6, 7, 0, 254],
            desc: false,
        },
        BytesCodecCase {
            enc: vec![1, 2, 3, 4, 5, 6, 7],
            dec: vec![254, 253, 252, 251, 250, 249, 248, 255, 1],
            desc: true,
        },
        BytesCodecCase {
            enc: vec![0, 0, 0, 0, 0, 0, 0, 0],
            dec: vec![0, 0, 0, 0, 0, 0, 0, 0, 255, 0, 0, 0, 0, 0, 0, 0, 0, 247],
            desc: false,
        },
        BytesCodecCase {
            enc: vec![0, 0, 0, 0, 0, 0, 0, 0],
            dec: vec![
                255, 255, 255, 255, 255, 255, 255, 255, 0, 255, 255, 255, 255, 255, 255, 255, 255,
                8,
            ],
            desc: true,
        },
        BytesCodecCase {
            enc: vec![1, 2, 3, 4, 5, 6, 7, 8],
            dec: vec![1, 2, 3, 4, 5, 6, 7, 8, 255, 0, 0, 0, 0, 0, 0, 0, 0, 247],
            desc: false,
        },
        BytesCodecCase {
            enc: vec![1, 2, 3, 4, 5, 6, 7, 8],
            dec: vec![
                254, 253, 252, 251, 250, 249, 248, 247, 0, 255, 255, 255, 255, 255, 255, 255, 255,
                8,
            ],
            desc: true,
        },
        BytesCodecCase {
            enc: vec![1, 2, 3, 4, 5, 6, 7, 8, 9],
            dec: vec![1, 2, 3, 4, 5, 6, 7, 8, 255, 9, 0, 0, 0, 0, 0, 0, 0, 248],
            desc: false,
        },
        BytesCodecCase {
            enc: vec![1, 2, 3, 4, 5, 6, 7, 8, 9],
            dec: vec![
                254, 253, 252, 251, 250, 249, 248, 247, 0, 246, 255, 255, 255, 255, 255, 255, 255,
                7,
            ],
            desc: true,
        },
    ];

    for input in inputs {
        assert_eq!(input.dec.len(), EncodedBytesLength(input.enc.len()));
        if input.desc {
            let b = EncodeBytesDesc(Vec::new(), &input.enc);
            assert_eq!(input.dec, b);
            let (remain, decoded) = DecodeBytesDesc(&b, None).expect("valid descending bytes");
            assert!(remain.is_empty());
            assert_eq!(input.enc, decoded);
        } else {
            let b = EncodeBytes(Vec::new(), &input.enc);
            assert_eq!(input.dec, b);
            let (remain, decoded) = DecodeBytes(&b, None).expect("valid ascending bytes");
            assert!(remain.is_empty());
            assert_eq!(input.enc, decoded);
        }
    }

    let errInputs = vec![
        vec![1, 2, 3, 4],
        vec![0, 0, 0, 0, 0, 0, 0, 247],
        vec![0, 0, 0, 0, 0, 0, 0, 0, 246],
        vec![0, 0, 0, 0, 0, 0, 0, 1, 247],
        vec![1, 2, 3, 4, 5, 6, 7, 8, 0],
        vec![1, 2, 3, 4, 5, 6, 7, 8, 255, 1],
        vec![1, 2, 3, 4, 5, 6, 7, 8, 255, 1, 2, 3, 4, 5, 6, 7, 8],
        vec![1, 2, 3, 4, 5, 6, 7, 8, 255, 1, 2, 3, 4, 5, 6, 7, 8, 255],
        vec![1, 2, 3, 4, 5, 6, 7, 8, 255, 1, 2, 3, 4, 5, 6, 7, 8, 0],
    ];

    for input in errInputs {
        assert!(
            DecodeBytes(&input, None).is_err(),
            "input should be rejected: {input:?}"
        );
    }
}

// BytesCodecExtCase 对应 Go TestBytesCodecExt 中的匿名 struct。
/// `EncodeBytesExt` 用例：明文与 memcomparable 期望值。
struct BytesCodecExtCase {
    enc: Vec<u8>,
    dec: Vec<u8>,
}

// TestBytesCodecExt 对应 Go 的 EncodeBytesExt 测试，专门处理 []byte{} 与 nil 切片的比较。
/// 验证 RawKV 直接追加与非 RawKV 走 EncodeBytes 的分支。
#[test]
pub fn TestBytesCodecExt() {
    let inputs = vec![
        BytesCodecExtCase {
            enc: vec![],
            dec: vec![0, 0, 0, 0, 0, 0, 0, 0, 247],
        },
        BytesCodecExtCase {
            enc: vec![1, 2, 3],
            dec: vec![1, 2, 3, 0, 0, 0, 0, 0, 250],
        },
        BytesCodecExtCase {
            enc: vec![1, 2, 3, 4, 5, 6, 7, 8, 9],
            dec: vec![1, 2, 3, 4, 5, 6, 7, 8, 255, 9, 0, 0, 0, 0, 0, 0, 0, 248],
        },
    ];

    for input in inputs {
        assert_eq!(input.enc, EncodeBytesExt(Vec::new(), &input.enc, true));
        assert_eq!(input.dec, EncodeBytesExt(Vec::new(), &input.enc, false));
    }
}
