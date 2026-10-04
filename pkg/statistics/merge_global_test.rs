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
use crate::*;

fn key(value: i64) -> Vec<u8> {
    codec::EncodeKey(
        codec::time::UTC,
        Vec::new(),
        vec![types::NewIntDatum(value)],
    )
    .unwrap()
}
fn hist(buckets: &[(i64, i64, i64, i64)]) -> Histogram {
    let ft = types::NewFieldType(types::mysql::TypeLonglong);
    let mut h = NewHistogram(1, 0, 2, 4, &ft, buckets.len(), 8);
    let mut cum = 0;
    for &(lo, up, mass, repeat) in buckets {
        cum += mass;
        h.AppendBucketWithNDV(
            &types::NewIntDatum(lo),
            &types::NewIntDatum(up),
            cum,
            repeat,
            0,
        );
    }
    h
}
fn top(entries: &[(i64, u64)]) -> TopN {
    let mut t = NewTopN(entries.len());
    for &(value, count) in entries {
        t.AppendTopN(key(value), count);
    }
    t.Sort();
    t
}
fn merge(h: &[Option<Histogram>], t: &[Option<TopN>], n: u32, b: i64) -> (Option<TopN>, Histogram) {
    MergePartTopNAndHistToGlobal(
        &stmtctx::NewStmtCtx(),
        &sqlkiller::sqlkiller::SQLKiller::default(),
        t,
        h,
        n,
        b,
        false,
    )
    .unwrap()
}
#[test]
fn combined_merge_aggregates_topn_and_repeats_without_losing_rows() {
    let h = vec![Some(hist(&[(1, 5, 10, 4)])), Some(hist(&[(2, 5, 12, 3)]))];
    let t = vec![Some(top(&[(5, 6), (9, 2)])), Some(top(&[(9, 1)]))];
    let (t, g) = merge(&h, &t, 1, 2);
    let t = t.unwrap();
    assert_eq!(t.QueryTopN(&key(5)), (13, true));
    assert_eq!(g.NotNullCount(), 18.0);
    assert_eq!(g.NullCount, 4);
    assert_eq!(g.TotColSize, 16);
    assert_eq!(g.LastUpdateVersion, 4);
    assert_eq!(h[0].as_ref().unwrap().Buckets[0].Count, 10);
    assert!(g.Buckets.iter().all(|b| b.NDV == 0));
}
#[test]
fn combined_merge_promotes_bucket_repeats_without_partition_topn() {
    let (t, g) = merge(&[Some(hist(&[(1, 2, 10, 6)]))], &[], 1, 2);
    assert_eq!(t.unwrap().QueryTopN(&key(2)), (6, true));
    assert_eq!(g.NotNullCount(), 4.0);
    assert_eq!(g.Buckets.last().unwrap().Repeat, 0);
}
#[test]
fn combined_merge_overlap_keeps_repeat_with_right_point_mass() {
    let (_, g) = merge(
        &[
            Some(hist(&[(0, 10, 100, 20)])),
            Some(hist(&[(5, 20, 100, 10)])),
        ],
        &[],
        0,
        2,
    );
    assert_eq!(g.NotNullCount(), 200.0);
    assert!(g.Len() <= 2);
    assert!(g.Buckets.iter().all(|b| b.Repeat <= b.Count));
}
#[test]
fn combined_merge_rejects_missing_histograms_and_honors_cancellation() {
    let sc = stmtctx::NewStmtCtx();
    let cancel = sqlkiller::sqlkiller::SQLKiller::default();
    for h in [vec![], vec![None, None]] {
        let e = MergePartTopNAndHistToGlobal(&sc, &cancel, &[], &h, 2, 10, false).unwrap_err();
        assert!(e.to_string().contains("no partition histograms"));
    }
    let e = MergePartTopNAndHistToGlobal(
        &sc,
        &{
            let killer = sqlkiller::sqlkiller::SQLKiller::default();
            killer.SendKillSignal(sqlkiller::sqlkiller::QueryInterrupted);
            killer
        },
        &[],
        &[Some(hist(&[(1, 2, 5, 3)]))],
        2,
        10,
        false,
    )
    .unwrap_err();
    assert!(e.to_string().contains("interrupted"));
    let (_, g) = merge(&[Some(hist(&[]))], &[], 2, 10);
    assert_eq!(g.Len(), 0);
    assert_eq!(g.NullCount, 2);
}
#[test]
fn combined_merge_virtual_histograms_do_not_wrap_bucket_indices() {
    let t = top(&(0..70_000).map(|i| (i, 2)).collect::<Vec<_>>());
    let (t, g) = merge(&[Some(hist(&[]))], &[Some(t)], 0, 128);
    assert!(t.is_none());
    assert_eq!(g.NotNullCount(), 140_000.0);
    assert!(g.Len() <= 128);
    assert_eq!(g.GetLower(0).GetInt64(), 0);
    assert_eq!(g.GetUpper(g.Len() - 1).GetInt64(), 69_999);
}
#[test]
fn combined_merge_singletons_follow_active_default_and_pool_capacity() {
    let active = vardef::AnalyzeDefaultNumTopN.Load() as usize;
    for (count, n, want) in [
        (active + 5, active, 0),
        (active, active, active),
        (4, 20, 4),
        (5, 3, 3),
    ] {
        let h = hist(
            &(0..count)
                .map(|i| ((i * 10 + 1) as i64, (i * 10 + 6) as i64, 4, 1))
                .collect::<Vec<_>>(),
        );
        let (t, g) = merge(&[Some(h)], &[], n as u32, 128);
        assert_eq!(t.as_ref().map_or(0, TopN::Num), want);
        assert_eq!(
            g.NotNullCount() + t.as_ref().map_or(0, TopN::TotalCount) as f64,
            (count * 4) as f64
        );
    }
}

fn type_matrix() -> Vec<(Box<types::FieldType>, types::Datum, types::Datum)> {
    let mut out = Vec::new();
    for tp in [
        types::mysql::TypeTiny,
        types::mysql::TypeShort,
        types::mysql::TypeInt24,
        types::mysql::TypeLong,
        types::mysql::TypeLonglong,
    ] {
        out.push((
            types::NewFieldType(tp),
            types::NewIntDatum(-3),
            types::NewIntDatum(7),
        ));
        let mut ft = types::NewFieldType(tp);
        ft.AddFlag(types::mysql::UnsignedFlag);
        out.push((ft, types::NewUintDatum(3), types::NewUintDatum(7)));
    }
    out.push((
        types::NewFieldType(types::mysql::TypeYear),
        types::NewIntDatum(2001),
        types::NewIntDatum(2020),
    ));
    out.push((
        types::NewFieldType(types::mysql::TypeFloat),
        types::NewFloat32Datum(-1.5),
        types::NewFloat32Datum(2.5),
    ));
    out.push((
        types::NewFieldType(types::mysql::TypeDouble),
        types::NewFloat64Datum(-1.5),
        types::NewFloat64Datum(2.5),
    ));
    for tp in [
        types::mysql::TypeVarchar,
        types::mysql::TypeString,
        types::mysql::TypeVarString,
        types::mysql::TypeBlob,
        types::mysql::TypeTinyBlob,
        types::mysql::TypeMediumBlob,
        types::mysql::TypeLongBlob,
    ] {
        let mut ft = types::NewFieldType(tp);
        ft.SetCollate(types::charset::CollationBin.to_owned());
        out.push((
            ft,
            types::NewBytesDatum(b"aaa".to_vec()),
            types::NewBytesDatum(b"zzz".to_vec()),
        ));
    }
    let mut ft = types::NewFieldType(types::mysql::TypeVarchar);
    ft.SetCollate("utf8mb4_general_ci".to_owned());
    out.push((
        ft,
        types::NewBytesDatum(collate::GetCollator("utf8mb4_general_ci").Key("AAA")),
        types::NewBytesDatum(collate::GetCollator("utf8mb4_general_ci").Key("zzz")),
    ));
    for tp in [types::mysql::TypeEnum, types::mysql::TypeSet] {
        let mut ft = types::NewFieldType(tp);
        ft.SetElems(vec!["z".to_owned(), "a".to_owned()]);
        let mut lo = types::Datum::default();
        let mut hi = types::Datum::default();
        if tp == types::mysql::TypeEnum {
            lo.SetMysqlEnum(
                types::Enum {
                    Name: "a".to_owned(),
                    Value: 2,
                },
                "utf8mb4_bin".to_owned(),
            );
            hi.SetMysqlEnum(
                types::Enum {
                    Name: "z".to_owned(),
                    Value: 1,
                },
                "utf8mb4_bin".to_owned(),
            );
        } else {
            lo.SetMysqlSet(
                types::Set {
                    Name: "a".to_owned(),
                    Value: 2,
                },
                "utf8mb4_bin".to_owned(),
            );
            hi.SetMysqlSet(
                types::Set {
                    Name: "z".to_owned(),
                    Value: 1,
                },
                "utf8mb4_bin".to_owned(),
            );
        }
        out.push((ft, lo, hi));
    }
    for low in [1, 256] {
        let mut ft = types::NewFieldType(types::mysql::TypeBit);
        ft.SetFlen(16);
        let mut lo = types::Datum::default();
        let mut hi = types::Datum::default();
        lo.SetMysqlBit(types::NewBinaryLiteralFromUint(low, 2));
        hi.SetMysqlBit(types::NewBinaryLiteralFromUint(2, 2));
        out.push((ft, lo, hi));
    }
    for tp in [
        types::mysql::TypeDate,
        types::mysql::TypeDatetime,
        types::mysql::TypeTimestamp,
    ] {
        let ctx = types::BasicTimeContext {
            flags: types::TimeFlags::default(),
            location: codec::time::UTC,
        };
        let lo = types::ParseTime(&ctx, "2001-01-01 00:00:00", tp, 0).unwrap();
        let hi = types::ParseTime(&ctx, "2020-06-06 00:00:00", tp, 0).unwrap();
        out.push((
            types::NewFieldType(tp),
            types::NewTimeDatum(lo),
            types::NewTimeDatum(hi),
        ));
    }
    let ft = types::NewFieldType(types::mysql::TypeDuration);
    out.push((
        ft,
        types::NewDurationDatum(types::Duration {
            Duration: 3_600_000_000_000,
            Fsp: 0,
        }),
        types::NewDurationDatum(types::Duration {
            Duration: 18_000_000_000_000,
            Fsp: 0,
        }),
    ));
    let mut ft = types::NewFieldType(types::mysql::TypeNewDecimal);
    ft.SetFlen(10);
    ft.SetDecimal(0);
    let mut lo = types::Datum::default();
    let mut hi = types::Datum::default();
    lo.SetMysqlDecimal({
        let mut d = types::MyDecimal::default();
        d.FromInt(-3);
        d
    });
    hi.SetMysqlDecimal({
        let mut d = types::MyDecimal::default();
        d.FromInt(7);
        d
    });
    lo.SetLength(10);
    hi.SetLength(10);
    lo.SetFrac(0);
    hi.SetFrac(0);
    out.push((ft, lo, hi));
    out
}
#[test]
fn combined_merge_aggregates_and_rebuilds_across_column_types() {
    let sc = stmtctx::NewStmtCtx();
    let killer = sqlkiller::sqlkiller::SQLKiller::default();
    for (ft, lo, hi) in type_matrix() {
        let encode =
            |d: types::Datum| codec::EncodeKey(codec::time::UTC, Vec::new(), vec![d]).unwrap();
        let lo_key = encode(lo.clone());
        let hi_key = encode(hi.clone());
        let mut t = NewTopN(2);
        t.AppendTopN(lo_key.clone(), 5);
        t.AppendTopN(hi_key.clone(), 4);
        t.Sort();
        let empty = NewHistogram(1, 2, 0, 0, &ft, 0, 0);
        let mut h = empty.clone();
        h.AppendBucket(&lo, &lo, 3, 3);
        h.AppendBucket(&hi, &hi, 10, 7);
        let (global, g) = MergePartTopNAndHistToGlobal(
            &sc,
            &killer,
            &[Some(t)],
            &[Some(empty.clone()), Some(h)],
            4,
            2,
            false,
        )
        .unwrap();
        let global = global.unwrap();
        assert_eq!(
            global.QueryTopN(&lo_key),
            (8, true),
            "type {}",
            ft.GetType()
        );
        assert_eq!(global.QueryTopN(&hi_key), (11, true));
        assert_eq!(global.Num(), 2);
        assert_eq!(g.NotNullCount(), 0.0);
        let mut t = NewTopN(1);
        t.AppendTopN(lo_key, 5);
        let mut h = empty.clone();
        h.AppendBucket(&hi, &hi, 10, 10);
        let (global, g) = MergePartTopNAndHistToGlobal(
            &sc,
            &killer,
            &[Some(t)],
            &[Some(empty), Some(h)],
            1,
            2,
            false,
        )
        .unwrap();
        assert_eq!(global.unwrap().TotalCount(), 10);
        assert_eq!(g.NotNullCount(), 5.0);
        assert_eq!(g.Len(), 1);
    }
}
#[test]
fn combined_merge_index_path_compares_encoded_keys() {
    let sc = stmtctx::NewStmtCtx();
    let killer = sqlkiller::sqlkiller::SQLKiller::default();
    let ft = types::NewFieldType(types::mysql::TypeBlob);
    for (_, lo, hi) in type_matrix() {
        let encode =
            |d: types::Datum| codec::EncodeKey(codec::time::UTC, Vec::new(), vec![d]).unwrap();
        let (mut lo, mut hi) = (encode(lo), encode(hi));
        if lo > hi {
            std::mem::swap(&mut lo, &mut hi);
        }
        let mut t = NewTopN(1);
        t.AppendTopN(lo.clone(), 5);
        let empty = NewHistogram(1, 2, 0, 0, &ft, 0, 0);
        let mut h = empty.clone();
        let lower = types::NewBytesDatum(lo.clone());
        let upper = types::NewBytesDatum(hi);
        h.AppendBucket(&lower, &lower, 3, 3);
        h.AppendBucket(&upper, &upper, 10, 7);
        let (t, _) = MergePartTopNAndHistToGlobal(
            &sc,
            &killer,
            &[Some(t)],
            &[Some(empty), Some(h)],
            3,
            2,
            true,
        )
        .unwrap();
        assert_eq!(t.unwrap().QueryTopN(&lo), (8, true));
    }
}

// Declarative fixtures from Go merge_global_cases_test.go; optimizer-specific
// callbacks remain to be verified in the owning cardinality crate.
#[test]
fn combined_case_disjoint_partition_ranges_keep_gap() {
    let sc = stmtctx::NewStmtCtx();
    let killer = sqlkiller::sqlkiller::SQLKiller::default();
    let is_index = false;
    let bound = |d: types::Datum| {
        if is_index {
            types::NewBytesDatum(codec::EncodeKey(codec::time::UTC, Vec::new(), vec![d]).unwrap())
        } else {
            d
        }
    };
    let ft = types::NewFieldType(if is_index {
        types::mysql::TypeBlob
    } else {
        types::mysql::TypeLong
    });
    let mut hists = Vec::new();
    let mut tops = Vec::new();
    let mut total = 0;
    let mut h = NewHistogram(1, 0, 0, 0, &ft, 0, 0);
    let mut cum = 0;
    let mut t = NewTopN(0);
    cum += 3;
    h.AppendBucket(
        &bound(types::NewIntDatum(1)),
        &bound(types::NewIntDatum(5)),
        cum,
        1,
    );
    total += 3;
    cum += 5;
    h.AppendBucket(
        &bound(types::NewIntDatum(6)),
        &bound(types::NewIntDatum(10)),
        cum,
        1,
    );
    total += 5;
    hists.push(Some(h));
    tops.push(Some(t));
    let mut h = NewHistogram(1, 0, 0, 0, &ft, 0, 0);
    let mut cum = 0;
    let mut t = NewTopN(0);
    cum += 3;
    h.AppendBucket(
        &bound(types::NewIntDatum(100)),
        &bound(types::NewIntDatum(105)),
        cum,
        1,
    );
    total += 3;
    cum += 5;
    h.AppendBucket(
        &bound(types::NewIntDatum(106)),
        &bound(types::NewIntDatum(110)),
        cum,
        1,
    );
    total += 5;
    hists.push(Some(h));
    tops.push(Some(t));
    let (top, g) =
        MergePartTopNAndHistToGlobal(&sc, &killer, &tops, &hists, 0, 2, is_index).unwrap();
    assert_eq!(
        g.NotNullCount() as i64 + top.as_ref().map_or(0, TopN::TotalCount) as i64,
        total
    );
    assert!(g.Len() <= 2);
    assert_merge_invariants(&sc, &g, top.as_ref(), is_index);
    assert_eq!(g.Len(), 2);
    let mut expected = 0;
    expected += 8;
    assert_eq!(g.Buckets[0].Count, expected);
    assert_eq!(
        g.GetLower(0)
            .Compare(
                sc.TypeCtx(),
                &types::NewIntDatum(1),
                collate::GetBinaryCollator().as_ref()
            )
            .unwrap(),
        0
    );
    assert_eq!(
        g.GetUpper(0)
            .Compare(
                sc.TypeCtx(),
                &types::NewIntDatum(10),
                collate::GetBinaryCollator().as_ref()
            )
            .unwrap(),
        0
    );
    expected += 8;
    assert_eq!(g.Buckets[1].Count, expected);
    assert_eq!(
        g.GetLower(1)
            .Compare(
                sc.TypeCtx(),
                &types::NewIntDatum(100),
                collate::GetBinaryCollator().as_ref()
            )
            .unwrap(),
        0
    );
    assert_eq!(
        g.GetUpper(1)
            .Compare(
                sc.TypeCtx(),
                &types::NewIntDatum(110),
                collate::GetBinaryCollator().as_ref()
            )
            .unwrap(),
        0
    );
}
#[test]
fn combined_case_single_partition_collapses_to_equidepth() {
    let sc = stmtctx::NewStmtCtx();
    let killer = sqlkiller::sqlkiller::SQLKiller::default();
    let is_index = false;
    let bound = |d: types::Datum| {
        if is_index {
            types::NewBytesDatum(codec::EncodeKey(codec::time::UTC, Vec::new(), vec![d]).unwrap())
        } else {
            d
        }
    };
    let ft = types::NewFieldType(if is_index {
        types::mysql::TypeBlob
    } else {
        types::mysql::TypeLong
    });
    let mut hists = Vec::new();
    let mut tops = Vec::new();
    let mut total = 0;
    let mut h = NewHistogram(1, 0, 0, 0, &ft, 0, 0);
    let mut cum = 0;
    let mut t = NewTopN(0);
    cum += 10;
    h.AppendBucket(
        &bound(types::NewIntDatum(1)),
        &bound(types::NewIntDatum(10)),
        cum,
        1,
    );
    total += 10;
    cum += 10;
    h.AppendBucket(
        &bound(types::NewIntDatum(11)),
        &bound(types::NewIntDatum(20)),
        cum,
        1,
    );
    total += 10;
    cum += 10;
    h.AppendBucket(
        &bound(types::NewIntDatum(21)),
        &bound(types::NewIntDatum(30)),
        cum,
        1,
    );
    total += 10;
    cum += 10;
    h.AppendBucket(
        &bound(types::NewIntDatum(31)),
        &bound(types::NewIntDatum(40)),
        cum,
        1,
    );
    total += 10;
    cum += 10;
    h.AppendBucket(
        &bound(types::NewIntDatum(41)),
        &bound(types::NewIntDatum(50)),
        cum,
        1,
    );
    total += 10;
    cum += 10;
    h.AppendBucket(
        &bound(types::NewIntDatum(51)),
        &bound(types::NewIntDatum(60)),
        cum,
        1,
    );
    total += 10;
    hists.push(Some(h));
    tops.push(Some(t));
    let (top, g) =
        MergePartTopNAndHistToGlobal(&sc, &killer, &tops, &hists, 0, 2, is_index).unwrap();
    assert_eq!(
        g.NotNullCount() as i64 + top.as_ref().map_or(0, TopN::TotalCount) as i64,
        total
    );
    assert!(g.Len() <= 2);
    assert_merge_invariants(&sc, &g, top.as_ref(), is_index);
    assert_eq!(g.Len(), 2);
    let mut expected = 0;
    expected += 30;
    assert_eq!(g.Buckets[0].Count, expected);
    assert_eq!(
        g.GetLower(0)
            .Compare(
                sc.TypeCtx(),
                &types::NewIntDatum(1),
                collate::GetBinaryCollator().as_ref()
            )
            .unwrap(),
        0
    );
    assert_eq!(
        g.GetUpper(0)
            .Compare(
                sc.TypeCtx(),
                &types::NewIntDatum(30),
                collate::GetBinaryCollator().as_ref()
            )
            .unwrap(),
        0
    );
    expected += 30;
    assert_eq!(g.Buckets[1].Count, expected);
    assert_eq!(
        g.GetLower(1)
            .Compare(
                sc.TypeCtx(),
                &types::NewIntDatum(31),
                collate::GetBinaryCollator().as_ref()
            )
            .unwrap(),
        0
    );
    assert_eq!(
        g.GetUpper(1)
            .Compare(
                sc.TypeCtx(),
                &types::NewIntDatum(60),
                collate::GetBinaryCollator().as_ref()
            )
            .unwrap(),
        0
    );
}
#[test]
fn combined_case_disjoint_with_gap_after_filled_bucket() {
    let sc = stmtctx::NewStmtCtx();
    let killer = sqlkiller::sqlkiller::SQLKiller::default();
    let is_index = false;
    let bound = |d: types::Datum| {
        if is_index {
            types::NewBytesDatum(codec::EncodeKey(codec::time::UTC, Vec::new(), vec![d]).unwrap())
        } else {
            d
        }
    };
    let ft = types::NewFieldType(if is_index {
        types::mysql::TypeBlob
    } else {
        types::mysql::TypeLong
    });
    let mut hists = Vec::new();
    let mut tops = Vec::new();
    let mut total = 0;
    let mut h = NewHistogram(1, 0, 0, 0, &ft, 0, 0);
    let mut cum = 0;
    let mut t = NewTopN(0);
    cum += 4;
    h.AppendBucket(
        &bound(types::NewIntDatum(8)),
        &bound(types::NewIntDatum(15)),
        cum,
        1,
    );
    total += 4;
    hists.push(Some(h));
    tops.push(Some(t));
    let mut h = NewHistogram(1, 0, 0, 0, &ft, 0, 0);
    let mut cum = 0;
    let mut t = NewTopN(0);
    cum += 7;
    h.AppendBucket(
        &bound(types::NewIntDatum(28)),
        &bound(types::NewIntDatum(49)),
        cum,
        1,
    );
    total += 7;
    hists.push(Some(h));
    tops.push(Some(t));
    let (top, g) =
        MergePartTopNAndHistToGlobal(&sc, &killer, &tops, &hists, 0, 4, is_index).unwrap();
    assert_eq!(
        g.NotNullCount() as i64 + top.as_ref().map_or(0, TopN::TotalCount) as i64,
        total
    );
    assert!(g.Len() <= 4);
    assert_merge_invariants(&sc, &g, top.as_ref(), is_index);
    assert_eq!(g.Len(), 2);
    let mut expected = 0;
    expected += 4;
    assert_eq!(g.Buckets[0].Count, expected);
    assert_eq!(
        g.GetLower(0)
            .Compare(
                sc.TypeCtx(),
                &types::NewIntDatum(8),
                collate::GetBinaryCollator().as_ref()
            )
            .unwrap(),
        0
    );
    assert_eq!(
        g.GetUpper(0)
            .Compare(
                sc.TypeCtx(),
                &types::NewIntDatum(15),
                collate::GetBinaryCollator().as_ref()
            )
            .unwrap(),
        0
    );
    expected += 7;
    assert_eq!(g.Buckets[1].Count, expected);
    assert_eq!(
        g.GetLower(1)
            .Compare(
                sc.TypeCtx(),
                &types::NewIntDatum(28),
                collate::GetBinaryCollator().as_ref()
            )
            .unwrap(),
        0
    );
    assert_eq!(
        g.GetUpper(1)
            .Compare(
                sc.TypeCtx(),
                &types::NewIntDatum(49),
                collate::GetBinaryCollator().as_ref()
            )
            .unwrap(),
        0
    );
}
#[test]
fn combined_case_three_disjoint_growing_partitions() {
    let sc = stmtctx::NewStmtCtx();
    let killer = sqlkiller::sqlkiller::SQLKiller::default();
    let is_index = false;
    let bound = |d: types::Datum| {
        if is_index {
            types::NewBytesDatum(codec::EncodeKey(codec::time::UTC, Vec::new(), vec![d]).unwrap())
        } else {
            d
        }
    };
    let ft = types::NewFieldType(if is_index {
        types::mysql::TypeBlob
    } else {
        types::mysql::TypeLong
    });
    let mut hists = Vec::new();
    let mut tops = Vec::new();
    let mut total = 0;
    let mut h = NewHistogram(1, 0, 0, 0, &ft, 0, 0);
    let mut cum = 0;
    let mut t = NewTopN(0);
    cum += 5;
    h.AppendBucket(
        &bound(types::NewIntDatum(1)),
        &bound(types::NewIntDatum(10)),
        cum,
        1,
    );
    total += 5;
    hists.push(Some(h));
    tops.push(Some(t));
    let mut h = NewHistogram(1, 0, 0, 0, &ft, 0, 0);
    let mut cum = 0;
    let mut t = NewTopN(0);
    cum += 11;
    h.AppendBucket(
        &bound(types::NewIntDatum(12)),
        &bound(types::NewIntDatum(20)),
        cum,
        1,
    );
    total += 11;
    hists.push(Some(h));
    tops.push(Some(t));
    let mut h = NewHistogram(1, 0, 0, 0, &ft, 0, 0);
    let mut cum = 0;
    let mut t = NewTopN(0);
    cum += 14;
    h.AppendBucket(
        &bound(types::NewIntDatum(22)),
        &bound(types::NewIntDatum(30)),
        cum,
        1,
    );
    total += 14;
    hists.push(Some(h));
    tops.push(Some(t));
    let (top, g) =
        MergePartTopNAndHistToGlobal(&sc, &killer, &tops, &hists, 0, 3, is_index).unwrap();
    assert_eq!(
        g.NotNullCount() as i64 + top.as_ref().map_or(0, TopN::TotalCount) as i64,
        total
    );
    assert!(g.Len() <= 3);
    assert_merge_invariants(&sc, &g, top.as_ref(), is_index);
}
#[test]
fn combined_case_overlapping_three_bucket_partitions() {
    let sc = stmtctx::NewStmtCtx();
    let killer = sqlkiller::sqlkiller::SQLKiller::default();
    let is_index = false;
    let bound = |d: types::Datum| {
        if is_index {
            types::NewBytesDatum(codec::EncodeKey(codec::time::UTC, Vec::new(), vec![d]).unwrap())
        } else {
            d
        }
    };
    let ft = types::NewFieldType(if is_index {
        types::mysql::TypeBlob
    } else {
        types::mysql::TypeLong
    });
    let mut hists = Vec::new();
    let mut tops = Vec::new();
    let mut total = 0;
    let mut h = NewHistogram(1, 0, 0, 0, &ft, 0, 0);
    let mut cum = 0;
    let mut t = NewTopN(0);
    cum += 5;
    h.AppendBucket(
        &bound(types::NewIntDatum(1)),
        &bound(types::NewIntDatum(10)),
        cum,
        1,
    );
    total += 5;
    cum += 8;
    h.AppendBucket(
        &bound(types::NewIntDatum(12)),
        &bound(types::NewIntDatum(30)),
        cum,
        1,
    );
    total += 8;
    cum += 6;
    h.AppendBucket(
        &bound(types::NewIntDatum(32)),
        &bound(types::NewIntDatum(50)),
        cum,
        1,
    );
    total += 6;
    hists.push(Some(h));
    tops.push(Some(t));
    let mut h = NewHistogram(1, 0, 0, 0, &ft, 0, 0);
    let mut cum = 0;
    let mut t = NewTopN(0);
    cum += 4;
    h.AppendBucket(
        &bound(types::NewIntDatum(15)),
        &bound(types::NewIntDatum(20)),
        cum,
        1,
    );
    total += 4;
    cum += 7;
    h.AppendBucket(
        &bound(types::NewIntDatum(25)),
        &bound(types::NewIntDatum(40)),
        cum,
        1,
    );
    total += 7;
    cum += 5;
    h.AppendBucket(
        &bound(types::NewIntDatum(45)),
        &bound(types::NewIntDatum(60)),
        cum,
        1,
    );
    total += 5;
    hists.push(Some(h));
    tops.push(Some(t));
    let (top, g) =
        MergePartTopNAndHistToGlobal(&sc, &killer, &tops, &hists, 0, 4, is_index).unwrap();
    assert_eq!(
        g.NotNullCount() as i64 + top.as_ref().map_or(0, TopN::TotalCount) as i64,
        total
    );
    assert!(g.Len() <= 4);
    assert_merge_invariants(&sc, &g, top.as_ref(), is_index);
}
#[test]
fn combined_case_boundary_repeat_stays_with_left_bucket() {
    let sc = stmtctx::NewStmtCtx();
    let killer = sqlkiller::sqlkiller::SQLKiller::default();
    let is_index = false;
    let bound = |d: types::Datum| {
        if is_index {
            types::NewBytesDatum(codec::EncodeKey(codec::time::UTC, Vec::new(), vec![d]).unwrap())
        } else {
            d
        }
    };
    let ft = types::NewFieldType(if is_index {
        types::mysql::TypeBlob
    } else {
        types::mysql::TypeLong
    });
    let mut hists = Vec::new();
    let mut tops = Vec::new();
    let mut total = 0;
    let mut h = NewHistogram(1, 0, 0, 0, &ft, 0, 0);
    let mut cum = 0;
    let mut t = NewTopN(0);
    cum += 100;
    h.AppendBucket(
        &bound(types::NewIntDatum(0)),
        &bound(types::NewIntDatum(10)),
        cum,
        20,
    );
    total += 100;
    hists.push(Some(h));
    tops.push(Some(t));
    let mut h = NewHistogram(1, 0, 0, 0, &ft, 0, 0);
    let mut cum = 0;
    let mut t = NewTopN(0);
    cum += 100;
    h.AppendBucket(
        &bound(types::NewIntDatum(10)),
        &bound(types::NewIntDatum(20)),
        cum,
        1,
    );
    total += 100;
    hists.push(Some(h));
    tops.push(Some(t));
    let (top, g) =
        MergePartTopNAndHistToGlobal(&sc, &killer, &tops, &hists, 0, 2, is_index).unwrap();
    assert_eq!(
        g.NotNullCount() as i64 + top.as_ref().map_or(0, TopN::TotalCount) as i64,
        total
    );
    assert!(g.Len() <= 2);
    assert_merge_invariants(&sc, &g, top.as_ref(), is_index);
    assert_eq!(g.Len(), 2);
    let mut expected = 0;
    expected += 100;
    assert_eq!(g.Buckets[0].Count, expected);
    assert_eq!(
        g.GetLower(0)
            .Compare(
                sc.TypeCtx(),
                &types::NewIntDatum(0),
                collate::GetBinaryCollator().as_ref()
            )
            .unwrap(),
        0
    );
    assert_eq!(
        g.GetUpper(0)
            .Compare(
                sc.TypeCtx(),
                &types::NewIntDatum(10),
                collate::GetBinaryCollator().as_ref()
            )
            .unwrap(),
        0
    );
    expected += 100;
    assert_eq!(g.Buckets[1].Count, expected);
    assert_eq!(
        g.GetLower(1)
            .Compare(
                sc.TypeCtx(),
                &types::NewIntDatum(10),
                collate::GetBinaryCollator().as_ref()
            )
            .unwrap(),
        0
    );
    assert_eq!(
        g.GetUpper(1)
            .Compare(
                sc.TypeCtx(),
                &types::NewIntDatum(20),
                collate::GetBinaryCollator().as_ref()
            )
            .unwrap(),
        0
    );
}
#[test]
fn combined_case_cut_boundary_has_no_observed_repeat() {
    let sc = stmtctx::NewStmtCtx();
    let killer = sqlkiller::sqlkiller::SQLKiller::default();
    let is_index = false;
    let bound = |d: types::Datum| {
        if is_index {
            types::NewBytesDatum(codec::EncodeKey(codec::time::UTC, Vec::new(), vec![d]).unwrap())
        } else {
            d
        }
    };
    let ft = types::NewFieldType(if is_index {
        types::mysql::TypeBlob
    } else {
        types::mysql::TypeLong
    });
    let mut hists = Vec::new();
    let mut tops = Vec::new();
    let mut total = 0;
    let mut h = NewHistogram(1, 0, 0, 0, &ft, 0, 0);
    let mut cum = 0;
    let mut t = NewTopN(0);
    cum += 100;
    h.AppendBucket(
        &bound(types::NewIntDatum(1)),
        &bound(types::NewIntDatum(100)),
        cum,
        1,
    );
    total += 100;
    hists.push(Some(h));
    tops.push(Some(t));
    let mut h = NewHistogram(1, 0, 0, 0, &ft, 0, 0);
    let mut cum = 0;
    let mut t = NewTopN(0);
    cum += 100;
    h.AppendBucket(
        &bound(types::NewIntDatum(90)),
        &bound(types::NewIntDatum(110)),
        cum,
        1,
    );
    total += 100;
    hists.push(Some(h));
    tops.push(Some(t));
    let (top, g) =
        MergePartTopNAndHistToGlobal(&sc, &killer, &tops, &hists, 0, 2, is_index).unwrap();
    assert_eq!(
        g.NotNullCount() as i64 + top.as_ref().map_or(0, TopN::TotalCount) as i64,
        total
    );
    assert!(g.Len() <= 2);
    assert_merge_invariants(&sc, &g, top.as_ref(), is_index);
    assert_eq!(g.Len(), 2);
    let mut expected = 0;
    expected += 90;
    assert_eq!(g.Buckets[0].Count, expected);
    assert_eq!(
        g.GetLower(0)
            .Compare(
                sc.TypeCtx(),
                &types::NewIntDatum(1),
                collate::GetBinaryCollator().as_ref()
            )
            .unwrap(),
        0
    );
    assert_eq!(
        g.GetUpper(0)
            .Compare(
                sc.TypeCtx(),
                &types::NewIntDatum(90),
                collate::GetBinaryCollator().as_ref()
            )
            .unwrap(),
        0
    );
    expected += 110;
    assert_eq!(g.Buckets[1].Count, expected);
    assert_eq!(
        g.GetLower(1)
            .Compare(
                sc.TypeCtx(),
                &types::NewIntDatum(90),
                collate::GetBinaryCollator().as_ref()
            )
            .unwrap(),
        0
    );
    assert_eq!(
        g.GetUpper(1)
            .Compare(
                sc.TypeCtx(),
                &types::NewIntDatum(110),
                collate::GetBinaryCollator().as_ref()
            )
            .unwrap(),
        0
    );
}
#[test]
fn combined_case_repeat_never_exceeds_bucket_mass() {
    let sc = stmtctx::NewStmtCtx();
    let killer = sqlkiller::sqlkiller::SQLKiller::default();
    let is_index = false;
    let bound = |d: types::Datum| {
        if is_index {
            types::NewBytesDatum(codec::EncodeKey(codec::time::UTC, Vec::new(), vec![d]).unwrap())
        } else {
            d
        }
    };
    let ft = types::NewFieldType(if is_index {
        types::mysql::TypeBlob
    } else {
        types::mysql::TypeLong
    });
    let mut hists = Vec::new();
    let mut tops = Vec::new();
    let mut total = 0;
    let mut h = NewHistogram(1, 0, 0, 0, &ft, 0, 0);
    let mut cum = 0;
    let mut t = NewTopN(0);
    cum += 2;
    h.AppendBucket(
        &bound(types::NewIntDatum(1)),
        &bound(types::NewIntDatum(10)),
        cum,
        1,
    );
    total += 2;
    hists.push(Some(h));
    tops.push(Some(t));
    let mut h = NewHistogram(1, 0, 0, 0, &ft, 0, 0);
    let mut cum = 0;
    let mut t = NewTopN(0);
    cum += 100;
    h.AppendBucket(
        &bound(types::NewIntDatum(5)),
        &bound(types::NewIntDatum(100)),
        cum,
        50,
    );
    total += 100;
    hists.push(Some(h));
    tops.push(Some(t));
    let mut h = NewHistogram(1, 0, 0, 0, &ft, 0, 0);
    let mut cum = 0;
    let mut t = NewTopN(0);
    cum += 100;
    h.AppendBucket(
        &bound(types::NewIntDatum(5)),
        &bound(types::NewIntDatum(100)),
        cum,
        50,
    );
    total += 100;
    hists.push(Some(h));
    tops.push(Some(t));
    let (top, g) =
        MergePartTopNAndHistToGlobal(&sc, &killer, &tops, &hists, 0, 2, is_index).unwrap();
    assert_eq!(
        g.NotNullCount() as i64 + top.as_ref().map_or(0, TopN::TotalCount) as i64,
        total
    );
    assert!(g.Len() <= 2);
    assert_merge_invariants(&sc, &g, top.as_ref(), is_index);
    assert_eq!(g.Len(), 2);
    let mut expected = 0;
    expected += 1;
    assert_eq!(g.Buckets[0].Count, expected);
    assert_eq!(
        g.GetLower(0)
            .Compare(
                sc.TypeCtx(),
                &types::NewIntDatum(1),
                collate::GetBinaryCollator().as_ref()
            )
            .unwrap(),
        0
    );
    assert_eq!(
        g.GetUpper(0)
            .Compare(
                sc.TypeCtx(),
                &types::NewIntDatum(5),
                collate::GetBinaryCollator().as_ref()
            )
            .unwrap(),
        0
    );
    expected += 201;
    assert_eq!(g.Buckets[1].Count, expected);
    assert_eq!(
        g.GetLower(1)
            .Compare(
                sc.TypeCtx(),
                &types::NewIntDatum(5),
                collate::GetBinaryCollator().as_ref()
            )
            .unwrap(),
        0
    );
    assert_eq!(
        g.GetUpper(1)
            .Compare(
                sc.TypeCtx(),
                &types::NewIntDatum(100),
                collate::GetBinaryCollator().as_ref()
            )
            .unwrap(),
        0
    );
}
#[test]
fn combined_case_hot_value_at_upper_across_partitions() {
    let h = (0..8)
        .map(|p| {
            Some(hist(&[
                (p * 10 + 1, p * 10 + 10, 50, 0),
                (500, 500, 200, 200),
            ]))
        })
        .collect::<Vec<_>>();
    let (t, g) = merge(&h, &[], 0, 10);
    assert_merge_invariants(&stmtctx::NewStmtCtx(), &g, t.as_ref(), false);
    assert_eq!(g.NotNullCount(), 2000.0);
    let i = (0..g.Len())
        .find(|&i| g.GetUpper(i).GetInt64() == 500)
        .unwrap();
    assert!(g.BucketCount(i) >= 1600);
}
#[test]
fn combined_case_spread_value_repeat_extraction() {
    let mut h = Vec::new();
    let mut t = Vec::new();
    for i in 0..20 {
        if i < 2 {
            h.push(Some(hist(&[
                (1, 20, 60, 8),
                (21, 49, 60, 6),
                (51, 100, 80, 10),
                (101, 199, 80, 12),
            ])));
            t.push(Some(top(&[(50, 500), (200, 100)])));
        } else {
            h.push(Some(hist(&[
                (1, 20, 80, 10),
                (21, 50, 120, 80),
                (51, 80, 80, 15),
                (81, 100, 70, 12),
            ])));
            t.push(Some(top(&[(300, 90), (200, 85)])));
        }
    }
    let (t, g) = merge(&h, &t, 2, 100);
    let t = t.unwrap();
    assert_eq!(t.QueryTopN(&key(50)), (2440, true));
    assert_eq!(t.QueryTopN(&key(200)), (1730, true));
    assert_eq!(t.TotalCount() as f64 + g.NotNullCount(), 11210.0);
    assert_merge_invariants(&stmtctx::NewStmtCtx(), &g, Some(&t), false);
}
#[test]
fn combined_case_inside_bucket_value_not_inflated() {
    let mut h = Vec::new();
    let mut t = Vec::new();
    for i in 0..20 {
        if i < 2 {
            h.push(Some(hist(&[
                (1, 25, 50, 5),
                (26, 49, 50, 6),
                (51, 100, 80, 10),
                (101, 199, 70, 12),
            ])));
            t.push(Some(top(&[(50, 60), (200, 40)])));
        } else {
            h.push(Some(hist(&[
                (1, 25, 100, 10),
                (26, 55, 150, 20),
                (56, 80, 130, 15),
                (81, 100, 120, 12),
            ])));
            t.push(Some(top(&[(200, 40), (300, 35)])));
        }
    }
    let (t, g) = merge(&h, &t, 2, 100);
    let t = t.unwrap();
    assert_eq!(t.QueryTopN(&key(200)), (800, true));
    assert_eq!(t.QueryTopN(&key(300)), (630, true));
    assert_eq!(t.TotalCount() as f64 + g.NotNullCount(), 11050.0);
    assert_merge_invariants(&stmtctx::NewStmtCtx(), &g, Some(&t), false);
}
#[test]
fn combined_case_topn_sorted_by_encoded_bytes() {
    let sc = stmtctx::NewStmtCtx();
    let killer = sqlkiller::sqlkiller::SQLKiller::default();
    let is_index = false;
    let bound = |d: types::Datum| {
        if is_index {
            types::NewBytesDatum(codec::EncodeKey(codec::time::UTC, Vec::new(), vec![d]).unwrap())
        } else {
            d
        }
    };
    let ft = types::NewFieldType(if is_index {
        types::mysql::TypeBlob
    } else {
        types::mysql::TypeLong
    });
    let mut hists = Vec::new();
    let mut tops = Vec::new();
    let mut total = 0;
    let mut h = NewHistogram(1, 0, 0, 0, &ft, 0, 0);
    let mut cum = 0;
    let mut t = NewTopN(0);
    cum += 1;
    h.AppendBucket(
        &bound(types::NewIntDatum(100)),
        &bound(types::NewIntDatum(200)),
        cum,
        0,
    );
    total += 1;
    t.AppendTopN(
        codec::EncodeKey(codec::time::UTC, Vec::new(), vec![types::NewIntDatum(5)]).unwrap(),
        10,
    );
    total += 10;
    t.AppendTopN(
        codec::EncodeKey(codec::time::UTC, Vec::new(), vec![types::NewIntDatum(10)]).unwrap(),
        20,
    );
    total += 20;
    t.AppendTopN(
        codec::EncodeKey(codec::time::UTC, Vec::new(), vec![types::NewIntDatum(15)]).unwrap(),
        5,
    );
    total += 5;
    hists.push(Some(h));
    tops.push(Some(t));
    let (top, g) =
        MergePartTopNAndHistToGlobal(&sc, &killer, &tops, &hists, 3, 100, is_index).unwrap();
    assert_eq!(
        g.NotNullCount() as i64 + top.as_ref().map_or(0, TopN::TotalCount) as i64,
        total
    );
    assert!(g.Len() <= 100);
    assert_merge_invariants(&sc, &g, top.as_ref(), is_index);
    let t = top.unwrap();
    assert_eq!(t.Num(), 3);
    assert_eq!(
        t.QueryTopN(
            &codec::EncodeKey(codec::time::UTC, Vec::new(), vec![types::NewIntDatum(5)]).unwrap()
        ),
        (10, true)
    );
    assert_eq!(
        t.QueryTopN(
            &codec::EncodeKey(codec::time::UTC, Vec::new(), vec![types::NewIntDatum(10)]).unwrap()
        ),
        (20, true)
    );
    assert_eq!(
        t.QueryTopN(
            &codec::EncodeKey(codec::time::UTC, Vec::new(), vec![types::NewIntDatum(15)]).unwrap()
        ),
        (5, true)
    );
}
#[test]
fn combined_case_topn_subtraction_leaves_no_zero_mass_bucket() {
    let sc = stmtctx::NewStmtCtx();
    let killer = sqlkiller::sqlkiller::SQLKiller::default();
    let is_index = false;
    let bound = |d: types::Datum| {
        if is_index {
            types::NewBytesDatum(codec::EncodeKey(codec::time::UTC, Vec::new(), vec![d]).unwrap())
        } else {
            d
        }
    };
    let ft = types::NewFieldType(if is_index {
        types::mysql::TypeBlob
    } else {
        types::mysql::TypeLong
    });
    let mut hists = Vec::new();
    let mut tops = Vec::new();
    let mut total = 0;
    let mut h = NewHistogram(1, 0, 0, 0, &ft, 0, 0);
    let mut cum = 0;
    let mut t = NewTopN(0);
    cum += 1;
    h.AppendBucket(
        &bound(types::NewIntDatum(2)),
        &bound(types::NewIntDatum(2)),
        cum,
        1,
    );
    total += 1;
    t.AppendTopN(
        codec::EncodeKey(codec::time::UTC, Vec::new(), vec![types::NewIntDatum(3)]).unwrap(),
        3,
    );
    total += 3;
    hists.push(Some(h));
    tops.push(Some(t));
    let mut h = NewHistogram(1, 0, 0, 0, &ft, 0, 0);
    let mut cum = 0;
    let mut t = NewTopN(0);
    cum += 2;
    h.AppendBucket(
        &bound(types::NewIntDatum(1)),
        &bound(types::NewIntDatum(3)),
        cum,
        1,
    );
    total += 2;
    cum += 1;
    h.AppendBucket(
        &bound(types::NewIntDatum(4)),
        &bound(types::NewIntDatum(4)),
        cum,
        1,
    );
    total += 1;
    t.AppendTopN(
        codec::EncodeKey(codec::time::UTC, Vec::new(), vec![types::NewIntDatum(2)]).unwrap(),
        3,
    );
    total += 3;
    hists.push(Some(h));
    tops.push(Some(t));
    let mut h = NewHistogram(1, 0, 0, 0, &ft, 0, 0);
    let mut cum = 0;
    let mut t = NewTopN(0);
    t.AppendTopN(
        codec::EncodeKey(codec::time::UTC, Vec::new(), vec![types::NewIntDatum(1)]).unwrap(),
        2,
    );
    total += 2;
    hists.push(Some(h));
    tops.push(Some(t));
    let (top, g) =
        MergePartTopNAndHistToGlobal(&sc, &killer, &tops, &hists, 1, 3, is_index).unwrap();
    assert_eq!(
        g.NotNullCount() as i64 + top.as_ref().map_or(0, TopN::TotalCount) as i64,
        total
    );
    assert!(g.Len() <= 3);
    assert_merge_invariants(&sc, &g, top.as_ref(), is_index);
}
#[test]
fn combined_case_varchar_with_binary_collation() {
    let sc = stmtctx::NewStmtCtx();
    let killer = sqlkiller::sqlkiller::SQLKiller::default();
    let is_index = false;
    let bound = |d: types::Datum| {
        if is_index {
            types::NewBytesDatum(codec::EncodeKey(codec::time::UTC, Vec::new(), vec![d]).unwrap())
        } else {
            d
        }
    };
    let ft = types::NewFieldType(if is_index {
        types::mysql::TypeBlob
    } else {
        types::mysql::TypeVarchar
    });
    let mut hists = Vec::new();
    let mut tops = Vec::new();
    let mut total = 0;
    let mut h = NewHistogram(1, 0, 0, 0, &ft, 0, 0);
    let mut cum = 0;
    let mut t = NewTopN(0);
    cum += 4;
    h.AppendBucket(
        &bound(types::NewStringDatum("bbb".to_owned())),
        &bound(types::NewStringDatum("ccc".to_owned())),
        cum,
        1,
    );
    total += 4;
    cum += 6;
    h.AppendBucket(
        &bound(types::NewStringDatum("ddd".to_owned())),
        &bound(types::NewStringDatum("eee".to_owned())),
        cum,
        2,
    );
    total += 6;
    t.AppendTopN(
        codec::EncodeKey(
            codec::time::UTC,
            Vec::new(),
            vec![types::NewStringDatum("aaa".to_owned())],
        )
        .unwrap(),
        5,
    );
    total += 5;
    t.AppendTopN(
        codec::EncodeKey(
            codec::time::UTC,
            Vec::new(),
            vec![types::NewStringDatum("zzz".to_owned())],
        )
        .unwrap(),
        3,
    );
    total += 3;
    hists.push(Some(h));
    tops.push(Some(t));
    let mut h = NewHistogram(1, 0, 0, 0, &ft, 0, 0);
    let mut cum = 0;
    let mut t = NewTopN(0);
    cum += 3;
    h.AppendBucket(
        &bound(types::NewStringDatum("bbb".to_owned())),
        &bound(types::NewStringDatum("ccc".to_owned())),
        cum,
        1,
    );
    total += 3;
    cum += 5;
    h.AppendBucket(
        &bound(types::NewStringDatum("ddd".to_owned())),
        &bound(types::NewStringDatum("eee".to_owned())),
        cum,
        2,
    );
    total += 5;
    t.AppendTopN(
        codec::EncodeKey(
            codec::time::UTC,
            Vec::new(),
            vec![types::NewStringDatum("aaa".to_owned())],
        )
        .unwrap(),
        4,
    );
    total += 4;
    t.AppendTopN(
        codec::EncodeKey(
            codec::time::UTC,
            Vec::new(),
            vec![types::NewStringDatum("yyy".to_owned())],
        )
        .unwrap(),
        2,
    );
    total += 2;
    hists.push(Some(h));
    tops.push(Some(t));
    let (top, g) =
        MergePartTopNAndHistToGlobal(&sc, &killer, &tops, &hists, 2, 3, is_index).unwrap();
    assert_eq!(
        g.NotNullCount() as i64 + top.as_ref().map_or(0, TopN::TotalCount) as i64,
        total
    );
    assert!(g.Len() <= 3);
    assert_merge_invariants(&sc, &g, top.as_ref(), is_index);
}
#[test]
fn combined_case_numtopn_zero_with_high_repeats() {
    let sc = stmtctx::NewStmtCtx();
    let killer = sqlkiller::sqlkiller::SQLKiller::default();
    let is_index = false;
    let bound = |d: types::Datum| {
        if is_index {
            types::NewBytesDatum(codec::EncodeKey(codec::time::UTC, Vec::new(), vec![d]).unwrap())
        } else {
            d
        }
    };
    let ft = types::NewFieldType(if is_index {
        types::mysql::TypeBlob
    } else {
        types::mysql::TypeLong
    });
    let mut hists = Vec::new();
    let mut tops = Vec::new();
    let mut total = 0;
    let mut h = NewHistogram(1, 0, 0, 0, &ft, 0, 0);
    let mut cum = 0;
    let mut t = NewTopN(0);
    cum += 30;
    h.AppendBucket(
        &bound(types::NewIntDatum(1)),
        &bound(types::NewIntDatum(10)),
        cum,
        20,
    );
    total += 30;
    cum += 40;
    h.AppendBucket(
        &bound(types::NewIntDatum(11)),
        &bound(types::NewIntDatum(20)),
        cum,
        25,
    );
    total += 40;
    hists.push(Some(h));
    tops.push(Some(t));
    let mut h = NewHistogram(1, 0, 0, 0, &ft, 0, 0);
    let mut cum = 0;
    let mut t = NewTopN(0);
    cum += 30;
    h.AppendBucket(
        &bound(types::NewIntDatum(1)),
        &bound(types::NewIntDatum(10)),
        cum,
        20,
    );
    total += 30;
    cum += 40;
    h.AppendBucket(
        &bound(types::NewIntDatum(11)),
        &bound(types::NewIntDatum(20)),
        cum,
        25,
    );
    total += 40;
    hists.push(Some(h));
    tops.push(Some(t));
    let (top, g) =
        MergePartTopNAndHistToGlobal(&sc, &killer, &tops, &hists, 0, 3, is_index).unwrap();
    assert_eq!(
        g.NotNullCount() as i64 + top.as_ref().map_or(0, TopN::TotalCount) as i64,
        total
    );
    assert!(g.Len() <= 3);
    assert_merge_invariants(&sc, &g, top.as_ref(), is_index);
}

fn assert_merge_invariants(
    sc: &stmtctx::StatementContext,
    h: &Histogram,
    t: Option<&TopN>,
    index: bool,
) {
    for i in 0..h.Len() {
        assert!(
            h.GetLower(i)
                .Compare(
                    sc.TypeCtx(),
                    h.GetUpper(i),
                    collate::GetBinaryCollator().as_ref()
                )
                .unwrap()
                <= 0
        );
        assert!(h.BucketCount(i) > 0);
        if i > 0 {
            assert!(
                h.GetLower(i)
                    .Compare(
                        sc.TypeCtx(),
                        h.GetUpper(i - 1),
                        collate::GetBinaryCollator().as_ref()
                    )
                    .unwrap()
                    >= 0
            );
        }
        let enc = if index {
            h.GetUpper(i).GetBytes().to_vec()
        } else {
            codec::EncodeKey(codec::time::UTC, Vec::new(), vec![h.GetUpper(i).clone()]).unwrap()
        };
        if t.is_some_and(|t| t.QueryTopN(&enc).1) {
            assert_eq!(h.Buckets[i].Repeat, 0);
        }
    }
    if let Some(t) = t {
        assert!(t.TopN.windows(2).all(|w| w[0].Encoded <= w[1].Encoded));
    }
}
fn uniform(n: i64, mass: i64, width: i64, offset: i64) -> Vec<(i64, i64, i64, i64)> {
    (0..n)
        .map(|i| (offset + i * width + 1, offset + i * width + width, mass, 0))
        .collect()
}
#[test]
fn combined_merge_uniform_and_fat_tail_pack_at_target() {
    let (_, g) = merge(&[Some(hist(&uniform(50, 20, 5, 0)))], &[], 0, 10);
    assert_eq!(g.Len(), 10);
    let masses = (0..g.Len()).map(|i| g.BucketCount(i)).collect::<Vec<_>>();
    assert!(*masses.iter().max().unwrap() < *masses.iter().min().unwrap() * 2);
    let mut input = uniform(10, 100, 10, 0);
    input.push((200, 200, 1000, 1000));
    let (_, g) = merge(&[Some(hist(&input))], &[], 0, 10);
    assert_eq!(g.NotNullCount(), 2000.0);
    let hot = (0..g.Len())
        .find(|&i| g.GetUpper(i).GetInt64() == 200)
        .unwrap();
    assert!(g.BucketCount(hot) >= 1000);
    for i in 0..g.Len() {
        if i != hot {
            assert!(g.BucketCount(i) <= 222);
        }
    }
    let (t, g) = merge(&[Some(hist(&input))], &[], 1, 10);
    assert_eq!(t.unwrap().QueryTopN(&key(200)), (1000, true));
    assert_eq!(g.NotNullCount(), 1000.0);
    let masses = (0..g.Len()).map(|i| g.BucketCount(i)).collect::<Vec<_>>();
    assert!(*masses.iter().max().unwrap() < *masses.iter().min().unwrap() * 2);
    let mut input = vec![(1, 1, 1000, 1000)];
    input.extend(uniform(100, 5, 10, 10));
    let (_, g) = merge(&[Some(hist(&input))], &[], 0, 10);
    assert_eq!(g.GetLower(0).GetInt64(), 1);
    assert!(g.BucketCount(0) >= 1000);
    assert!((1..g.Len()).filter(|&i| g.BucketCount(i) < 20).count() <= 1);
    assert_eq!(g.NotNullCount(), 1500.0);
}
#[test]
fn combined_merge_requested_bucket_caps_preserve_all_mass() {
    for (n, mass, offset, buckets, total) in [(5, 10, 100, 1, 100), (3, 5, 100, 50, 30)] {
        let (_, g) = merge(
            &[
                Some(hist(&uniform(n, mass, 5, 0))),
                Some(hist(&uniform(n, mass, 5, offset))),
            ],
            &[],
            0,
            buckets,
        );
        assert_eq!(g.NotNullCount(), total as f64);
        assert!(g.Len() <= buckets as usize);
        assert_merge_invariants(&stmtctx::NewStmtCtx(), &g, None, false);
    }
}
#[test]
fn combined_merge_capacity_errors_are_explicit() {
    let sc = stmtctx::NewStmtCtx();
    let killer = sqlkiller::sqlkiller::SQLKiller::default();
    let hists = vec![None; u16::MAX as usize + 1];
    assert!(
        MergePartTopNAndHistToGlobal(&sc, &killer, &[], &hists, 0, 1, false)
            .unwrap_err()
            .to_string()
            .contains("too many partition histograms")
    );
    let mut h = hist(&[]);
    h.Buckets = vec![Bucket::default(); u16::MAX as usize + 1];
    assert!(
        MergePartTopNAndHistToGlobal(&sc, &killer, &[], &[Some(h)], 0, 1, false)
            .unwrap_err()
            .to_string()
            .contains("too many buckets")
    );
    let mut hists = vec![None; u16::MAX as usize];
    hists[0] = Some(hist(&[]));
    let t = top(&[(1, 2)]);
    assert!(
        MergePartTopNAndHistToGlobal(&sc, &killer, &[Some(t)], &hists, 0, 1, false)
            .unwrap_err()
            .to_string()
            .contains("too many virtual histograms")
    );
}
#[test]
fn combined_merge_sqlkiller_preserves_signal_errors() {
    let sc = stmtctx::NewStmtCtx();
    for signal in [
        sqlkiller::sqlkiller::QueryInterrupted,
        sqlkiller::sqlkiller::MaxExecTimeExceeded,
        sqlkiller::sqlkiller::QueryMemoryExceeded,
        sqlkiller::sqlkiller::ServerMemoryExceeded,
    ] {
        let killer = sqlkiller::sqlkiller::SQLKiller::default();
        killer.SendKillSignal(signal);
        let expected = killer.HandleSignal().unwrap_err();
        let error = MergePartTopNAndHistToGlobal(
            &sc,
            &killer,
            &[],
            &[Some(hist(&[(1, 2, 5, 1)]))],
            1,
            2,
            false,
        )
        .unwrap_err();
        assert_eq!(error.to_string(), expected.to_string());
    }
}

#[test]
fn combined_merge_production_scale_virtual_hist_chunking() {
    let mut hists = Vec::with_capacity(8192);
    let mut tops = Vec::with_capacity(8192);
    for p in 0..8192i64 {
        let base = p * 12500;
        hists.push(Some(hist(
            &(0..256)
                .map(|b| {
                    let lo = base + b * 39 + 302;
                    (lo, lo + 38, 12, 4)
                })
                .collect::<Vec<_>>(),
        )));
        tops.push(Some(top(&(0..100)
            .map(|i| (base + i * 3 + 1, 4))
            .collect::<Vec<_>>())));
    }
    let (t, g) = merge(&hists, &tops, 100, 256);
    assert_eq!(
        g.NotNullCount() + t.as_ref().map_or(0, TopN::TotalCount) as f64,
        8192.0 * (256.0 * 12.0 + 100.0 * 4.0)
    );
    assert!((200..=256).contains(&g.Len()));
    let mut previous = 0;
    let masses = g
        .Buckets
        .iter()
        .map(|b| {
            let mass = b.Count - previous;
            previous = b.Count;
            mass
        })
        .collect::<Vec<_>>();
    assert!(*masses.iter().min().unwrap() > 0);
    assert!(*masses.iter().max().unwrap() as f64 / (*masses.iter().min().unwrap() as f64) < 10.0);
}

#[test]
fn combined_merge_type_universe_and_encoded_order_contract() {
    let cases = type_matrix();
    let covered = cases
        .iter()
        .map(|(ft, _, _)| ft.GetType())
        .collect::<std::collections::BTreeSet<_>>();
    for tp in 0..=255u8 {
        if types::TypeStr(tp).is_empty()
            || covered.contains(&tp)
            || [
                types::mysql::TypeUnspecified,
                types::mysql::TypeNull,
                types::mysql::TypeGeometry,
                types::mysql::TypeJSON,
                types::mysql::TypeTiDBVectorFloat32,
            ]
            .contains(&tp)
        {
            continue;
        }
        panic!(
            "column type {} ({tp}) needs a merge case",
            types::TypeStr(tp)
        );
    }
    let sc = stmtctx::NewStmtCtx();
    let mut divergent = std::collections::BTreeSet::new();
    for (ft, lo, hi) in cases {
        for (a, b) in [
            (lo.clone(), hi.clone()),
            (types::GetMinValue(&ft), types::GetMaxValue(&ft)),
        ] {
            if a.Kind() != lo.Kind() || b.Kind() != hi.Kind() {
                continue;
            }
            let Ok(a_key) = codec::EncodeKey(codec::time::UTC, Vec::new(), vec![a.clone()]) else {
                continue;
            };
            let Ok(b_key) = codec::EncodeKey(codec::time::UTC, Vec::new(), vec![b.clone()]) else {
                continue;
            };
            let order = a
                .Compare(sc.TypeCtx(), &b, collate::GetBinaryCollator().as_ref())
                .unwrap();
            if (order < 0) != (a_key < b_key) {
                assert!(
                    [
                        types::mysql::TypeEnum,
                        types::mysql::TypeSet,
                        types::mysql::TypeBit
                    ]
                    .contains(&ft.GetType())
                );
                divergent.insert(ft.GetType());
            }
        }
    }
    assert_eq!(
        divergent,
        [
            types::mysql::TypeEnum,
            types::mysql::TypeSet,
            types::mysql::TypeBit
        ]
        .into_iter()
        .collect()
    );
}

#[test]
fn merge_singleton_filter_tracks_changed_active_default() {
    struct Restore(u64);
    impl Drop for Restore {
        fn drop(&mut self) {
            vardef::AnalyzeDefaultNumTopN.Store(self.0);
        }
    }
    let _restore = Restore(vardef::AnalyzeDefaultNumTopN.Load());
    vardef::AnalyzeDefaultNumTopN.Store(150);
    let h = Some(hist(
        &(0..200)
            .map(|i| (i * 10 + 1, i * 10 + 6, 4, 1))
            .collect::<Vec<_>>(),
    ));
    for (n, want) in [
        (150, 0),
        (DefaultTopNValue as u32, DefaultTopNValue as usize),
    ] {
        let (t, g) = merge(std::slice::from_ref(&h), &[], n, 128);
        assert_eq!(t.as_ref().map_or(0, TopN::Num), want);
        assert_eq!(
            g.NotNullCount() + t.as_ref().map_or(0, TopN::TotalCount) as f64,
            800.0
        );
    }
}

#[test]
fn combined_merge_deterministic_fuzz_invariants_across_types_and_index() {
    let sc = stmtctx::NewStmtCtx();
    for (ft, lo, hi) in type_matrix() {
        for is_index in [false, true] {
            let encode =
                |d: types::Datum| codec::EncodeKey(codec::time::UTC, Vec::new(), vec![d]).unwrap();
            let lo_key = encode(lo.clone());
            let hi_key = encode(hi.clone());
            let (field, a, b) = if is_index {
                (
                    types::NewFieldType(types::mysql::TypeBlob),
                    types::NewBytesDatum(lo_key.clone()),
                    types::NewBytesDatum(hi_key.clone()),
                )
            } else {
                (ft.clone(), lo.clone(), hi.clone())
            };
            let (a, b) = if a
                .Compare(sc.TypeCtx(), &b, collate::GetBinaryCollator().as_ref())
                .unwrap()
                > 0
            {
                (b, a)
            } else {
                (a, b)
            };
            for seed in 1..=12i64 {
                let mut hists = Vec::new();
                let mut tops = Vec::new();
                let mut total = 0;
                for part in 0..(seed % 5 + 1) {
                    let mass = (seed * 17 + part * 31) % 97 + 2;
                    let repeat = (seed + part) % (mass + 1);
                    let mut h = NewHistogram(1, 10, 0, 0, &field, 1, 0);
                    h.AppendBucket(&a, &b, mass, repeat);
                    hists.push(Some(h));
                    let count = (seed * 3 + part) % 11 + 1;
                    let mut t = NewTopN(1);
                    t.AppendTopN(lo_key.clone(), count as u64);
                    tops.push(Some(t));
                    total += mass + count;
                }
                let requested = (seed % 4) as u32;
                let cap = seed % 6 + 1;
                let (top, h) = MergePartTopNAndHistToGlobal(
                    &sc,
                    &sqlkiller::sqlkiller::SQLKiller::default(),
                    &tops,
                    &hists,
                    requested,
                    cap,
                    is_index,
                )
                .unwrap();
                assert_eq!(
                    h.NotNullCount() + top.as_ref().map_or(0, TopN::TotalCount) as f64,
                    total as f64,
                    "type {} index {is_index} seed {seed}",
                    ft.GetType()
                );
                assert!(h.Len() <= cap as usize);
                assert!(top.as_ref().map_or(0, TopN::Num) <= requested as usize);
                let mut prior = 0;
                for i in 0..h.Len() {
                    let bucket = &h.Buckets[i];
                    assert!(bucket.Count > prior);
                    assert!(bucket.Repeat >= 0 && bucket.Repeat <= bucket.Count - prior);
                    prior = bucket.Count;
                    assert!(
                        h.GetLower(i)
                            .Compare(
                                sc.TypeCtx(),
                                h.GetUpper(i),
                                collate::GetBinaryCollator().as_ref()
                            )
                            .unwrap()
                            <= 0
                    );
                    if i > 0 {
                        assert!(
                            h.GetLower(i)
                                .Compare(
                                    sc.TypeCtx(),
                                    h.GetUpper(i - 1),
                                    collate::GetBinaryCollator().as_ref()
                                )
                                .unwrap()
                                >= 0
                        );
                    }
                }
            }
        }
    }
}
