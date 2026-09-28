// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//	http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// TTL SQL 字面量格式化的单元测试入口。
//
// 校验 `FormatSQLDatum` 将字符串 Datum（列值的统一内存表示）按 MySQL 转义规则输出为可嵌入 SQL 的字面量。

/// 验证字符串 Datum 中的单引号与换行符会被转义成 MySQL 可解析的字面量。
#[test]
fn sql_datum_formatter_escapes_mysql_string_literals() {
    use crate::{Datum, FieldKind, FieldType, FormatSQLDatum};

    let formatted = FormatSQLDatum(
        &Datum::String("O'Reilly\n".to_owned()),
        &FieldType::new(FieldKind::Varchar),
    )
    .expect("string datum");
    assert_eq!(formatted, "'O\\'Reilly\\n'");
}
