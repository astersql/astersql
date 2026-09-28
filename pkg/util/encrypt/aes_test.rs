// Copyright 2026 AsterSQL.
// Copyright 2017 PingCAP, Inc.
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

// AES 加解密单元测试：对齐 Go `aes_test.go` 的向量与错误分支。
//
// 覆盖 PKCS7 填充/去填充、ECB/CBC/OFB/CTR/CFB 表驱动加解密，以及 MySQL 密钥派生。

// 本文件对照 pkg/util/encrypt/aes_test.go，保留 Go 测试结构与全部测试向量。
// 这段逻辑覆盖 PKCS7 padding、ECB/CBC/OFB/CTR/CFB 加解密表驱动用例和 MySQL key 派生，
// 测试直接调用 Rust 加密实现，并验证成功结果和错误分支。

use ::aes::{Aes128, Aes192, Aes256};
use cipher::{BlockDecrypt, BlockEncrypt, KeyInit, generic_array::GenericArray};
use util_encrypt::aes::*;

// to_hex 对应 Go 的 toHex，按大写十六进制输出字节。
fn to_hex(buf: &[u8]) -> String {
    buf.iter()
        .map(|b| format!("{:02X}", b))
        .collect::<Vec<_>>()
        .join("")
}

// TestPad 对应 Go 的 TestPad，校验 PKCS7Pad 在 8/16 字节块上的补齐内容。
#[test]
fn test_pad() {
    let pad_cases = vec![
        (vec![0x0A, 0x0B, 0x0C, 0x0D], 8_usize, "0A0B0C0D04040404"),
        (
            vec![0x0A, 0x0B, 0x0C, 0x0D, 0x0A, 0x0B, 0x0C, 0x0D],
            8_usize,
            "0A0B0C0D0A0B0C0D0808080808080808",
        ),
        (
            vec![0x0A, 0x0B, 0x0C, 0x0D],
            16_usize,
            "0A0B0C0D0C0C0C0C0C0C0C0C0C0C0C0C",
        ),
    ];

    for (input, block_size, expect) in pad_cases {
        // GO: p, err := PKCS7Pad(p, blockSize); require.NoError(t, err)
        let padded = PKCS7Pad(&input, block_size);
        assert_eq!(expect, to_hex(&padded.unwrap()));
    }
}

// TestUnpad 对应 Go 的 TestUnpad。
// 前半段验证合法 padding，后半段保留块大小、padding 长度和 padding 内容错误分支。
#[test]
fn test_unpad() {
    let valid_cases = vec![
        (
            vec![0x0A, 0x0B, 0x0C, 0x0D, 0x04, 0x04, 0x04, 0x04],
            8_usize,
            "0A0B0C0D",
        ),
        (
            vec![
                0x0A, 0x0B, 0x0C, 0x0D, 0x0A, 0x0B, 0x0C, 0x0D, 0x08, 0x08, 0x08, 0x08, 0x08, 0x08,
                0x08, 0x08,
            ],
            8_usize,
            "0A0B0C0D0A0B0C0D",
        ),
        (
            vec![
                0x0A, 0x0B, 0x0C, 0x0D, 0x0C, 0x0C, 0x0C, 0x0C, 0x0C, 0x0C, 0x0C, 0x0C, 0x0C, 0x0C,
                0x0C, 0x0C,
            ],
            16_usize,
            "0A0B0C0D",
        ),
        (
            vec![0x08, 0x08, 0x08, 0x08, 0x08, 0x08, 0x08, 0x08],
            8_usize,
            "",
        ),
    ];
    for (input, block_size, expect) in valid_cases {
        let unpadded = PKCS7Unpad(&input, block_size);
        assert_eq!(expect, to_hex(&unpadded.unwrap()));
    }

    let invalid_cases = vec![
        vec![0x0A, 0x0B, 0x0C, 0x04, 0x04, 0x04, 0x04],
        vec![0x0A, 0x0B, 0x0C, 0x02, 0x03, 0x04, 0x04, 0x04, 0x04],
        vec![],
        vec![
            0x0C, 0x0C, 0x0C, 0x0C, 0x0C, 0x0C, 0x0C, 0x09, 0x09, 0x09, 0x09, 0x09, 0x09, 0x09,
            0x09, 0x09,
        ],
        vec![0x0C, 0x0C, 0x0C, 0x0C, 0x0C, 0x0C, 0x0C, 0x00],
        vec![
            0x0A, 0x0B, 0x0C, 0x0D, 0x0A, 0x0B, 0x0C, 0x0D, 0x04, 0x08, 0x08, 0x08, 0x08, 0x08,
            0x08, 0x08,
        ],
        vec![0x03, 0x08, 0x08, 0x08, 0x08, 0x08, 0x08, 0x08],
        vec![0x0A, 0x0B, 0x0C, 0x0D, 0x04, 0x04, 0x03, 0x04],
    ];
    for input in invalid_cases {
        // GO: require.Error(t, err)
        assert!(PKCS7Unpad(&input, 8).is_err());
    }
}

// TestAESECB 对应 Go 的 NIST SP 800-38A ECB 测试向量。
// 这里保留三种 key 长度的输入/输出向量，真实 aes.NewCipher 与 CryptBlocks 仅作为迁移形状。
#[test]
fn test_aes_ecb() {
    let common_input = vec![
        0x6b, 0xc1, 0xbe, 0xe2, 0x2e, 0x40, 0x9f, 0x96, 0xe9, 0x3d, 0x7e, 0x11, 0x73, 0x93, 0x17,
        0x2a, 0xae, 0x2d, 0x8a, 0x57, 0x1e, 0x03, 0xac, 0x9c, 0x9e, 0xb7, 0x6f, 0xac, 0x45, 0xaf,
        0x8e, 0x51, 0x30, 0xc8, 0x1c, 0x46, 0xa3, 0x5c, 0xe4, 0x11, 0xe5, 0xfb, 0xc1, 0x19, 0x1a,
        0x0a, 0x52, 0xef, 0xf6, 0x9f, 0x24, 0x45, 0xdf, 0x4f, 0x9b, 0x17, 0xad, 0x2b, 0x41, 0x7b,
        0xe6, 0x6c, 0x37, 0x10,
    ];
    let common_key128 = vec![
        0x2b, 0x7e, 0x15, 0x16, 0x28, 0xae, 0xd2, 0xa6, 0xab, 0xf7, 0x15, 0x88, 0x09, 0xcf, 0x4f,
        0x3c,
    ];
    let common_key192 = vec![
        0x8e, 0x73, 0xb0, 0xf7, 0xda, 0x0e, 0x64, 0x52, 0xc8, 0x10, 0xf3, 0x2b, 0x80, 0x90, 0x79,
        0xe5, 0x62, 0xf8, 0xea, 0xd2, 0x52, 0x2c, 0x6b, 0x7b,
    ];
    let common_key256 = vec![
        0x60, 0x3d, 0xeb, 0x10, 0x15, 0xca, 0x71, 0xbe, 0x2b, 0x73, 0xae, 0xf0, 0x85, 0x7d, 0x77,
        0x81, 0x1f, 0x35, 0x2c, 0x07, 0x3b, 0x61, 0x08, 0xd7, 0x2d, 0x98, 0x10, 0xa3, 0x09, 0x14,
        0xdf, 0xf4,
    ];
    let ecb_aes_tests = vec![
        (
            "ECB-AES128",
            common_key128,
            common_input.clone(),
            vec![
                0x3a, 0xd7, 0x7b, 0xb4, 0x0d, 0x7a, 0x36, 0x60, 0xa8, 0x9e, 0xca, 0xf3, 0x24, 0x66,
                0xef, 0x97, 0xf5, 0xd3, 0xd5, 0x85, 0x03, 0xb9, 0x69, 0x9d, 0xe7, 0x85, 0x89, 0x5a,
                0x96, 0xfd, 0xba, 0xaf, 0x43, 0xb1, 0xcd, 0x7f, 0x59, 0x8e, 0xce, 0x23, 0x88, 0x1b,
                0x00, 0xe3, 0xed, 0x03, 0x06, 0x88, 0x7b, 0x0c, 0x78, 0x5e, 0x27, 0xe8, 0xad, 0x3f,
                0x82, 0x23, 0x20, 0x71, 0x04, 0x72, 0x5d, 0xd4,
            ],
        ),
        (
            "ECB-AES192",
            common_key192,
            common_input.clone(),
            vec![
                0xbd, 0x33, 0x4f, 0x1d, 0x6e, 0x45, 0xf2, 0x5f, 0xf7, 0x12, 0xa2, 0x14, 0x57, 0x1f,
                0xa5, 0xcc, 0x97, 0x41, 0x04, 0x84, 0x6d, 0x0a, 0xd3, 0xad, 0x77, 0x34, 0xec, 0xb3,
                0xec, 0xee, 0x4e, 0xef, 0xef, 0x7a, 0xfd, 0x22, 0x70, 0xe2, 0xe6, 0x0a, 0xdc, 0xe0,
                0xba, 0x2f, 0xac, 0xe6, 0x44, 0x4e, 0x9a, 0x4b, 0x41, 0xba, 0x73, 0x8d, 0x6c, 0x72,
                0xfb, 0x16, 0x69, 0x16, 0x03, 0xc1, 0x8e, 0x0e,
            ],
        ),
        (
            "ECB-AES256",
            common_key256,
            common_input.clone(),
            vec![
                0xf3, 0xee, 0xd1, 0xbd, 0xb5, 0xd2, 0xa0, 0x3c, 0x06, 0x4b, 0x5a, 0x7e, 0x3d, 0xb1,
                0x81, 0xf8, 0x59, 0x1c, 0xcb, 0x10, 0xd4, 0x10, 0xed, 0x26, 0xdc, 0x5b, 0xa7, 0x4a,
                0x31, 0x36, 0x28, 0x70, 0xb6, 0xed, 0x21, 0xb9, 0x9c, 0xa6, 0xf4, 0xf9, 0xf1, 0x53,
                0xe7, 0xb1, 0xbe, 0xaf, 0xed, 0x1d, 0x23, 0x30, 0x4b, 0x7a, 0x39, 0xf9, 0xf3, 0xff,
                0x06, 0x7d, 0x8d, 0x8f, 0x9e, 0x24, 0xec, 0xc7,
            ],
        ),
    ];

    for (test, key, input, output) in ecb_aes_tests {
        let mut encrypted = input.clone();
        macro_rules! crypt {
            ($cipher:ty) => {{
                let cipher = <$cipher>::new_from_slice(&key).unwrap();
                for block in encrypted.chunks_exact_mut(16) {
                    cipher.encrypt_block(GenericArray::from_mut_slice(block));
                }
                assert_eq!(output, encrypted, "{test}: ECB encrypt");
                for block in encrypted.chunks_exact_mut(16) {
                    cipher.decrypt_block(GenericArray::from_mut_slice(block));
                }
            }};
        }
        match key.len() {
            16 => crypt!(Aes128),
            24 => crypt!(Aes192),
            32 => crypt!(Aes256),
            _ => panic!("{test}: invalid key length"),
        }
        assert_eq!(input, encrypted, "{test}: ECB decrypt");
    }
}

struct AesEncryptCase {
    str_: &'static str,
    key: &'static str,
    iv: &'static str,
    expect: &'static str,
    is_error: bool,
}

struct AesDecryptCase {
    str_: &'static str,
    key: &'static str,
    iv: &'static str,
    expect: &'static str,
    is_error: bool,
}

// assert_encrypt_cases 对应 Go 中多个 AESEncryptWith* 表驱动循环。
// mode_name/function_go 记录原测试调用的模式名和 Go 函数名，便于后续接线核对。
fn assert_encrypt_cases(mode_name: &str, function_go: &str, tests: Vec<AesEncryptCase>) {
    for tt in tests {
        let result = match mode_name {
            "ECB" => AESEncryptWithECB(tt.str_.as_bytes(), tt.key.as_bytes()),
            "CBC" => AESEncryptWithCBC(tt.str_.as_bytes(), tt.key.as_bytes(), tt.iv.as_bytes()),
            "OFB" => AESEncryptWithOFB(tt.str_.as_bytes(), tt.key.as_bytes(), tt.iv.as_bytes()),
            "CTR" => AESEncryptWithCTR(tt.str_.as_bytes(), tt.key.as_bytes(), tt.iv.as_bytes()),
            "CFB" => AESEncryptWithCFB(tt.str_.as_bytes(), tt.key.as_bytes(), tt.iv.as_bytes()),
            _ => panic!("unknown AES mode {mode_name}"),
        };
        if tt.is_error {
            assert!(
                result.is_err(),
                "{function_go} should reject key length {}",
                tt.key.len()
            );
        } else {
            assert_eq!(tt.expect, to_hex(&result.unwrap()), "{function_go}");
        }
    }
}

// assert_decrypt_cases 对应 Go 中多个 AESDecryptWith* 表驱动循环。
fn assert_decrypt_cases(mode_name: &str, function_go: &str, tests: Vec<AesDecryptCase>) {
    for tt in tests {
        let encrypted = decode_hex(tt.str_).unwrap_or_else(|| tt.str_.as_bytes().to_vec());
        let result = match mode_name {
            "ECB" => AESDecryptWithECB(&encrypted, tt.key.as_bytes()),
            "CBC" => AESDecryptWithCBC(&encrypted, tt.key.as_bytes(), tt.iv.as_bytes()),
            "OFB" => AESDecryptWithOFB(&encrypted, tt.key.as_bytes(), tt.iv.as_bytes()),
            "CTR" => AESDecryptWithCTR(&encrypted, tt.key.as_bytes(), tt.iv.as_bytes()),
            "CFB" => AESDecryptWithCFB(&encrypted, tt.key.as_bytes(), tt.iv.as_bytes()),
            _ => panic!("unknown AES mode {mode_name}"),
        };
        if tt.is_error {
            assert!(
                result.is_err(),
                "{function_go} should reject case {}",
                tt.str_
            );
        } else {
            assert_eq!(tt.expect.as_bytes(), result.unwrap(), "{function_go}");
        }
    }
}

fn decode_hex(value: &str) -> Option<Vec<u8>> {
    if value.len() % 2 != 0 {
        return None;
    }
    (0..value.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&value[i..i + 2], 16).ok())
        .collect()
}

// TestAESEncryptWithECB 对应 Go 的 ECB 加密表。
#[test]
fn test_aes_encrypt_with_ecb() {
    let tests = vec![
        AesEncryptCase {
            str_: "pingcap",
            key: "1234567890123456",
            iv: "",
            expect: "697BFE9B3F8C2F289DD82C88C7BC95C4",
            is_error: false,
        },
        AesEncryptCase {
            str_: "pingcap123",
            key: "1234567890123456",
            iv: "",
            expect: "CEC348F4EF5F84D3AA6C4FA184C65766",
            is_error: false,
        },
        AesEncryptCase {
            str_: "pingcap",
            key: "123456789012345678901234",
            iv: "",
            expect: "E435438AC6798B4718533096436EC342",
            is_error: false,
        },
        AesEncryptCase {
            str_: "pingcap",
            key: "12345678901234567",
            iv: "",
            expect: "",
            is_error: true,
        },
        AesEncryptCase {
            str_: "pingcap",
            key: "123456789012345",
            iv: "",
            expect: "",
            is_error: true,
        },
    ];
    assert_encrypt_cases("ECB", "AESEncryptWithECB", tests);
}

// TestAESDecryptWithECB 对应 Go 的 ECB 解密表，额外包含 invalid padding / padding size 用例。
#[test]
fn test_aes_decrypt_with_ecb() {
    let tests = vec![
        AesDecryptCase {
            str_: "697BFE9B3F8C2F289DD82C88C7BC95C4",
            key: "1234567890123456",
            iv: "",
            expect: "pingcap",
            is_error: false,
        },
        AesDecryptCase {
            str_: "CEC348F4EF5F84D3AA6C4FA184C65766",
            key: "1234567890123456",
            iv: "",
            expect: "pingcap123",
            is_error: false,
        },
        AesDecryptCase {
            str_: "E435438AC6798B4718533096436EC342",
            key: "123456789012345678901234",
            iv: "",
            expect: "pingcap",
            is_error: false,
        },
        AesDecryptCase {
            str_: "",
            key: "12345678901234567",
            iv: "",
            expect: "pingcap",
            is_error: true,
        },
        AesDecryptCase {
            str_: "",
            key: "123456789012345",
            iv: "",
            expect: "pingcap",
            is_error: true,
        },
        AesDecryptCase {
            str_: "11223344556677112233",
            key: "1234567890123456",
            iv: "",
            expect: "",
            is_error: true,
        },
        AesDecryptCase {
            str_: "11223344556677112233112233445566",
            key: "1234567890123456",
            iv: "",
            expect: "",
            is_error: true,
        },
        AesDecryptCase {
            str_: "1122334455667711223311223344556611",
            key: "1234567890123456",
            iv: "",
            expect: "",
            is_error: true,
        },
    ];
    assert_decrypt_cases("ECB", "AESDecryptWithECB", tests);
}

// TestAESEncryptWithCBC 对应 Go 的 CBC 加密表。
#[test]
fn test_aes_encrypt_with_cbc() {
    let tests = vec![
        AesEncryptCase {
            str_: "pingcap",
            key: "1234567890123456",
            iv: "1234567890123456",
            expect: "2ECA0077C5EA5768A0485AA522774792",
            is_error: false,
        },
        AesEncryptCase {
            str_: "pingcap123",
            key: "1234567890123456",
            iv: "1234567890123456",
            expect: "042962D340F2F95BCC07B56EAC378D3A",
            is_error: false,
        },
        AesEncryptCase {
            str_: "pingcap",
            key: "123456789012345678901234",
            iv: "1234567890123456",
            expect: "EDECE05D9FE662E381130F7F19BA67F7",
            is_error: false,
        },
        AesEncryptCase {
            str_: "pingcap",
            key: "12345678901234567",
            iv: "1234567890123456",
            expect: "",
            is_error: true,
        },
        AesEncryptCase {
            str_: "pingcap",
            key: "123456789012345",
            iv: "1234567890123456",
            expect: "",
            is_error: true,
        },
    ];
    assert_encrypt_cases("CBC", "AESEncryptWithCBC", tests);
}

// TestAESEncryptWithOFB 与 TestAESEncryptWithCTR/CFB 共用同一批 Go 向量。
fn stream_encrypt_cases() -> Vec<AesEncryptCase> {
    vec![
        AesEncryptCase {
            str_: "pingcap",
            key: "1234567890123456",
            iv: "1234567890123456",
            expect: "0515A36BBF3DE0",
            is_error: false,
        },
        AesEncryptCase {
            str_: "pingcap123",
            key: "1234567890123456",
            iv: "1234567890123456",
            expect: "0515A36BBF3DE0DBE9DD",
            is_error: false,
        },
        AesEncryptCase {
            str_: "pingcap",
            key: "123456789012345678901234",
            iv: "1234567890123456",
            expect: "45A57592449893",
            is_error: false,
        },
        AesEncryptCase {
            str_: "pingcap",
            key: "12345678901234567",
            iv: "1234567890123456",
            expect: "",
            is_error: true,
        },
        AesEncryptCase {
            str_: "pingcap",
            key: "123456789012345",
            iv: "1234567890123456",
            expect: "",
            is_error: true,
        },
    ]
}

// stream_decrypt_cases 对应 OFB/CTR/CFB 解密测试的相同 Go 向量。
fn stream_decrypt_cases() -> Vec<AesDecryptCase> {
    vec![
        AesDecryptCase {
            str_: "0515A36BBF3DE0",
            key: "1234567890123456",
            iv: "1234567890123456",
            expect: "pingcap",
            is_error: false,
        },
        AesDecryptCase {
            str_: "0515A36BBF3DE0DBE9DD",
            key: "1234567890123456",
            iv: "1234567890123456",
            expect: "pingcap123",
            is_error: false,
        },
        AesDecryptCase {
            str_: "45A57592449893",
            key: "123456789012345678901234",
            iv: "1234567890123456",
            expect: "pingcap",
            is_error: false,
        },
        AesDecryptCase {
            str_: "pingcap",
            key: "12345678901234567",
            iv: "1234567890123456",
            expect: "",
            is_error: true,
        },
        AesDecryptCase {
            str_: "pingcap",
            key: "123456789012345",
            iv: "1234567890123456",
            expect: "",
            is_error: true,
        },
    ]
}

#[test]
fn test_aes_encrypt_with_ofb() {
    assert_encrypt_cases("OFB", "AESEncryptWithOFB", stream_encrypt_cases());
}

#[test]
fn test_aes_decrypt_with_ofb() {
    assert_decrypt_cases("OFB", "AESDecryptWithOFB", stream_decrypt_cases());
}

#[test]
fn test_aes_encrypt_with_ctr() {
    assert_encrypt_cases("CTR", "AESEncryptWithCTR", stream_encrypt_cases());
}

#[test]
fn test_aes_decrypt_with_ctr() {
    assert_decrypt_cases("CTR", "AESDecryptWithCTR", stream_decrypt_cases());
}

// TestAESDecryptWithCBC 对应 Go 的 CBC 解密表。
#[test]
fn test_aes_decrypt_with_cbc() {
    let tests = vec![
        AesDecryptCase {
            str_: "2ECA0077C5EA5768A0485AA522774792",
            key: "1234567890123456",
            iv: "1234567890123456",
            expect: "pingcap",
            is_error: false,
        },
        AesDecryptCase {
            str_: "042962D340F2F95BCC07B56EAC378D3A",
            key: "1234567890123456",
            iv: "1234567890123456",
            expect: "pingcap123",
            is_error: false,
        },
        AesDecryptCase {
            str_: "EDECE05D9FE662E381130F7F19BA67F7",
            key: "123456789012345678901234",
            iv: "1234567890123456",
            expect: "pingcap",
            is_error: false,
        },
        AesDecryptCase {
            str_: "",
            key: "12345678901234567",
            iv: "1234567890123456",
            expect: "pingcap",
            is_error: true,
        },
        AesDecryptCase {
            str_: "",
            key: "123456789012345",
            iv: "1234567890123456",
            expect: "pingcap",
            is_error: true,
        },
        AesDecryptCase {
            str_: "11223344556677112233",
            key: "1234567890123456",
            iv: "1234567890123456",
            expect: "",
            is_error: true,
        },
        AesDecryptCase {
            str_: "11223344556677112233112233445566",
            key: "1234567890123456",
            iv: "1234567890123456",
            expect: "",
            is_error: true,
        },
        AesDecryptCase {
            str_: "1122334455667711223311223344556611",
            key: "1234567890123456",
            iv: "1234567890123456",
            expect: "",
            is_error: true,
        },
    ];
    assert_decrypt_cases("CBC", "AESDecryptWithCBC", tests);
}

#[test]
fn test_aes_encrypt_with_cfb() {
    assert_encrypt_cases("CFB", "AESEncryptWithCFB", stream_encrypt_cases());
}

#[test]
fn test_aes_decrypt_with_cfb() {
    assert_decrypt_cases("CFB", "AESDecryptWithCFB", stream_decrypt_cases());
}

// TestDeriveKeyMySQL 对应 Go 的 MySQL AES key 派生兼容测试。
#[test]
fn test_derive_key_mysql() {
    let cases = vec![
        (
            "MySQL=insecure! MySQL=insecure! ".as_bytes().to_vec(),
            "00000000000000000000000000000000",
        ),
        (
            vec![0xC0, 0x10, 0x44, 0xCC, 0x10, 0xD9],
            "C01044CC10D900000000000000000000",
        ),
        (
            "MySecretVeryLooooongPassword".as_bytes().to_vec(),
            "22163D0233131607210A001D4C6F6F6F",
        ),
    ];
    for (password, expect) in cases {
        // GO: p = DeriveKeyMySQL(p, 16); require.Equal(t, expect, toHex(p))
        let derived = DeriveKeyMySQL(&password, 16);
        assert_eq!(expect, to_hex(&derived));
    }
}
