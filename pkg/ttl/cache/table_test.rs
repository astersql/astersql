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

// PhysicalTable 构造、过期时间计算与扫描范围拆分的单元测试。
//
// 覆盖无 TTL、各类主键形态、分区解析、EvalExpireTime 与 SplitScanRanges 等路径。

use crate::table::{
    EvalExpireTime, GetASCIIPrefixDatumFromBytes, GetNextBytesHandleDatum,
    GetNextIntDatumFromCommonHandle, GetNextIntHandle, IndexColumn, IndexInfo, KeyKind,
    NewBasePhysicalTable, NewPhysicalTable, PartitionDefinition, RegionProvider, TTLInfo,
    TableInfo, TimeUnit, getTableKeyColumns,
};
use crate::task::Datum;

/// 构造测试用列元信息。
fn column(name: &str, public: bool, kind: KeyKind) -> crate::table::Column {
    crate::table::Column {
        id: 0,
        name: name.into(),
        public,
        key_kind: kind,
        nullable: false,
        hidden: false,
    }
}

/// 构造默认非分区、无主键、无 TTL 的基础表信息。
fn base_table(name: &str) -> TableInfo {
    TableInfo {
        id: 1,
        name: name.into(),
        public: true,
        pk_is_handle: false,
        common_handle: false,
        columns: Vec::new(),
        primary_index_offsets: Vec::new(),
        indexes: Vec::new(),
        partitions: Vec::new(),
        ttl: None,
    }
}

// 对应 Go 用例 t1：没有 TTL 表达式的表不能构造出 PhysicalTable。
#[test]
fn test_new_ttl_table_rejects_non_ttl_table() {
    let table = base_table("t1");
    assert!(NewPhysicalTable("test", &table, "").is_err());
}

// 对应 Go 用例 ttl1：没有主键的表使用 _tidb_rowid 作为唯一 key column。
#[test]
fn test_new_ttl_table_uses_row_id_without_primary_key() {
    let mut table = base_table("ttl1");
    table.columns = vec![column("t", true, KeyKind::SignedInt)];
    table.ttl = Some(TTLInfo {
        column_name: "t".into(),
        interval: "2".into(),
        unit: TimeUnit::Hour,
    });
    let physical = NewPhysicalTable("test", &table, "").unwrap();
    assert_eq!(physical.ID, table.id);
    assert_eq!(physical.KeyColumns.len(), 1);
    assert_eq!(physical.KeyColumns[0].name, "_tidb_rowid");
    assert!(physical.Partition.is_empty());
    assert!(physical.PartitionDef.is_none());
}

// 对应 Go 用例 ttl2：单列整数主键作为 key column。
#[test]
fn test_new_ttl_table_uses_single_column_primary_key() {
    let mut table = base_table("ttl2");
    table.pk_is_handle = true;
    table.columns = vec![
        column("id", true, KeyKind::SignedInt),
        column("t", true, KeyKind::SignedInt),
    ];
    table.ttl = Some(TTLInfo {
        column_name: "t".into(),
        interval: "3".into(),
        unit: TimeUnit::Hour,
    });
    let physical = NewPhysicalTable("test", &table, "").unwrap();
    assert_eq!(physical.KeyColumns.len(), 1);
    assert_eq!(physical.KeyColumns[0].name, "id");
}

// 对应 Go 用例 ttl3：复合主键 (a,b,c) 按 primary_index_offsets 顺序展开。
#[test]
fn test_new_ttl_table_uses_composite_primary_key() {
    let mut table = base_table("ttl3");
    table.common_handle = true;
    table.columns = vec![
        column("a", true, KeyKind::SignedInt),
        column("b", true, KeyKind::Bytes),
        column("c", true, KeyKind::Bytes),
        column("t", true, KeyKind::SignedInt),
    ];
    table.primary_index_offsets = vec![0, 1, 2];
    table.ttl = Some(TTLInfo {
        column_name: "t".into(),
        interval: "1".into(),
        unit: TimeUnit::Month,
    });
    let physical = NewPhysicalTable("test", &table, "").unwrap();
    assert_eq!(physical.KeyColumns.len(), 3);
    assert_eq!(
        physical
            .KeyColumns
            .iter()
            .map(|c| c.name.as_str())
            .collect::<Vec<_>>(),
        vec!["a", "b", "c"]
    );
}

// 对应 Go 用例 ttl4：分区表按分区名解析出对应分区 ID，且未知分区名报错。
#[test]
fn test_new_ttl_table_partitioned_table_resolves_partition_id() {
    let mut table = base_table("ttl4");
    table.pk_is_handle = true;
    table.columns = vec![
        column("id", true, KeyKind::SignedInt),
        column("t", true, KeyKind::SignedInt),
    ];
    table.partitions = vec![
        PartitionDefinition {
            id: 10,
            name: "p0".into(),
        },
        PartitionDefinition {
            id: 11,
            name: "p1".into(),
        },
    ];
    table.ttl = Some(TTLInfo {
        column_name: "t".into(),
        interval: "1".into(),
        unit: TimeUnit::Day,
    });

    let physical = NewPhysicalTable("test", &table, "p1").unwrap();
    assert_eq!(physical.ID, 11);
    assert_eq!(physical.Partition, "p1");
    assert_eq!(physical.PartitionDef.as_ref().unwrap().id, 11);

    assert!(NewPhysicalTable("test", &table, "").is_err());
    assert!(NewPhysicalTable("test", &table, "missing").is_err());
}

// 对应 Go：不存在的 TTL 时间列应报错。
#[test]
fn test_new_ttl_table_rejects_missing_time_column() {
    let mut table = base_table("ttl_missing_col");
    table.ttl = Some(TTLInfo {
        column_name: "missing".into(),
        interval: "1".into(),
        unit: TimeUnit::Day,
    });
    assert!(NewPhysicalTable("test", &table, "").is_err());
}

// 对应 Go：非 public 表不能构造 PhysicalTable。
#[test]
fn test_new_base_physical_table_rejects_non_public_table() {
    let mut table = base_table("ttl_not_public");
    table.public = false;
    let time_column = column("t", true, KeyKind::SignedInt);
    assert!(NewBasePhysicalTable("test", &table, "", time_column).is_err());
}

// 无主键时退回隐式 _tidb_rowid 作为扫描键列。
#[test]
fn test_get_table_key_columns_default_row_id_when_no_primary_key() {
    let table = base_table("t");
    let columns = getTableKeyColumns(&table).unwrap();
    assert_eq!(columns.len(), 1);
    assert_eq!(columns[0].name, "_tidb_rowid");
}

// FullName：有分区时为 db.table.partition，否则为 db.table。
#[test]
fn test_full_name_includes_partition_when_present() {
    let mut table = base_table("t");
    table.columns = vec![column("t", true, KeyKind::SignedInt)];
    table.partitions = vec![PartitionDefinition {
        id: 10,
        name: "p0".into(),
    }];
    table.ttl = Some(TTLInfo {
        column_name: "t".into(),
        interval: "1".into(),
        unit: TimeUnit::Day,
    });
    let physical = NewPhysicalTable("test", &table, "p0").unwrap();
    assert_eq!(physical.FullName(), "test.t.p0");

    let mut table2 = base_table("t2");
    table2.columns = vec![column("t", true, KeyKind::SignedInt)];
    table2.ttl = Some(TTLInfo {
        column_name: "t".into(),
        interval: "1".into(),
        unit: TimeUnit::Day,
    });
    let physical2 = NewPhysicalTable("test", &table2, "").unwrap();
    assert_eq!(physical2.FullName(), "test.t2");
}

// 键前缀长度不得超过 KeyColumns 数量。
#[test]
fn test_validate_key_prefix_rejects_overlong_prefix() {
    let mut table = base_table("t");
    table.columns = vec![column("t", true, KeyKind::SignedInt)];
    table.ttl = Some(TTLInfo {
        column_name: "t".into(),
        interval: "1".into(),
        unit: TimeUnit::Day,
    });
    let physical = NewPhysicalTable("test", &table, "").unwrap();
    assert!(physical.ValidateKeyPrefix(&[Datum::Int(1)]).is_ok());
    assert!(
        physical
            .ValidateKeyPrefix(&[Datum::Int(1), Datum::Int(2)])
            .is_err()
    );
}

// 对应 Go TestEvalTTLExpireTime 中 day/month 间隔用例：从整数秒直接计算差值。
#[test]
fn test_eval_expire_time_day_and_month_units() {
    let now = 1_700_000_000_i64;
    assert_eq!(
        EvalExpireTime(now, "1", TimeUnit::Day).unwrap(),
        now - 86400
    );
    assert_eq!(
        EvalExpireTime(now, "2", TimeUnit::Week).unwrap(),
        now - 14 * 86400
    );
    assert_eq!(
        EvalExpireTime(now, "90", TimeUnit::Minute).unwrap(),
        now - 90 * 60
    );

    // 1970-01-01 减去 1 个月应回到 1969-12-01（跨年）。
    let jan_1_1970 = 0_i64;
    let expected = -31 * 86400; // December has 31 days
    assert_eq!(
        EvalExpireTime(jan_1_1970, "1", TimeUnit::Month).unwrap(),
        expected
    );
}

// interval 非数字时应报错。
#[test]
fn test_eval_expire_time_rejects_invalid_interval() {
    assert!(EvalExpireTime(0, "not-a-number", TimeUnit::Day).is_err());
}

// 对应 Go TestEvalTTLExpireTime：字符串形式的 HOUR_MINUTE 间隔也必须可计算。
#[test]
fn test_eval_expire_time_supports_hour_minute_interval() {
    assert_eq!(
        EvalExpireTime(0, "'1:3'", TimeUnit::HourMinute).unwrap(),
        -(60 * 60 + 3 * 60)
    );
}

// PhysicalTable::EvalExpireTime 读取表上 TTLInfo 的 interval/unit。
#[test]
fn test_physical_table_eval_expire_time_uses_ttl_info() {
    let mut table = base_table("t");
    table.columns = vec![column("t", true, KeyKind::SignedInt)];
    table.ttl = Some(TTLInfo {
        column_name: "t".into(),
        interval: "1".into(),
        unit: TimeUnit::Day,
    });
    let physical = NewPhysicalTable("test", &table, "").unwrap();
    assert_eq!(physical.EvalExpireTime(86400).unwrap(), 0);
}

/// 固定返回预设 Region 范围的 RegionProvider 桩。
struct FixedRegions {
    ranges: Vec<crate::table::KeyRange>,
}
impl RegionProvider for FixedRegions {
    fn locate_key_range(
        &self,
        _start: &[u8],
        _end: &[u8],
    ) -> Result<Vec<crate::table::KeyRange>, String> {
        Ok(self.ranges.clone())
    }
}

/// 构造单字节起止的测试 KeyRange。
fn key_range(start: u8, end: u8) -> crate::table::KeyRange {
    crate::table::KeyRange {
        start: vec![start],
        end: vec![end],
    }
}

// 对应 Go SplitScanRanges：region 数量足够时应按主键类型拆出多个 ScanRange，
// key column 为空或 splitCnt<=1 时退回单个全表范围。
#[test]
fn test_split_scan_ranges_falls_back_when_split_count_not_greater_than_one() {
    let mut table = base_table("t");
    table.columns = vec![column("id", true, KeyKind::SignedInt)];
    table.pk_is_handle = true;
    table.ttl = Some(TTLInfo {
        column_name: "id".into(),
        interval: "1".into(),
        unit: TimeUnit::Day,
    });
    let physical = NewPhysicalTable("test", &table, "").unwrap();
    let ranges = physical.SplitScanRanges(None, 1).unwrap();
    assert_eq!(ranges.len(), 1);
    assert!(ranges[0].Start.is_empty() && ranges[0].End.is_empty());
}

// 无 RegionProvider 时无法按 Region 拆分，退回单范围。
#[test]
fn test_split_scan_ranges_falls_back_without_region_provider() {
    let mut table = base_table("t");
    table.columns = vec![column("id", true, KeyKind::SignedInt)];
    table.pk_is_handle = true;
    table.ttl = Some(TTLInfo {
        column_name: "id".into(),
        interval: "1".into(),
        unit: TimeUnit::Day,
    });
    let physical = NewPhysicalTable("test", &table, "").unwrap();
    let ranges = physical.SplitScanRanges(None, 4).unwrap();
    assert_eq!(ranges.len(), 1);
}

// Region 数量足够时按主键类型拆出多个 ScanRange。
#[test]
fn test_split_scan_ranges_splits_by_region_count() {
    let mut table = base_table("t");
    table.columns = vec![column("id", true, KeyKind::SignedInt)];
    table.pk_is_handle = true;
    table.ttl = Some(TTLInfo {
        column_name: "id".into(),
        interval: "1".into(),
        unit: TimeUnit::Day,
    });
    let physical = NewPhysicalTable("test", &table, "").unwrap();
    let regions = FixedRegions {
        ranges: vec![
            key_range(0, 1),
            key_range(1, 2),
            key_range(2, 3),
            key_range(3, 4),
        ],
    };
    let ranges = physical.SplitScanRanges(Some(&regions), 4).unwrap();
    assert!(ranges.len() > 1);
    assert!(matches!(ranges[0].Start, ref v if v.is_empty()));
    assert!(matches!(ranges.last().unwrap().End, ref v if v.is_empty()));
}

// 覆盖整数/字节句柄推进辅助函数的边界输入。
#[test]
fn test_get_next_int_handle_helpers() {
    let prefix = vec![1, 2, 3];
    assert_eq!(GetNextIntHandle(&prefix, &prefix), Some(i64::MIN));
    let mut outside = prefix.clone();
    outside.extend_from_slice(&[9, 9, 9]);
    assert!(GetNextIntHandle(&[255], &prefix).is_none());
    let _ = GetNextIntDatumFromCommonHandle(&outside, &prefix, true);
    let _ = GetNextBytesHandleDatum(&outside, &prefix);
}

#[test]
fn test_get_ascii_prefix_datum_from_bytes_truncates_at_first_control_byte() {
    assert_eq!(
        GetASCIIPrefixDatumFromBytes(b"abc"),
        Datum::String("abc".into())
    );
    assert_eq!(
        GetASCIIPrefixDatumFromBytes(b"\0abc"),
        Datum::String(String::new())
    );
    assert_eq!(
        GetASCIIPrefixDatumFromBytes(b"ab\x01c"),
        Datum::String("ab".into())
    );
    assert_eq!(
        GetASCIIPrefixDatumFromBytes(b"ab\rc\xff"),
        Datum::String("ab\rc".into())
    );
}

#[test]
fn ttl_index_selection_rejects_unsafe_indexes_and_prefers_single_time_column() {
    let mut table = base_table("indexed");
    table.columns = vec![
        crate::table::Column {
            id: 1,
            ..column("id", true, KeyKind::SignedInt)
        },
        crate::table::Column {
            id: 2,
            ..column("created_at", true, KeyKind::SignedInt)
        },
    ];
    table.pk_is_handle = true;
    table.ttl = Some(TTLInfo {
        column_name: "created_at".into(),
        interval: "1".into(),
        unit: TimeUnit::Day,
    });
    table.indexes = vec![
        IndexInfo {
            id: 10,
            name: "wrong_first".into(),
            public: true,
            columns: vec![IndexColumn {
                column_offset: 0,
                prefix_length: None,
            }],
            ..IndexInfo::default()
        },
        IndexInfo {
            id: 11,
            name: "ttl_idx".into(),
            public: true,
            columns: vec![IndexColumn {
                column_offset: 1,
                prefix_length: None,
            }],
            ..IndexInfo::default()
        },
    ];
    let physical = NewPhysicalTable("test", &table, "").unwrap();
    assert_eq!(physical.FindTTLIndex().unwrap().name, "ttl_idx");
    let plan = physical
        .BuildTTLIndexScanPlan(&physical.Indices[1])
        .unwrap();
    assert_eq!(
        plan.ScanColumns
            .iter()
            .map(|column| column.name.as_str())
            .collect::<Vec<_>>(),
        vec!["created_at", "id"]
    );
    assert_eq!(
        plan.TableKey(&[Datum::Time(1), Datum::Int(7)]),
        vec![Datum::Int(7)]
    );
}
