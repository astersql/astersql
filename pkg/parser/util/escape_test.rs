// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     http://www.apache.org/licenses/LICENSE-2.0
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// UnescapeChar 与 IHasher 状态更新的单元测试。
//
// 对照 Go 的 `escape_test.go`：覆盖 MySQL 反斜杠转义各分支，
// 并用可记录状态的测试哈希器验证 IHasher 写入与 Reset 协议。

// MySQL 反斜杠转义与 IHasher 接口的单元测试。
//
// 覆盖 `UnescapeChar` 各转义分支（含 LIKE 通配符），以及 `IHasher`
// 各 Hash* 方法的状态累积、Sum64 与 Reset 语义。

// 校验内存中的字节转义结果。
// TestUnescapeChar 对应 Go 的同名测试函数，逐项覆盖 MySQL 反斜杠后的单字节转义。
use super::{IHasher, UnescapeChar};

/// 对照 Go TestUnescapeChar：逐项断言反斜杠后单字节的还原结果。
#[test]
fn test_unescape_char() {
    /// 单组输入字节与期望输出。
    struct TestCase {
        input: u8,
        want: Vec<u8>,
    }

    let tests = vec![
        // 标准单字节转义：Go 期望返回换行、NUL、退格、Ctrl-Z、回车和制表符。
        TestCase {
            input: b'n',
            want: vec![b'\n'],
        },
        TestCase {
            input: b'0',
            want: vec![0],
        },
        TestCase {
            input: b'b',
            want: vec![8],
        },
        TestCase {
            input: b'Z',
            want: vec![26],
        },
        TestCase {
            input: b'r',
            want: vec![b'\r'],
        },
        TestCase {
            input: b't',
            want: vec![b'\t'],
        },
        // LIKE 通配符需要保留反斜杠和原字符，不能走默认的“去掉反斜杠”路径。
        TestCase {
            input: b'%',
            want: vec![b'\\', b'%'],
        },
        TestCase {
            input: b'_',
            want: vec![b'\\', b'_'],
        },
        // 自转义字符：反斜杠本身、单引号和双引号只返回字符自身。
        TestCase {
            input: b'\\',
            want: vec![b'\\'],
        },
        TestCase {
            input: b'\'',
            want: vec![b'\''],
        },
        TestCase {
            input: b'"',
            want: vec![b'"'],
        },
        // 其他字符沿用 Go 默认分支：删除前导反斜杠，只保留当前字节。
        TestCase {
            input: b'a',
            want: vec![b'a'],
        },
        TestCase {
            input: b'z',
            want: vec![b'z'],
        },
        TestCase {
            input: b'1',
            want: vec![b'1'],
        },
        TestCase {
            input: b' ',
            want: vec![b' '],
        },
    ];

    for tt in tests {
        let got = UnescapeChar(tt.input);
        assert_eq!(tt.want, got, "UnescapeChar({:?})", tt.input as char);
    }
}

/// 测试用 IHasher：按调用顺序把各 Hash* 写入累积到 bytes，Sum64 做 FNV 风格折叠。
#[derive(Default)]
struct StateHasher {
    bytes: Vec<u8>,
}

impl IHasher for StateHasher {
    fn HashBool(&mut self, val: bool) {
        self.bytes.push(u8::from(val));
    }

    fn HashInt(&mut self, val: isize) {
        self.bytes.extend_from_slice(&val.to_le_bytes());
    }

    fn HashInt64(&mut self, val: i64) {
        self.bytes.extend_from_slice(&val.to_le_bytes());
    }

    fn HashUint64(&mut self, val: u64) {
        self.bytes.extend_from_slice(&val.to_le_bytes());
    }

    fn HashFloat64(&mut self, val: f64) {
        self.bytes.extend_from_slice(&val.to_bits().to_le_bytes());
    }

    fn HashRune(&mut self, val: i32) {
        self.bytes.extend_from_slice(&val.to_le_bytes());
    }

    fn HashString(&mut self, val: &str) {
        self.bytes.extend_from_slice(val.as_bytes());
    }

    fn HashByte(&mut self, val: u8) {
        self.bytes.push(val);
    }

    fn HashBytes(&mut self, val: &[u8]) {
        self.bytes.extend_from_slice(val);
    }

    fn Reset(&mut self) {
        self.bytes.clear();
    }

    fn Sum64(&self) -> u64 {
        self.bytes.iter().fold(0_u64, |sum, byte| {
            sum.wrapping_mul(1099511628211)
                .wrapping_add(u64::from(*byte))
        })
    }
}

/// 校验 IHasher 各类型写入、Sum64 非零以及 Reset 清空状态。
/// 校验各 Hash* 按序写入、Sum64 非零，以及 Reset 清空状态。
#[test]
fn test_ihasher_state_updates_and_reset() {
    let mut hasher = StateHasher::default();
    let mut expected = Vec::new();

    // 依次写入多种类型，expected 用同一编码规则手工拼出对照字节。
    hasher.HashBool(true);
    expected.push(1);
    hasher.HashInt(-2);
    expected.extend_from_slice(&(-2_isize).to_le_bytes());
    hasher.HashInt64(i64::MIN);
    expected.extend_from_slice(&i64::MIN.to_le_bytes());
    hasher.HashUint64(u64::MAX);
    expected.extend_from_slice(&u64::MAX.to_le_bytes());
    hasher.HashFloat64(-0.0);
    expected.extend_from_slice(&(-0.0_f64).to_bits().to_le_bytes());
    hasher.HashRune(-1);
    expected.extend_from_slice(&(-1_i32).to_le_bytes());
    hasher.HashString("中");
    expected.extend_from_slice("中".as_bytes());
    hasher.HashByte(0x80);
    expected.push(0x80);
    hasher.HashBytes(&[0, 0xff]);
    expected.extend_from_slice(&[0, 0xff]);

    assert_eq!(expected, hasher.bytes);
    assert_ne!(0, hasher.Sum64());
    hasher.Reset();
    assert!(hasher.bytes.is_empty());
    assert_eq!(0, hasher.Sum64());
}
