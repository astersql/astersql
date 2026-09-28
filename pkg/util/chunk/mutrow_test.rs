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

// `MutRow`（可变单行 Chunk）克隆与写值隔离的单元测试。
//
// 对应 Go `mutrow_test.go`：`Clone` 应深拷贝列数据，对副本的
// `SetValue` 不得改写原行，避免共享底层缓冲导致的别名问题。

/// 验证克隆后修改副本不影响原行，且未改列保持一致。
#[test]
fn mutable_row_updates_values_without_aliasing_its_clone() {
    use super::mutrow::{GoAny, MutRowFromValues};
    let row = MutRowFromValues(vec![GoAny::Int64(7), GoAny::String("a".to_owned())]);
    let mut cloned = row.Clone();
    // 只改第 0 列；原行仍为 7，副本为 9，字符串列两边均为 "a"。
    cloned.SetValue(0, GoAny::Int64(9));
    assert_eq!(row.ToRow().GetInt64(0), 7);
    assert_eq!(cloned.ToRow().GetInt64(0), 9);
    assert_eq!(cloned.ToRow().GetString(1), "a");
}

/// Go routes `KindMysqlBit` through `Datum.GetValue()` as a binary literal in
/// both construction and assignment paths.
#[test]
fn mysql_bit_datums_preserve_their_binary_payload() {
    use super::mutrow::{MutRowFromDatums, MutRowFromTypes};
    use super::{mysql, types};

    let bit = types::NewMysqlBitDatum(types::BinaryLiteral(vec![0x12, 0x34]));
    let constructed = MutRowFromDatums(vec![bit.clone()]);
    assert_eq!(constructed.ToRow().GetBytes(0), vec![0x12, 0x34]);

    let mut assigned = MutRowFromTypes(vec![*types::NewFieldType(mysql::TypeBit)]);
    assigned.SetDatum(0, bit);
    assert_eq!(assigned.ToRow().GetBytes(0), vec![0x12, 0x34]);

    let mut interface = types::Datum::default();
    interface.SetInterface(Box::new(42_i64));
    assigned.SetDatum(0, interface);
    assert_eq!(assigned.ToRow().GetInt64(0), 42);
}
