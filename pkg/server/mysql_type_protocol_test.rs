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

// MySQL 类型协议的端到端回归测试。
//
// 测试通过真实 TCP 监听器执行建表、写入和查询，既校验文本协议返回的原始字节，
// 也校验列定义包中的类型码、标志位、字符集和小数精度，防止服务端元数据与实际值不一致。

use crate::mysql_compat_test_support::{
    CLIENT_DEPRECATE_EOF, ColumnDefinition, MysqlCompatServer, TextValue, WireResponse,
};
use astersql_parser_mysql::r#type as mysql;

// 建库、切库和写入都必须走 OK 包；夹具阶段若收到结果集或错误包，应立即失败。
fn expect_ok(response: WireResponse, context: &str) {
    assert!(
        matches!(response, WireResponse::Ok(_)),
        "{context} did not return OK: {response:?}",
    );
}

// 按列名定位元数据，使后续断言不依赖列在结果集中的固定下标。
fn column<'a>(columns: &'a [ColumnDefinition], name: &str) -> &'a ColumnDefinition {
    columns
        .iter()
        .find(|column| column.name == name)
        .unwrap_or_else(|| panic!("column {name} missing from {columns:?}"))
}

#[test]
fn mysql_type_packets_expose_correct_type_flags_charset_and_decimal() {
    let server = MysqlCompatServer::start().expect("start real MySQL listener");
    let mut client = server
        .connect_root(CLIENT_DEPRECATE_EOF, None)
        .expect("connect to real MySQL listener");
    // 夹具同时覆盖无符号整数上界、多字节字符、内嵌 NUL、时间小数秒和复合类型。
    for sql in [
        "create database type_wire",
        "use type_wire",
        "create table type_packets (\
         id bigint unsigned not null primary key,\
         dec_v decimal(20,6) null,\
         char_v char(4) null,\
         varchar_v varchar(16) null,\
         binary_v binary(4) null,\
         varbinary_v varbinary(8) null,\
         blob_v blob null,\
         datetime_v datetime(6) null,\
         timestamp_v timestamp(3) null,\
         json_v json null,\
         enum_v enum('red','blue') null,\
         set_v set('a','b') null)",
        "insert into type_packets values (\
         18446744073709551615, 12345678901234.123456, 'é', '空',\
         X'41004200', X'410042', X'41004200',\
         '2024-02-29 12:34:56.123456', '2024-02-29 12:34:56.123',\
         '{\"n\":1}', 'blue', 'a,b')",
    ] {
        expect_ok(
            client
                .query(sql)
                .unwrap_or_else(|error| panic!("send fixture statement {sql}: {error}")),
            sql,
        );
    }

    // 文本协议应保持各类型的规范文本或原始字节表示，尤其不能截断二进制字段中的 NUL。
    let WireResponse::ResultSet(result) = client
        .query("select * from type_packets")
        .expect("read type values and metadata over real TCP")
    else {
        panic!("type SELECT did not return a result set");
    };
    assert_eq!(result.rows.len(), 1);
    assert_eq!(
        result.rows[0],
        vec![
            TextValue::Bytes(b"18446744073709551615".to_vec()),
            TextValue::Bytes(b"12345678901234.123456".to_vec()),
            TextValue::Bytes("é".as_bytes().to_vec()),
            TextValue::Bytes("空".as_bytes().to_vec()),
            TextValue::Bytes(b"A\0B\0".to_vec()),
            TextValue::Bytes(b"A\0B".to_vec()),
            TextValue::Bytes(b"A\0B\0".to_vec()),
            TextValue::Bytes(b"2024-02-29 12:34:56.123456".to_vec()),
            TextValue::Bytes(b"2024-02-29 12:34:56.123".to_vec()),
            TextValue::Bytes(b"{\"n\": 1}".to_vec()),
            TextValue::Bytes(b"blue".to_vec()),
            TextValue::Bytes(b"a,b".to_vec()),
        ]
    );

    // 整数列的类型码和约束标志来自列定义包，而非根据返回值推断。
    let id = column(&result.columns, "id");
    assert_eq!(id.column_type, mysql::TypeLonglong);
    assert_ne!(id.flags & mysql::UnsignedFlag as u16, 0);
    assert_ne!(id.flags & mysql::NotNullFlag as u16, 0);
    assert_ne!(id.flags & mysql::PriKeyFlag as u16, 0);

    // DECIMAL 的显示宽度还要包含符号位和小数点，精度信息必须与 DDL 中的 scale 一致。
    let decimal = column(&result.columns, "dec_v");
    assert_eq!(decimal.column_type, mysql::TypeNewDecimal);
    assert_eq!(decimal.decimals, 6);
    assert_eq!(decimal.column_length, 22);

    assert_eq!(
        column(&result.columns, "char_v").column_type,
        mysql::TypeString
    );
    assert_eq!(
        column(&result.columns, "varchar_v").column_type,
        mysql::TypeVarString
    );
    // BINARY、VARBINARY 与 BLOB 共享二进制字符集和 BinaryFlag，避免被客户端按文本解码。
    for name in ["binary_v", "varbinary_v", "blob_v"] {
        let binary = column(&result.columns, name);
        assert_eq!(
            binary.character_set,
            u16::from(astersql_parser_mysql::charset::BinaryDefaultCollationID),
            "binary charset for {name}"
        );
        assert_ne!(
            binary.flags & mysql::BinaryFlag as u16,
            0,
            "binary flag for {name}"
        );
    }
    // 时间类型分别携带自身类型码和小数秒位数，不能统一退化为字符串元数据。
    assert_eq!(
        column(&result.columns, "datetime_v").column_type,
        mysql::TypeDatetime
    );
    assert_eq!(column(&result.columns, "datetime_v").decimals, 6);
    assert_eq!(
        column(&result.columns, "timestamp_v").column_type,
        mysql::TypeTimestamp
    );
    assert_eq!(column(&result.columns, "timestamp_v").decimals, 3);
    assert_eq!(
        column(&result.columns, "json_v").column_type,
        mysql::TypeJSON
    );
    // ENUM/SET 在线路上使用字符串类型码，并依靠专用标志保留逻辑类型身份。
    let enum_v = column(&result.columns, "enum_v");
    assert_eq!(enum_v.column_type, mysql::TypeString);
    assert_ne!(enum_v.flags & mysql::EnumFlag as u16, 0);
    let set_v = column(&result.columns, "set_v");
    assert_eq!(set_v.column_type, mysql::TypeString);
    assert_ne!(set_v.flags & mysql::SetFlag as u16, 0);
}
