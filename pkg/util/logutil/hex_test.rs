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

// `hex` 模块单元测试。
//
// 覆盖 Region 类消息的 `Hex` 渲染，以及字节 / 嵌套消息的 `PrettyPrint`
// 小写十六进制输出，对齐 Go 侧期望字符串。

use super::hex::{Hex, ProtoField, ProtoValue};
use super::main_test::PrettyPrint;

/// 构造含 StartKey/EndKey 的 Region 消息，断言 `Hex` 输出与 Go 一致。
#[test]
fn TestHex() {
    // 模拟 PD/TiKV Region 元数据字段布局
    let region = ProtoValue::message(vec![
        ProtoField::new("Id", 6662_u64),
        ProtoField::new(
            "StartKey",
            vec![
                b't', 200, b'\\', 0, 0, 0, b'\\', 0, 0, 0, 37, b'-', 0, 0, 0, 0, 0, 0, 0, 37,
            ],
        ),
        ProtoField::new("EndKey", b"3asg3asd".as_slice()),
        ProtoField::new("RegionEpoch", ProtoValue::Nil),
        ProtoField::new("Peers", ProtoValue::List(vec![])),
        ProtoField::new("EncryptionMeta", ProtoValue::Nil),
        ProtoField::new("IsInFlashback", false),
        ProtoField::new("FlashbackStartTs", 0_u64),
    ]);

    let expected = "{Id:6662 StartKey:74c85c0000005c000000252d0000000000000025 EndKey:3361736733617364 RegionEpoch:<nil> Peers:[] EncryptionMeta:<nil> IsInFlashback:false FlashbackStartTs:0}";
    assert_eq!(expected, Hex(&region).to_string());
}

/// 断言字节切片与含空 EndKey 的消息经 PrettyPrint 得到正确 hex。
#[test]
fn TestPrettyPrint() {
    let byte_slice = "asd2fsdafs中文3af".as_bytes();
    let rendered = PrettyPrint(&ProtoValue::Bytes(byte_slice.to_vec()));
    assert_eq!("61736432667364616673e4b8ade69687336166", rendered);
    assert_eq!(encode_hex(byte_slice), rendered);

    // Go reflect cannot distinguish uint8 from byte, so both use the byte path.
    // 中文补充：Go reflect 无法区分 uint8 与 byte，二者均走字节 hex 路径。
    let int_slice = vec![1_u8, 2, 3, b'a', b'b', b'c', b'\''];
    assert_eq!("01020361626327", PrettyPrint(&ProtoValue::Bytes(int_slice)));

    let key_range = ProtoValue::message(vec![
        ProtoField::new("StartKey", b"_txxey23_i263".as_slice()),
        ProtoField::new("EndKey", Vec::<u8>::new()),
    ]);
    assert_eq!(
        "{StartKey:5f747878657932335f69323633 EndKey:}",
        PrettyPrint(&key_range)
    );
}

/// Go 按原始结构体字段索引决定是否写分隔空格；即使首个 XXX 字段被跳过，
/// 后续可见字段仍保留一个前导空格。
#[test]
fn test_skipped_leading_xxx_field_preserves_go_spacing() {
    let message = ProtoValue::message(vec![
        ProtoField::new("XXX_Internal", 1_u64),
        ProtoField::new("Id", 7_u64),
    ]);

    assert_eq!("{ Id:7}", PrettyPrint(&message));
}

/// 参考实现：逐字节格式化为两位小写 hex，用于对照 PrettyPrint。
fn encode_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
