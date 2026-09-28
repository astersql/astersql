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

// MySQL 常用类型的端到端兼容性回归测试。
//
// 通过真实会话建表、写入并查询类型矩阵，同时核对字段类型元数据与文本化结果，
// 防止类型映射正确但取值失真，或值可读但元数据退化。

use crate::runtime::{ConcreteRecordSet, ConcreteSession, CreateAnalyzeSession};
use crate::testutil::TestRecordSet;

// 执行测试准备语句，并在失败信息中保留原始 SQL，便于定位具体的类型处理阶段。
fn execute(session: &ConcreteSession, sql: &str) {
    session
        .execute(sql)
        .unwrap_or_else(|error| panic!("type compatibility setup failed: {sql}: {error}"));
}

// 验证常见 MySQL 类型从建表、写入到结果集读取均保留元数据和值语义。
#[test]
fn common_mysql_types_round_trip_without_metadata_or_value_loss() {
    use astersql_parser_mysql::r#type::{
        TypeBlob, TypeDate, TypeDatetime, TypeDouble, TypeFloat, TypeLong, TypeLonglong,
        TypeNewDecimal, TypeSet, TypeString, TypeTimestamp, TypeTiny, TypeVarchar,
    };

    let (_domain, session) = CreateAnalyzeSession().expect("canonical type session");
    execute(&session, "create database type_compat");
    execute(&session, "use type_compat");
    // 该矩阵同时覆盖数值、字符/二进制、时间、JSON、枚举集合及默认值语义。
    execute(
        &session,
        "create table type_cases (\
         id bigint unsigned not null primary key,\
         tiny_s tinyint null,\
         int_s int null,\
         big_u bigint unsigned null,\
         dec_v decimal(20,6) null,\
         float_v float null,\
         double_v double null,\
         bool_v boolean null,\
         char_v char(4) null,\
         varchar_v varchar(16) null,\
         text_v text null,\
         binary_v binary(4) null,\
         varbinary_v varbinary(8) null,\
         blob_v blob null,\
         date_v date null,\
         time_v time(3) null,\
         datetime_v datetime(6) null,\
         timestamp_v timestamp(3) null,\
         json_v json null,\
         enum_v enum('red','blue') null,\
         set_v set('a','b') null,\
         nullable_v varchar(8) null,\
         default_v varchar(8) not null default 'ready')",
    );
    // 首行刻意使用整数边界、Unicode、内嵌零字节和带精度的时间值，防止静默截断或改写。
    execute(
        &session,
        "insert into type_cases values (\
         18446744073709551615, -128, -2147483648, 18446744073709551615,\
         12345678901234.123456, 1.25, 2.5, true,\
         'é', '空', '', X'41004200', X'410042', X'41004200',\
         '2024-02-29', '12:34:56.789', '2024-02-29 12:34:56.123456',\
         '2024-02-29 12:34:56.123', '{\"n\":1}', 'blue', 'a,b', null, default)",
    );
    execute(
        &session,
        "insert into type_cases (id, default_v) values (2, default)",
    );

    // SELECT 列顺序与后续元数据及行值断言一一对应，避免只验证其中一个表示层。
    let mut result = session
        .execute(
            "select id, tiny_s, int_s, big_u, dec_v, float_v, double_v, bool_v,\
             char_v, varchar_v, text_v, binary_v, varbinary_v, blob_v,\
             date_v, time_v, datetime_v, timestamp_v, json_v, enum_v, set_v,\
             nullable_v, default_v from type_cases",
        )
        .expect("read type matrix")
        .remove(0);
    // 字段类型码必须与 MySQL 协议层暴露的规范类型一致。
    assert_eq!(
        result
            .result_fields()
            .iter()
            .map(|field| field.as_ref().expect("typed field").column.GetType())
            .collect::<Vec<_>>(),
        vec![
            TypeLonglong,
            TypeTiny,
            TypeLong,
            TypeLonglong,
            TypeNewDecimal,
            TypeFloat,
            TypeDouble,
            TypeTiny,
            TypeString,
            TypeVarchar,
            TypeBlob,
            TypeString,
            TypeVarchar,
            TypeBlob,
            TypeDate,
            astersql_parser_mysql::r#type::TypeDuration,
            TypeDatetime,
            TypeTimestamp,
            astersql_parser_mysql::r#type::TypeJSON,
            astersql_parser_mysql::r#type::TypeEnum,
            TypeSet,
            TypeVarchar,
            TypeVarchar,
        ]
    );
    // 已填充行验证各类型对外呈现的文本形式，包括二进制零字节和规范化 JSON。
    assert_eq!(
        result.Next().expect("read populated type row"),
        Some(vec![
            "18446744073709551615".into(),
            "-128".into(),
            "-2147483648".into(),
            "18446744073709551615".into(),
            "12345678901234.123456".into(),
            "1.25".into(),
            "2.5".into(),
            "1".into(),
            "é".into(),
            "空".into(),
            "".into(),
            "A\0B\0".into(),
            "A\0B".into(),
            "A\0B\0".into(),
            "2024-02-29".into(),
            "12:34:56.789".into(),
            "2024-02-29 12:34:56.123456".into(),
            "2024-02-29 12:34:56.123".into(),
            "{\"n\": 1}".into(),
            "blue".into(),
            "a,b".into(),
            "<nil>".into(),
            "ready".into(),
        ])
    );
    // 省略的可空列应保持 NULL，显式 DEFAULT 则应解析为列默认值。
    assert_eq!(
        result.Next().expect("read NULL/default row"),
        Some(vec![
            "2".into(),
            "<nil>".into(),
            "<nil>".into(),
            "<nil>".into(),
            "<nil>".into(),
            "<nil>".into(),
            "<nil>".into(),
            "<nil>".into(),
            "<nil>".into(),
            "<nil>".into(),
            "<nil>".into(),
            "<nil>".into(),
            "<nil>".into(),
            "<nil>".into(),
            "<nil>".into(),
            "<nil>".into(),
            "<nil>".into(),
            "<nil>".into(),
            "<nil>".into(),
            "<nil>".into(),
            "<nil>".into(),
            "<nil>".into(),
            "ready".into(),
        ])
    );
    assert_eq!(result.Next().expect("type result exhausted"), None);

    // 下界之外的 TINYINT 必须报错，不能经截断或环绕后以其他值落盘。
    let overflow = match session.execute("insert into type_cases (id, tiny_s) values (3, -129)") {
        Ok(_) => panic!("TINYINT underflow must not be stored as a different value"),
        Err(error) => error,
    };
    assert!(
        overflow
            .to_string()
            .to_ascii_lowercase()
            .contains("out of range"),
        "unexpected overflow error: {overflow}"
    );
}
