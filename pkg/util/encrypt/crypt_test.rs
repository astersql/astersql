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

// MySQL SQLEncode/SQLDecode 单元测试：对齐 Go `crypt_test.go` 的向量。
//
// 表驱动覆盖多语言明文、空密码与互逆往返；期望密文以大写十六进制表示。

// 本文件对照 pkg/util/encrypt/crypt_test.go，保留 Go 测试结构与全部测试向量。
// SQLDecode/SQLEncode 的表驱动测试，包含多语言字符串和密码用例，
// 测试直接调用 SQLDecode/SQLEncode，覆盖多语言输入和互逆行为。

use util_encrypt::crypt::{SQLDecode, SQLEncode};

/// 将字节切片格式化为大写十六进制字符串，便于与 Go 期望值比较。
fn to_hex(buf: &[u8]) -> String {
    buf.iter().map(|b| format!("{b:02X}")).collect()
}

/// 单条 SQL 编解码用例：明文、密码、期望结果与是否期望错误。
struct SqlCryptCase {
    str_: &'static str,
    passwd: &'static str,
    expect: &'static str,
    is_error: bool,
}

// TestSQLDecode 对应 Go 的 TestSQLDecode。
// 每个 case 保留明文、密码和期望密文字节的大写十六进制表示。
#[test]
fn test_sql_decode() {
    let tests = vec![
        SqlCryptCase {
            str_: "",
            passwd: "",
            expect: "",
            is_error: false,
        },
        SqlCryptCase {
            str_: "pingcap",
            passwd: "1234567890123456",
            expect: "2C35B5A4ADF391",
            is_error: false,
        },
        SqlCryptCase {
            str_: "pingcap",
            passwd: "asdfjasfwefjfjkj",
            expect: "351CC412605905",
            is_error: false,
        },
        SqlCryptCase {
            str_: "pingcap123",
            passwd: "123456789012345678901234",
            expect: "7698723DC6DFE7724221",
            is_error: false,
        },
        SqlCryptCase {
            str_: "pingcap#%$%^",
            passwd: "*^%YTu1234567",
            expect: "8634B9C55FF55E5B6328F449",
            is_error: false,
        },
        SqlCryptCase {
            str_: "pingcap",
            passwd: "",
            expect: "4A77B524BD2C5C",
            is_error: false,
        },
        SqlCryptCase {
            str_: "分布式データベース",
            passwd: "pass1234@#$%%^^&",
            expect: "80CADC8D328B3026D04FB285F36FED04BBCA0CC685BF78B1E687CE",
            is_error: false,
        },
        SqlCryptCase {
            str_: "分布式データベース",
            passwd: "分布式7782734adgwy1242",
            expect: "0E24CFEF272EE32B6E0BFBDB89F29FB43B4B30DAA95C3F914444BC",
            is_error: false,
        },
        SqlCryptCase {
            str_: "pingcap",
            passwd: "密匙",
            expect: "CE5C02A5010010",
            is_error: false,
        },
        SqlCryptCase {
            str_: "pingcap数据库",
            passwd: "数据库passwd12345667",
            expect: "36D5F90D3834E30E396BE3226E3B4ED3",
            is_error: false,
        },
    ];

    for tt in tests {
        let result = SQLDecode(tt.str_.as_bytes(), tt.passwd.as_bytes());
        assert_eq!(tt.is_error, result.is_err());
        let crypted = result.unwrap();
        assert_eq!(tt.expect, to_hex(&crypted));
    }
}

// TestSQLEncode 对应 Go 的 TestSQLEncode。
// Go 先 SQLDecode 生成密文，再 SQLEncode 回明文，验证编码/解码表互逆。
#[test]
fn test_sql_encode() {
    let tests = vec![
        SqlCryptCase {
            str_: "",
            passwd: "",
            expect: "",
            is_error: false,
        },
        SqlCryptCase {
            str_: "pingcap",
            passwd: "1234567890123456",
            expect: "pingcap",
            is_error: false,
        },
        SqlCryptCase {
            str_: "pingcap",
            passwd: "asdfjasfwefjfjkj",
            expect: "pingcap",
            is_error: false,
        },
        SqlCryptCase {
            str_: "pingcap123",
            passwd: "123456789012345678901234",
            expect: "pingcap123",
            is_error: false,
        },
        SqlCryptCase {
            str_: "pingcap#%$%^",
            passwd: "*^%YTu1234567",
            expect: "pingcap#%$%^",
            is_error: false,
        },
        SqlCryptCase {
            str_: "pingcap",
            passwd: "",
            expect: "pingcap",
            is_error: false,
        },
        SqlCryptCase {
            str_: "分布式データベース",
            passwd: "pass1234@#$%%^^&",
            expect: "分布式データベース",
            is_error: false,
        },
        SqlCryptCase {
            str_: "分布式データベース",
            passwd: "分布式7782734adgwy1242",
            expect: "分布式データベース",
            is_error: false,
        },
        SqlCryptCase {
            str_: "pingcap",
            passwd: "密匙",
            expect: "pingcap",
            is_error: false,
        },
        SqlCryptCase {
            str_: "pingcap数据库",
            passwd: "数据库passwd12345667",
            expect: "pingcap数据库",
            is_error: false,
        },
    ];

    for tt in tests {
        let crypted = SQLDecode(tt.str_.as_bytes(), tt.passwd.as_bytes()).unwrap();
        let result = SQLEncode(&crypted, tt.passwd.as_bytes());
        assert_eq!(tt.is_error, result.is_err());
        let uncrypted = result.unwrap();
        assert_eq!(tt.expect.as_bytes(), uncrypted);
    }
}
