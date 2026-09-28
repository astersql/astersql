// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
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

// parser/util 迁移对照单元测试，验证与 Go 行为逐字节/逐方法对齐。
//
// 对全部 u8 输入核对 UnescapeChar，并用录音式哈希器确认 IHasher 方法面与变更协议。

// parser/util 迁移期 Aster 单元测试。
//
// 全字节对照 Go 的 UnescapeChar，并校验 IHasher 方法面与 Reset/Sum64 协议。

use super::{IHasher, UnescapeChar};

/// 复刻 Go UnescapeChar 分支，作为 Rust 实现的期望基准。
fn go_unescape_char(input: u8) -> Vec<u8> {
    match input {
        b'n' => vec![b'\n'],
        b'0' => vec![0],
        b'b' => vec![8],
        b'Z' => vec![26],
        b'r' => vec![b'\r'],
        b't' => vec![b'\t'],
        b'%' | b'_' => vec![b'\\', input],
        _ => vec![input],
    }
}

/// 对全部 256 个字节断言 UnescapeChar 与 Go 基准一致。
#[test]
fn unescape_char_matches_go_for_every_byte() {
    // 穷举每个字节，确保默认分支与 LIKE 通配符分支均与 Go 一致。
    for input in u8::MIN..=u8::MAX {
        assert_eq!(
            go_unescape_char(input),
            UnescapeChar(input),
            "input byte {input:#04x}"
        );
    }
}

/// 记录调用序列的测试用 Hasher，用于核对 IHasher 方法面。
/// 测试用 IHasher：把每次 Hash* 调用格式化为字符串记入 calls，Sum64 返回调用次数。
#[derive(Default)]
struct RecordingHasher {
    calls: Vec<String>,
}

impl IHasher for RecordingHasher {
    fn HashBool(&mut self, val: bool) {
        self.calls.push(format!("bool:{val}"));
    }

    fn HashInt(&mut self, val: isize) {
        self.calls.push(format!("int:{val}"));
    }

    fn HashInt64(&mut self, val: i64) {
        self.calls.push(format!("int64:{val}"));
    }

    fn HashUint64(&mut self, val: u64) {
        self.calls.push(format!("uint64:{val}"));
    }

    fn HashFloat64(&mut self, val: f64) {
        self.calls.push(format!("float64:{:016x}", val.to_bits()));
    }

    fn HashRune(&mut self, val: i32) {
        self.calls.push(format!("rune:{val}"));
    }

    fn HashString(&mut self, val: &str) {
        self.calls.push(format!("string:{val}"));
    }

    fn HashByte(&mut self, val: u8) {
        self.calls.push(format!("byte:{val}"));
    }

    fn HashBytes(&mut self, val: &[u8]) {
        self.calls.push(format!("bytes:{val:?}"));
    }

    fn Reset(&mut self) {
        self.calls.clear();
    }

    fn Sum64(&self) -> u64 {
        self.calls.len() as u64
    }
}

/// 校验 IHasher 各 Hash* 调用顺序、Sum64 计数与 Reset 清空。
/// 经 dyn IHasher 调用全部 Hash*，确认方法面存在且 Reset 清空、Sum64 反映调用数。
#[test]
fn ihasher_preserves_go_method_surface_and_mutation_protocol() {
    let mut concrete = RecordingHasher::default();
    let hasher: &mut dyn IHasher = &mut concrete;

    // 覆盖 Go Hasher 接口中的各类写入入口（含边界值与多字节字符串）。
    // 经 dyn IHasher 依次写入各类型，再 Reset 核对摘要归零。
    hasher.HashBool(true);
    hasher.HashInt(isize::MIN);
    hasher.HashInt64(i64::MIN);
    hasher.HashUint64(u64::MAX);
    hasher.HashFloat64(-0.0);
    hasher.HashRune('中' as i32);
    hasher.HashString("TiDB中");
    hasher.HashByte(0xff);
    hasher.HashBytes(&[0, 1, 0xff]);

    assert_eq!(9, hasher.Sum64());
    hasher.Reset();
    assert_eq!(0, hasher.Sum64());
}
