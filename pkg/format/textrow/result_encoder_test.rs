// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
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

// ResultEncoder 与 IsStringColumnType 的单元测试。
//
// 分别覆盖 utf-8（noop）、gbk 转码、binary 透传，以及字符串列类型枚举。

use super::{IsStringColumnType, NewResultEncoder, mysql};

// TestResultEncoder 对应 Go 测试：分别覆盖 utf-8、gbk 与 binary 三种元数据编码路径。
#[test]
fn TestResultEncoder() {
    // utf-8 在 Go 实现中作为 noop 编码，EncodeMeta 应直接返回原始字节。
    let mut d = NewResultEncoder("utf-8");
    let src = b"test_string".to_vec();
    let result = d.EncodeMeta(&src);
    assert_eq!(src, result);

    // gbk 分支验证中文“一”会被转换为 MySQL 兼容的 GBK 字节序列。
    d = NewResultEncoder("gbk");
    let result = d.EncodeMeta("一".as_bytes());
    assert_eq!(vec![0xd2, 0xbb], result);

    // binary 分支不做字符集转换，仍应保持 UTF-8 字符串字节。
    d = NewResultEncoder("binary");
    let result = d.EncodeMeta("一".as_bytes());
    assert_eq!("一", String::from_utf8_lossy(&result));
}

// TestIsStringColumnType 对应 Go 测试：列举所有应被视为字符串列的 MySQL 类型。
#[test]
fn TestIsStringColumnType() {
    let string_types: Vec<u8> = vec![
        mysql::TypeString,
        mysql::TypeVarString,
        mysql::TypeVarchar,
        mysql::TypeBit,
        mysql::TypeTinyBlob,
        mysql::TypeMediumBlob,
        mysql::TypeLongBlob,
        mysql::TypeBlob,
        mysql::TypeEnum,
        mysql::TypeSet,
        mysql::TypeJSON,
        mysql::TypeTiDBVectorFloat32,
    ];

    for tp in string_types {
        // 每个类型单独带上 type 值，保留 Go require.True 的诊断信息。
        assert!(IsStringColumnType(tp), "type {tp}");
    }

    // Longlong 是数值类型，不能进入字符串列字符集覆盖路径。
    assert!(!IsStringColumnType(mysql::TypeLonglong));
}
