// Copyright 2026 AsterSQL.
// Copyright 2018 PingCAP, Inc.
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

// 列元数据到 Protobuf 转换的单元测试。
//
// 验证 `ColumnToProto` / `ColumnsToProto` 对 flag、排序规则（collation）、
// ENUM 元素、数组列以及 TiFlash 生成列标记的保留与映射行为。

use util_dependency::misc::{ColumnMetadata, ColumnToProto, ColumnsToProto, ProtoColumnInfo};

/// TiFlash 侧标识虚拟生成列的 flag 位（第 23 位）。
const GENERATED_COLUMN_FLAG: i32 = 1 << 23;

/// 测试用列元数据，实现 `ColumnMetadata` trait。
#[derive(Clone, Debug)]
struct TestColumn {
    id: i64,
    tp: i32,
    flag: i32,
    collation: i32,
    column_len: i32,
    decimal: i32,
    elems: Vec<String>,
    array_element_type: i32,
    array: bool,
    virtual_generated: bool,
    primary_key: bool,
}

impl ColumnMetadata for TestColumn {
    fn id(&self) -> i64 {
        self.id
    }
    fn collation_id(&self) -> i32 {
        self.collation
    }
    fn column_len(&self) -> i32 {
        self.column_len
    }
    fn decimal(&self) -> i32 {
        self.decimal
    }
    fn flags(&self) -> i32 {
        self.flag
    }
    fn elements(&self) -> Vec<String> {
        self.elems.clone()
    }
    fn field_type(&self) -> i32 {
        self.tp
    }
    fn array_element_type(&self) -> i32 {
        self.array_element_type
    }
    fn is_array(&self) -> bool {
        self.array
    }
    fn is_virtual_generated(&self) -> bool {
        self.virtual_generated
    }
    fn is_primary_key(&self) -> bool {
        self.primary_key
    }
}

/// 默认测试列：整型、主键、utf8 排序规则。
fn column() -> TestColumn {
    TestColumn {
        id: 1,
        tp: 3,
        flag: 10,
        collation: 83,
        column_len: 11,
        decimal: 0,
        elems: Vec::new(),
        array_element_type: 3,
        array: false,
        virtual_generated: false,
        primary_key: true,
    }
}

/// 验证 flag/collation/列长、PkHandle、新旧 collation、ENUM elems 与数组列映射。
#[test]
fn test_column_to_proto_preserves_flags_collations_enum_and_array_metadata() {
    let long_utf8 = column();
    let proto = ColumnToProto(&long_utf8, false, false);
    assert_eq!(
        proto,
        ProtoColumnInfo {
            ColumnId: 1,
            Tp: 3,
            Collation: 83,
            ColumnLen: 11,
            Decimal: 0,
            Flag: 10,
            Elems: Vec::new(),
            PkHandle: false,
        }
    );

    for pk_is_handle in [false, true] {
        let protos = ColumnsToProto(
            &[long_utf8.clone(), long_utf8.clone()],
            pk_is_handle,
            false,
            false,
        );
        assert_eq!(protos.len(), 2);
        for proto in protos {
            assert_eq!(proto.Flag, 10);
            assert_eq!(proto.PkHandle, pk_is_handle);
        }
    }

    let latin1 = TestColumn {
        collation: 8,
        ..column()
    };
    assert_eq!(ColumnToProto(&latin1, false, false).Collation, 8);
    let new_collation_utf8 = TestColumn {
        collation: -83,
        ..column()
    };
    let new_collation_latin1 = TestColumn {
        collation: -8,
        ..latin1
    };
    assert_eq!(
        ColumnToProto(&new_collation_utf8, false, false).Collation,
        -83
    );
    assert_eq!(
        ColumnToProto(&new_collation_latin1, false, false).Collation,
        -8
    );

    let enum_column = TestColumn {
        tp: 247,
        elems: vec!["a".to_owned(), "b".to_owned()],
        ..column()
    };
    assert_eq!(ColumnToProto(&enum_column, false, false).Elems, ["a", "b"]);

    // 数组字符串列：开启数组映射时 collation 会变为 binary(63)。
    let array_string = TestColumn {
        tp: 254,
        array_element_type: 254,
        collation: 46,
        column_len: 100,
        array: true,
        ..column()
    };
    let proto = ColumnToProto(&array_string, true, false);
    assert_eq!(proto.Tp, 254);
    assert_eq!(proto.Collation, 63);
    assert_eq!(proto.ColumnLen, 100);
}

/// TiFlash 路径下虚拟生成列应打上 GENERATED_COLUMN_FLAG；非 TiFlash 则不打。
#[test]
fn test_generated_column_flag_for_tiflash() {
    let generated = TestColumn {
        virtual_generated: true,
        ..column()
    };
    let normal = TestColumn {
        id: 2,
        primary_key: false,
        ..column()
    };

    assert_eq!(
        ColumnToProto(&generated, false, true).Flag & GENERATED_COLUMN_FLAG,
        GENERATED_COLUMN_FLAG
    );
    assert_eq!(
        ColumnToProto(&generated, false, false).Flag & GENERATED_COLUMN_FLAG,
        0
    );
    assert_eq!(
        ColumnToProto(&normal, false, true).Flag & GENERATED_COLUMN_FLAG,
        0
    );
    let protos = ColumnsToProto(&[generated, normal], false, false, true);
    assert_ne!(protos[0].Flag & GENERATED_COLUMN_FLAG, 0);
    assert_eq!(protos[1].Flag & GENERATED_COLUMN_FLAG, 0);
}
