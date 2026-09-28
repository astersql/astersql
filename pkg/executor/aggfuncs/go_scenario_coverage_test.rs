// 聚合函数 Go 版本关键场景的 Rust 回归覆盖。
//
// 重点验证分区部分结果合并、并行 DISTINCT 去重、NULL 处理，以及 JSON 聚合和
// LEAD/LAG 的状态语义，确保 Rust 实现与对应 Go 测试关注的行为保持一致。

use super::aggfuncs::SpillValue;
use super::func_avg::{DecimalAvg, DistinctFloatAvg, FloatAvg};
use super::func_bitfuncs::{BitAggKind, BitAggregator};
use super::func_count::CountAggregator;
use super::func_count_distinct::{CountDistinct, CountDistinctMulti, DistinctValue};
use super::func_group_concat::GroupConcat;
use super::func_json_arrayagg::JsonArrayAgg;
use super::func_json_objectagg::JsonObjectAgg;
use super::func_lead_lag::{Lag, Lead, LeadLagRow};
use super::func_stddevpop::stddev_population_distinct;
use super::func_stddevsamp::stddev_sample_distinct;
use super::func_sum::{Decimal, DistinctDecimalSum, DistinctFloatSum, FloatSum};
use super::func_sum_int::{SumDistinctInt64, SumInt};
use super::func_varpop::{DistinctVariance, VarianceState};

// 分布式聚合会分别累积源、目标分区，再合并部分结果；合并后必须等价于一次性聚合。
#[test]
fn test_merge_partial_result4_avg() {
    let mut destination = FloatAvg::default();
    destination.update([Some(2.0)]);
    let mut source = FloatAvg::default();
    source.update([Some(4.0), Some(6.0)]);
    destination.merge(&source);
    assert_eq!(destination.result(), Some(4.0));
}

#[test]
fn test_avg() {
    let mut avg = DecimalAvg::default();
    avg.update([Some(Decimal::new(125, 2)), None, Some(Decimal::new(75, 2))])
        .unwrap();
    assert_eq!(avg.result(2).unwrap(), Some(Decimal::new(100, 2)));
}

#[test]
fn test_mem_avg() {
    let mut avg = FloatAvg::default();
    avg.update([Some(1.0), Some(3.0)]);
    assert_eq!(avg.partial_result(), (2, 4.0));
}

#[test]
fn test_merge_partial_result4_bit_funcs() {
    let mut destination = BitAggregator::new(BitAggKind::Or);
    destination.update([Some(0b0011)]);
    let mut source = BitAggregator::new(BitAggKind::Or);
    source.update([Some(0b1100)]);
    destination.merge(&source);
    assert_eq!(destination.value(), 0b1111);
}

#[test]
fn test_mem_bit_func() {
    let mut xor = BitAggregator::new(BitAggKind::Xor);
    xor.update([Some(1), None, Some(1), Some(3)]);
    assert_eq!(xor.value(), 3);
}

#[test]
fn test_merge_partial_result4_count() {
    let mut destination = CountAggregator::default();
    destination.update([Some(1), None]).unwrap();
    let mut source = CountAggregator::default();
    source.update([Some(2), Some(3)]).unwrap();
    destination.merge(&source).unwrap();
    assert_eq!(destination.value(), 3);
}

#[test]
fn test_mem_count() {
    let mut count = CountAggregator::default();
    count.update([Some("a"), None, Some("b")]).unwrap();
    assert_eq!(count.value(), 2);
}

// Rust 自研的补充压力场景。Go 的 TestWriteTime 验证时间编码，不能用 COUNT 循环冒充；
// 对应的时间键编码契约由 func_count_distinct_test 覆盖。
#[test]
fn test_count_high_volume_updates() {
    let mut count = CountAggregator::default();
    for value in 0..100_i64 {
        count.update([Some(value)]).unwrap();
    }
    assert_eq!(count.value(), 100);
}

// 并行 DISTINCT 聚合在合并分区状态时仍需跨分区去重，不能重复计入重叠值。
#[test]
fn test_parallel_distinct_count() {
    let mut destination = CountDistinct::<i64>::default();
    destination.update([Some(1), Some(2), Some(2)]);
    let mut source = CountDistinct::<i64>::default();
    source.update([Some(2), Some(3)]);
    destination.merge(&source);
    assert_eq!(destination.count(), 3);
}

#[test]
fn test_parallel_distinct_sum() {
    let mut destination = DistinctFloatSum::default();
    destination.update([Some(1.0), Some(2.0)]);
    let mut source = DistinctFloatSum::default();
    source.update([Some(2.0), Some(4.0)]);
    destination.merge(&source);
    assert_eq!(destination.value(), Some(7.0));
}

#[test]
fn test_parallel_distinct_sum_int() {
    let mut sum = SumDistinctInt64::default();
    sum.update([Some(2), Some(2), Some(5)]);
    assert_eq!(sum.value().unwrap(), Some(7));
}

#[test]
fn test_parallel_distinct_avg() {
    let mut avg = DistinctFloatAvg::default();
    avg.update([Some(2.0), Some(2.0), Some(4.0)]);
    assert_eq!(avg.result(), Some(3.0));
}

#[test]
fn test_parallel_distinct_var_and_stddev() {
    let mut variance = DistinctVariance::default();
    variance.update([Some(1.0), Some(1.0), Some(3.0)]);
    assert_eq!(variance.population_variance(), Some(1.0));
    assert_eq!(stddev_population_distinct(&variance), Some(1.0));
    assert_eq!(stddev_sample_distinct(&variance), Some(2.0_f64.sqrt()));
}

#[test]
fn test_parallel_distinct_group_concat() {
    let mut destination = GroupConcat::new(b" ".to_vec(), 64, true);
    destination.update([Some(b"a".to_vec()), Some(b"a".to_vec())]);
    let mut source = GroupConcat::new(b" ".to_vec(), 64, true);
    source.update([Some(b"b".to_vec())]);
    destination.merge(&source);
    let result = destination.result().unwrap();
    assert_eq!(result.len(), 3);
    // 分区合并不承诺字符串的拼接顺序，因此这里只校验两种合法排列。
    assert!(result == b"a b" || result == b"b a");
}

// JSON 聚合除合并部分状态外，还覆盖重置、空键拒绝及重复键合并等状态边界。
#[test]
fn test_merge_partial_result4_json_arrayagg() {
    let mut destination = JsonArrayAgg::default();
    destination.update([SpillValue::Int64(1)]);
    let mut source = JsonArrayAgg::default();
    source.update([SpillValue::String("two".to_owned())]);
    destination.merge(&source);
    assert_eq!(destination.result().unwrap().len(), 2);
}

#[test]
fn test_json_arrayagg() {
    let mut array = JsonArrayAgg::default();
    array.update([SpillValue::Bool(true), SpillValue::Int64(2)]);
    assert!(array.result().is_some());
    array.reset();
    assert!(array.result().is_none());
}

#[test]
fn test_mem_json_arrayagg() {
    let mut array = JsonArrayAgg::default();
    array.update([SpillValue::String("payload".to_owned())]);
    assert_eq!(array.result().unwrap().len(), 1);
}

#[test]
fn test_merge_partial_result4_json_objectagg() {
    let mut destination = JsonObjectAgg::default();
    destination
        .update([(Some("key".to_owned()), SpillValue::Int64(1))])
        .unwrap();
    let mut source = JsonObjectAgg::default();
    source
        .update([(Some("key".to_owned()), SpillValue::Int64(2))])
        .unwrap();
    destination.merge(&source);
    assert_eq!(destination.result().unwrap().len(), 1);
}

#[test]
fn test_json_objectagg() {
    let mut object = JsonObjectAgg::default();
    assert!(object.update([(None, SpillValue::Int64(1))]).is_err());
    object
        .update([(Some("key".to_owned()), SpillValue::Bool(true))])
        .unwrap();
    assert!(object.result().is_some());
}

#[test]
fn test_mem_json_objectagg() {
    let mut object = JsonObjectAgg::default();
    object
        .update([
            (Some("a".to_owned()), SpillValue::String("value".to_owned())),
            (Some("b".to_owned()), SpillValue::Int64(1)),
        ])
        .unwrap();
    assert_eq!(object.result().unwrap().len(), 2);
}

// LEAD/LAG 越过分区边界时返回行级默认值，同时保持偏移量对应的取值顺序。
#[test]
fn test_lead_lag() {
    let rows = [
        LeadLagRow::new(Some(1), Some(99)),
        LeadLagRow::new(Some(2), Some(98)),
        LeadLagRow::new(Some(3), Some(97)),
    ];
    let mut lead = Lead::new(1);
    lead.update(rows.clone());
    assert_eq!(lead.next_value(), Some(Some(2)));
    assert_eq!(lead.next_value(), Some(Some(3)));
    assert_eq!(lead.next_value(), Some(Some(97)));

    let mut lag = Lag::new(1);
    lag.update(rows);
    assert_eq!(lag.next_value(), Some(Some(99)));
    assert_eq!(lag.next_value(), Some(Some(1)));
    assert_eq!(lag.next_value(), Some(Some(2)));
}

#[test]
fn test_mem_lead_lag() {
    let mut lead = Lead::new(2);
    assert!(lead.update([LeadLagRow::new(Some("v"), None)]) > 0);
}

#[test]
fn test_sum_and_variance_partial_merge() {
    let mut sum = SumInt::default();
    sum.update([Some(4), Some(6)]).unwrap();
    let mut other = SumInt::default();
    other.update([Some(10)]).unwrap();
    sum.merge(&other).unwrap();
    assert_eq!(sum.value(), Some(20));

    let mut variance = VarianceState::default();
    variance.update([Some(1.0), Some(2.0), Some(3.0)]);
    assert_eq!(variance.population_variance(), Some(2.0 / 3.0));
}

#[test]
fn test_decimal_distinct_sum_and_multi_distinct_nulls() {
    let mut sum = DistinctDecimalSum::default();
    sum.update([
        Some(Decimal::new(100, 2)),
        Some(Decimal::new(100, 2)),
        Some(Decimal::new(25, 2)),
    ]);
    assert_eq!(sum.value().unwrap(), Some(Decimal::new(125, 2)));

    let mut multi = CountDistinctMulti::default();
    multi
        .update([
            vec![Some(DistinctValue::Int(1)), Some(DistinctValue::Int(2))],
            vec![Some(DistinctValue::Int(1)), None],
        ])
        .unwrap();
    // 多列 COUNT DISTINCT 的任一列为 NULL 时，整组键都不参与计数。
    assert_eq!(multi.count(), 1);
}
