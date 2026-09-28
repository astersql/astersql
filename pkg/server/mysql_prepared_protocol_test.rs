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

// MySQL 预处理语句二进制协议的端到端测试。
//
// 测试通过真实监听端口手工构造 COM_STMT_EXECUTE 载荷，覆盖参数元数据、
// 长数据分片、NULL 位图、类型缓存，以及语句关闭后的错误响应。

use crate::mysql_compat_test_support::{
    CLIENT_DEPRECATE_EOF, MysqlCompatServer, TextValue, WireResponse, put_lenenc_bytes,
};

// 将只允许成功响应的协议步骤收敛为统一断言，失败时保留调用场景和完整响应。
fn expect_ok(response: WireResponse, context: &str) -> crate::mysql_compat_test_support::OkPacket {
    let WireResponse::Ok(ok) = response else {
        panic!("{context} did not return OK: {response:?}");
    };
    ok
}

fn execute_payload(
    statement_id: u32,
    null_bitmap: u8,
    new_types: bool,
    types: &[(u8, bool)],
    values: Vec<u8>,
) -> Vec<u8> {
    // COM_STMT_EXECUTE 依次携带语句 ID、游标标志、迭代次数、NULL 位图和参数值；
    // 只有 new-params-bound 标志置位时才发送本轮参数的类型与无符号标记。
    let mut payload = statement_id.to_le_bytes().to_vec();
    payload.push(0);
    payload.extend_from_slice(&1_u32.to_le_bytes());
    payload.push(null_bitmap);
    payload.push(u8::from(new_types));
    if new_types {
        for (tp, unsigned) in types {
            payload.extend_from_slice(&[*tp, if *unsigned { 0x80 } else { 0 }]);
        }
    }
    payload.extend_from_slice(&values);
    payload
}

// 按 INSERT 占位符顺序编码二进制参数值；由长数据缓冲区或 NULL 位图提供的字段
// 不写入值区，日期和时间则使用协议规定的长度前缀结构。
fn insert_values(
    signed: i64,
    unsigned: u64,
    decimal: &[u8],
    text: &[u8],
    inline_blob: Option<&[u8]>,
    date: (u16, u8, u8),
    time: (u32, u8, u8, u8, u32),
    nullable: Option<&[u8]>,
) -> Vec<u8> {
    let mut values = Vec::new();
    values.extend_from_slice(&signed.to_le_bytes());
    values.extend_from_slice(&unsigned.to_le_bytes());
    put_lenenc_bytes(&mut values, decimal);
    put_lenenc_bytes(&mut values, text);
    if let Some(blob) = inline_blob {
        put_lenenc_bytes(&mut values, blob);
    }
    values.push(4);
    values.extend_from_slice(&date.0.to_le_bytes());
    values.extend_from_slice(&[date.1, date.2]);
    values.push(12);
    values.push(0);
    values.extend_from_slice(&time.0.to_le_bytes());
    values.extend_from_slice(&[time.1, time.2, time.3]);
    values.extend_from_slice(&time.4.to_le_bytes());
    if let Some(nullable) = nullable {
        put_lenenc_bytes(&mut values, nullable);
    }
    values
}

#[test]
fn mysql_prepared_statements_execute_real_sql_with_binary_values() {
    // 类型码同时覆盖有符号/无符号整数、十进制、字符串、二进制、日期和时间。
    const TYPES: [(u8, bool); 8] = [
        (0x08, false),
        (0x08, true),
        (0xf6, false),
        (0xfd, false),
        (0xfc, false),
        (0x0a, false),
        (0x0b, false),
        (0xfd, false),
    ];

    let server = MysqlCompatServer::start().expect("start real MySQL listener");
    let mut client = server
        .connect_root(CLIENT_DEPRECATE_EOF, None)
        .expect("connect to real MySQL listener");
    for sql in [
        "create database prepared_wire",
        "use prepared_wire",
        "create table prepared_values (\
         id bigint unsigned auto_increment primary key,\
         signed_v bigint not null,\
         unsigned_v bigint unsigned not null,\
         decimal_v decimal(20,6) not null,\
         text_v varchar(32) not null,\
         blob_v blob not null,\
         date_v date not null,\
         time_v time(6) not null,\
         nullable_v varchar(16) null)",
    ] {
        expect_ok(client.query(sql).expect("send setup SQL"), sql);
    }

    let insert = client
        .prepare(
            "insert into prepared_values \
             (signed_v,unsigned_v,decimal_v,text_v,blob_v,date_v,time_v,nullable_v) \
             values (?,?,?,?,?,?,?,?)",
        )
        .expect("prepare parameterized INSERT");
    assert_eq!(insert.parameter_count, 8);
    assert!(insert.columns.is_empty());

    // 第 5 个参数通过多个 COM_STMT_SEND_LONG_DATA 包拼接，特意包含 NUL 和非 UTF-8 字节；
    // 第 8 个参数由 NULL 位图标记，因此两者都不出现在本轮内联值区。
    client
        .send_long_data(insert.statement_id, 4, b"A\0")
        .expect("send first long-data chunk");
    client
        .send_long_data(insert.statement_id, 4, &[0xff, b'B'])
        .expect("send second long-data chunk");
    let first_insert = execute_payload(
        insert.statement_id,
        1 << 7,
        true,
        &TYPES,
        insert_values(
            -7,
            u64::MAX,
            b"12345678901234.123456",
            "预处理".as_bytes(),
            None,
            (2024, 2, 29),
            (1, 2, 3, 4, 567_890),
            None,
        ),
    );
    let ok = expect_ok(
        client
            .execute_prepared(&first_insert)
            .expect("execute prepared INSERT"),
        "prepared INSERT",
    );
    assert_eq!(ok.affected_rows, 1);
    assert_eq!(ok.last_insert_id, 1);

    // RESET 必须丢弃尚未执行的长数据，但保留已绑定参数类型；第二次执行因而既能改用
    // 内联 BLOB，又能在 new-params-bound 为 false 时复用首次执行的类型表。
    client
        .send_long_data(insert.statement_id, 4, b"discarded")
        .expect("bind data before reset");
    expect_ok(
        client
            .reset_prepared(insert.statement_id)
            .expect("COM_STMT_RESET"),
        "COM_STMT_RESET",
    );
    let second_insert = execute_payload(
        insert.statement_id,
        0,
        false,
        &[],
        insert_values(
            -8,
            42,
            b"9.500000",
            b"reuse",
            Some(b"inline\0blob"),
            (2025, 1, 2),
            (0, 3, 4, 5, 6),
            Some(b"present"),
        ),
    );
    let ok = expect_ok(
        client
            .execute_prepared(&second_insert)
            .expect("reuse prepared parameter types"),
        "second prepared INSERT",
    );
    assert_eq!(ok.affected_rows, 1);
    assert_eq!(ok.last_insert_id, 2);

    let select = client
        .prepare(
            "select id,signed_v,unsigned_v,decimal_v,text_v,blob_v,date_v,time_v,nullable_v \
             from prepared_values where id = ?",
        )
        .expect("prepare parameterized SELECT");
    assert_eq!(select.parameter_count, 1);
    assert_eq!(select.columns.len(), 9);
    assert_eq!(select.columns[0].name, "id");
    assert_eq!(select.columns[0].column_type, 0x08);

    // SELECT 的结果走二进制行协议，验证极值、Unicode、原始字节及时间归一化均无损。
    let mut id = Vec::new();
    id.extend_from_slice(&1_u64.to_le_bytes());
    let select_first = execute_payload(select.statement_id, 0, true, &[(0x08, true)], id);
    let response = client
        .execute_prepared(&select_first)
        .expect("execute binary SELECT");
    let WireResponse::ResultSet(result) = response else {
        panic!("prepared SELECT did not return a result set: {response:?}");
    };
    assert_eq!(
        result.rows,
        vec![vec![
            TextValue::Bytes(b"1".to_vec()),
            TextValue::Bytes(b"-7".to_vec()),
            TextValue::Bytes(u64::MAX.to_string().into_bytes()),
            TextValue::Bytes(b"12345678901234.123456".to_vec()),
            TextValue::Bytes("预处理".as_bytes().to_vec()),
            TextValue::Bytes(vec![b'A', 0, 0xff, b'B']),
            TextValue::Bytes(b"2024-02-29".to_vec()),
            TextValue::Bytes(b"26:03:04.567890".to_vec()),
            TextValue::Null,
        ]]
    );

    // 再次省略参数类型，确认同一 SELECT 语句可复用已缓存的无符号 BIGINT 类型。
    let mut reused_id = select.statement_id.to_le_bytes().to_vec();
    reused_id.push(0);
    reused_id.extend_from_slice(&1_u32.to_le_bytes());
    reused_id.extend_from_slice(&[0, 0]);
    reused_id.extend_from_slice(&2_u64.to_le_bytes());
    let WireResponse::ResultSet(reused) = client
        .execute_prepared(&reused_id)
        .expect("execute with reused parameter type")
    else {
        panic!("reused prepared SELECT did not return a result set");
    };
    assert_eq!(
        reused.rows[0][5],
        TextValue::Bytes(b"inline\0blob".to_vec())
    );
    assert_eq!(reused.rows[0][8], TextValue::Bytes(b"present".to_vec()));

    // COM_STMT_CLOSE 无响应包；后续执行同一 ID 应由服务端返回语句不存在错误。
    client
        .close_prepared(select.statement_id)
        .expect("COM_STMT_CLOSE");
    let WireResponse::Err(error) = client
        .execute_prepared(&reused_id)
        .expect("closed statement returns ERR")
    else {
        panic!("closed prepared statement did not return ERR");
    };
    assert!(error.message.contains("not found"), "{error:?}");
}
