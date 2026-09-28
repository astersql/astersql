// Copyright 2026 AsterSQL.
// Copyright 2019 PingCAP, Inc.
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

// `Column` null bitmap 与定长追加的基础单元测试。
//
// 覆盖跨字节边界写入 null（第 8 行，bit 落在下一字节）后，行数、IsNull 与取值仍正确。

/// 验证定长列在字节边界处混合追加值与 null 时 bitmap 与取值一致。
#[test]
fn column_bitmap_tracks_values_and_nulls_across_a_byte_boundary() {
    use super::newFixedLenColumn;
    // elemLen=8 对应 int64；capacity=10 使 nullBitmap 跨过第 8 位字节边界。
    let mut column = newFixedLenColumn(8, 10);
    for value in 0..10 {
        if value == 8 {
            column.AppendNull();
        } else {
            column.AppendInt64(value);
        }
    }
    assert_eq!(column.Rows(), 10);
    assert!(column.IsNull(8));
    assert_eq!(column.GetInt64(9), 9);
}

/// 覆盖 Go 的 Resize/SetNulls/PreAlloc 系列断言。
#[test]
fn resize_and_null_ranges_match_go() {
    use super::{newFixedLenColumn, sizeInt64};

    let mut column = newFixedLenColumn(sizeInt64, 2);
    column.ResizeInt64(9, true);
    assert_eq!(column.Rows(), 9);
    assert_eq!(column.nullCount(), 9);
    column.SetNulls(1, 8, false);
    assert!(column.IsNull(0));
    assert!(!column.IsNull(1));
    assert!(!column.IsNull(7));
    assert!(column.IsNull(8));
    assert_eq!(column.nullCount(), 2);

    column.ResizeInt64(3, false);
    assert_eq!(column.Int64s(), vec![0, 0, 0]);
    assert_eq!(column.nullCount(), 0);
    column.AppendInt64(2_333);
    assert_eq!(column.GetInt64(3), 2_333);
}

/// 覆盖 Go TestReserve/TestGetRaw，扩容时必须保留已有数据。
#[test]
fn reserve_and_raw_access_preserve_data() {
    use super::{newFixedLenColumn, newVarLenColumn};

    let mut strings = newVarLenColumn(0);
    strings.AppendString("abc");
    let old_data = strings.data.clone();
    let old_offsets = strings.offsets.clone();
    strings.Reserve(100, 100, 100);
    assert_eq!(strings.data, old_data);
    assert_eq!(strings.offsets, old_offsets);
    assert!(strings.data.capacity() - strings.data.len() >= 100);
    assert!(strings.offsets.capacity() - strings.offsets.len() >= 100);
    assert_eq!(strings.GetRaw(0), b"abc");
    strings.SetRaw(0, b"xyz");
    assert_eq!(strings.GetString(0), "xyz");

    let mut fixed = newFixedLenColumn(4, 1);
    fixed.AppendFloat32(1.5);
    assert_eq!(fixed.GetRaw(0), &1.5_f32.to_ne_bytes());
}

/// 覆盖 Go 的 fixed/varlen CopyReconstruct 与追加后续行。
#[test]
fn copy_reconstruct_fixed_and_variable_columns_match_go() {
    use super::{newFixedLenColumn, newVarLenColumn};

    let mut fixed = newFixedLenColumn(8, 5);
    let mut variable = newVarLenColumn(5);
    for (index, value) in [10, 20, 30, 40, 50].into_iter().enumerate() {
        if index == 2 {
            fixed.AppendNull();
            variable.AppendNull();
        } else {
            fixed.AppendInt64(value);
            variable.AppendString(&value.to_string());
        }
    }
    let mut fixed = fixed.CopyReconstruct(Some(&[4, 2, 0]), None);
    let mut variable = variable.CopyReconstruct(Some(&[4, 2, 0]), None);
    assert_eq!(fixed.Rows(), 3);
    assert_eq!(fixed.GetInt64(0), 50);
    assert!(fixed.IsNull(1));
    assert_eq!(variable.GetString(0), "50");
    assert!(variable.IsNull(1));
    assert_eq!(variable.GetString(2), "10");

    fixed.AppendInt64(60);
    variable.AppendString("60");
    assert_eq!(fixed.GetInt64(3), 60);
    assert_eq!(variable.GetString(3), "60");
}

/// 覆盖 Go TestColumnCopy：新分配和复用目标都必须深拷贝底层缓冲。
#[test]
fn copy_construct_is_deep_for_new_and_reused_destinations() {
    use super::{newFixedLenColumn, newVarLenColumn, sizeInt64};

    let mut fixed = newFixedLenColumn(sizeInt64, 3);
    fixed.AppendInt64(11);
    fixed.AppendNull();
    fixed.AppendInt64(33);
    let fresh = fixed.CopyConstruct(None);
    let reused = fixed.CopyConstruct(Some(newFixedLenColumn(sizeInt64, 1)));

    fixed.data[0] ^= 0xff;
    fixed.nullBitmap[0] = 0;
    assert_eq!(fresh.GetInt64(0), 11);
    assert!(!fresh.IsNull(0));
    assert!(fresh.IsNull(1));
    assert_eq!(reused.GetInt64(2), 33);

    let mut variable = newVarLenColumn(2);
    variable.AppendString("alpha");
    variable.AppendString("beta");
    let copied = variable.CopyConstruct(None);
    variable.SetRaw(0, b"ALPHA");
    assert_eq!(copied.GetString(0), "alpha");
    assert_eq!(copied.GetString(1), "beta");
}

/// 覆盖 Go 的各定长/变长访问器，确保追加后的原始布局和类型往返一致。
#[test]
fn typed_append_and_getters_round_trip_go_layouts() {
    use super::types;
    use super::{
        newFixedLenColumn, newVarLenColumn, sizeFloat32, sizeFloat64, sizeInt64, sizeMyDecimal,
        sizeTime,
    };

    let mut ints = newFixedLenColumn(sizeInt64, 2);
    ints.AppendInt64(-7);
    ints.AppendUint64(u64::MAX - 2);
    assert_eq!(ints.GetInt64(0), -7);
    assert_eq!(ints.GetUint64(1), u64::MAX - 2);

    let mut f32s = newFixedLenColumn(sizeFloat32, 1);
    f32s.AppendFloat32(1.25);
    assert_eq!(f32s.Float32s(), vec![1.25]);
    let mut f64s = newFixedLenColumn(sizeFloat64, 1);
    f64s.AppendFloat64(-2.5);
    assert_eq!(f64s.GetFloat64(0), -2.5);

    let decimal = types::MyDecimal::default();
    let mut decimals = newFixedLenColumn(sizeMyDecimal, 1);
    decimals.AppendMyDecimal(&decimal);
    assert_eq!(decimals.GetDecimal(0), decimal);

    let time = types::Time {
        coreTime: types::CoreTime(0x1234_5678),
    };
    let mut times = newFixedLenColumn(sizeTime, 1);
    times.AppendTime(time);
    assert_eq!(times.GetTime(0), time);
    times.AppendDuration(types::Duration {
        Duration: -42,
        Fsp: 6,
    });
    assert_eq!(
        times.GetDuration(1, 3),
        types::Duration {
            Duration: -42,
            Fsp: 3
        }
    );

    let mut names = newVarLenColumn(3);
    names.AppendEnum(types::Enum {
        Name: "red".into(),
        Value: 2,
    });
    names.AppendSet(types::Set {
        Name: "x,y".into(),
        Value: 5,
    });
    names.AppendBytes(b"raw");
    assert_eq!(
        names.GetEnum(0),
        types::Enum {
            Name: "red".into(),
            Value: 2
        }
    );
    assert_eq!(
        names.GetSet(1),
        types::Set {
            Name: "x,y".into(),
            Value: 5
        }
    );
    assert_eq!(names.GetBytes(2), b"raw");

    let vector = types::ParseVectorFloat32("[1.0,-2.5,3.25]").unwrap();
    let mut vectors = newVarLenColumn(1);
    vectors.AppendVectorFloat32(vector);
    assert_eq!(vectors.GetVectorFloat32(0).Elements(), &[1.0, -2.5, 3.25]);
}

/// 覆盖 Go TestVectorizedNulls/TestColumnResizeInt64 的 bitmap 精确结果。
#[test]
fn merge_nulls_and_resize_keep_exact_bitmap_bits() {
    use super::{newFixedLenColumn, sizeInt64};

    let mut left = newFixedLenColumn(sizeInt64, 11);
    let mut right = newFixedLenColumn(sizeInt64, 11);
    left.ResizeInt64(11, false);
    right.ResizeInt64(11, false);
    left.SetNull(1, true);
    right.SetNull(9, true);
    left.MergeNulls(&[*right]);
    assert!(left.IsNull(1));
    assert!(left.IsNull(9));
    assert_eq!(left.nullCount(), 2);

    left.ResizeUint64(4, false);
    assert_eq!(left.nullBitmap, vec![0b0000_1111]);
    left.AppendUint64(11);
    left.AppendNull();
    assert_eq!(left.nullBitmap, vec![0b0001_1111]);
    left.ResizeUint64(11, false);
    assert_eq!(left.nullBitmap, vec![0xff, 0b0000_0111]);
    left.ResizeUint64(7, true);
    assert_eq!(left.nullBitmap, vec![0]);
    left.AppendUint64(32);
    left.AppendUint64(32);
    assert_eq!(left.nullBitmap, vec![0b1000_0000, 0b0000_0001]);
}

/// 覆盖 Go TestResetColumn：公开 Reset 必须同时切换固定/变长布局。
#[test]
fn reset_changes_column_layout_for_the_requested_eval_type() {
    use super::types;
    use super::{newFixedLenColumn, newVarLenColumn, sizeGoDuration, sizeInt64};

    let mut variable = newVarLenColumn(1);
    variable.AppendString("old");
    variable.Reset(types::ETInt);
    assert!(variable.IsFixed());
    assert_eq!(variable.elemBuf.len(), sizeInt64);
    variable.AppendInt64(9);
    assert_eq!(variable.GetInt64(0), 9);

    let mut fixed = newFixedLenColumn(sizeInt64, 1);
    fixed.AppendInt64(1);
    fixed.Reset(types::ETString);
    assert!(!fixed.IsFixed());
    assert_eq!(fixed.offsets, vec![0]);
    fixed.AppendString("new");
    assert_eq!(fixed.GetString(0), "new");

    let mut time = newFixedLenColumn(super::sizeTime, 1);
    time.Reset(types::ETDuration);
    time.AppendDuration(types::Duration::default());
    assert_eq!(time.data.len(), sizeGoDuration);
}
