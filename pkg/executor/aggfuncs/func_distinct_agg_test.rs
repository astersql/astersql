// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
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

// Go randomly selects empty, partially NULL, entirely NULL, and non-NULL
// inputs, then compares serial and parallel DISTINCT aggregation. These
// deterministic cases cover every branch on every run with the same matrix.

use crate::func_avg::{DistinctDecimalAvg, DistinctFloatAvg};
use crate::func_count_distinct::{
    CountDistinct, CountDistinctMulti, DistinctValue, update_distinct_real, update_distinct_string,
};
use crate::func_group_concat::GroupConcat;
use crate::func_stddevpop::stddev_population_distinct;
use crate::func_stddevsamp::stddev_sample_distinct;
use crate::func_sum::{Decimal, DistinctDecimalSum, DistinctFloatSum};
use crate::func_sum_int::{SumDistinctInt64, SumDistinctUint64};
use crate::func_varpop::DistinctVariance;

fn assert_count_partition_merge<T>(left: &[Option<T>], right: &[Option<T>], expected: i64)
where
    T: Clone + Eq + std::hash::Hash,
{
    let mut parallel = CountDistinct::<T>::default();
    parallel.update(left.iter().cloned());
    let mut source = CountDistinct::<T>::default();
    source.update(right.iter().cloned());
    parallel.merge(&source);
    let mut serial = CountDistinct::<T>::default();
    serial.update(left.iter().chain(right).cloned());
    assert_eq!(parallel.count(), expected);
    assert_eq!(parallel.count(), serial.count());
}

#[test]
fn test_parallel_distinct_count() {
    assert_count_partition_merge::<i64>(&[], &[], 0);
    assert_count_partition_merge(&[None, Some(1), Some(1)], &[Some(1), Some(2)], 2);
    assert_count_partition_merge::<i64>(&[None, None], &[None], 0);
    // Duration DISTINCT uses its integer duration representation.
    assert_count_partition_merge(&[Some(1_000_i64), None], &[Some(1_000), Some(2_000)], 2);

    let mut real = CountDistinct::<u64>::default();
    update_distinct_real(&mut real, [Some(1.5), None, Some(1.5)]);
    let mut real_source = CountDistinct::<u64>::default();
    update_distinct_real(&mut real_source, [Some(1.5), Some(2.5)]);
    real.merge(&real_source);
    assert_eq!(real.count(), 2);

    let mut decimal = CountDistinct::<Decimal>::default();
    decimal.update([Some(Decimal::new(100, 2)), None, Some(Decimal::new(100, 2))]);
    let mut decimal_source = CountDistinct::<Decimal>::default();
    decimal_source.update([Some(Decimal::new(250, 2))]);
    decimal.merge(&decimal_source);
    assert_eq!(decimal.count(), 2);

    let mut strings = CountDistinct::<Vec<u8>>::default();
    update_distinct_string(
        &mut strings,
        [Some("A".to_owned()), None, Some("a".to_owned())],
        |value| value.to_ascii_lowercase().into_bytes(),
    );
    assert_eq!(strings.count(), 1);

    let mut multi = CountDistinctMulti::default();
    multi
        .update([
            vec![
                Some(DistinctValue::String(b"a".to_vec())),
                Some(DistinctValue::String(b"x".to_vec())),
            ],
            vec![Some(DistinctValue::String(b"a".to_vec())), None],
        ])
        .unwrap();
    let mut multi_source = CountDistinctMulti::default();
    multi_source
        .update([
            vec![
                Some(DistinctValue::String(b"a".to_vec())),
                Some(DistinctValue::String(b"x".to_vec())),
            ],
            vec![
                Some(DistinctValue::String(b"b".to_vec())),
                Some(DistinctValue::String(b"y".to_vec())),
            ],
        ])
        .unwrap();
    multi.merge(&multi_source);
    assert_eq!(multi.count(), 2);
}

#[test]
fn test_parallel_distinct_sum() {
    let mut float = DistinctFloatSum::default();
    float.update([Some(1.0), None, Some(1.0)]);
    let mut float_source = DistinctFloatSum::default();
    float_source.update([Some(1.0), Some(3.0)]);
    float.merge(&float_source);
    assert_eq!(float.value(), Some(4.0));

    let mut decimal = DistinctDecimalSum::default();
    decimal.update([Some(Decimal::new(125, 2)), None, Some(Decimal::new(125, 2))]);
    let mut decimal_source = DistinctDecimalSum::default();
    decimal_source.update([Some(Decimal::new(75, 2))]);
    decimal.merge(&decimal_source);
    assert_eq!(decimal.value().unwrap(), Some(Decimal::new(200, 2)));
    assert_eq!(DistinctFloatSum::default().value(), None);
    assert_eq!(DistinctDecimalSum::default().value().unwrap(), None);
}

#[test]
fn test_parallel_distinct_sum_int() {
    let mut signed = SumDistinctInt64::default();
    signed.update([Some(-2), None, Some(-2)]);
    let mut signed_source = SumDistinctInt64::default();
    signed_source.update([Some(-2), Some(5)]);
    signed.merge(&signed_source);
    assert_eq!(signed.value().unwrap(), Some(3));

    let mut unsigned = SumDistinctUint64::default();
    unsigned.update([Some(2), None, Some(2)]);
    let mut unsigned_source = SumDistinctUint64::default();
    unsigned_source.update([Some(2), Some(5)]);
    unsigned.merge(&unsigned_source);
    assert_eq!(unsigned.value().unwrap(), Some(7));
}

#[test]
fn test_parallel_distinct_avg() {
    let mut float = DistinctFloatAvg::default();
    float.update([Some(2.0), None, Some(2.0)]);
    let mut float_source = DistinctFloatAvg::default();
    float_source.update([Some(2.0), Some(4.0)]);
    float.merge(&float_source);
    assert_eq!(float.result(), Some(3.0));

    let mut decimal = DistinctDecimalAvg::default();
    decimal.update([Some(Decimal::new(100, 2)), None, Some(Decimal::new(100, 2))]);
    let mut decimal_source = DistinctDecimalAvg::default();
    decimal_source.update([Some(Decimal::new(300, 2))]);
    decimal.merge(&decimal_source);
    assert_eq!(decimal.result(2).unwrap(), Some(Decimal::new(200, 2)));
    assert_eq!(DistinctFloatAvg::default().result(), None);
    assert_eq!(DistinctDecimalAvg::default().result(2).unwrap(), None);
}

#[test]
fn test_parallel_distinct_var_and_stddev() {
    let mut values = DistinctVariance::default();
    values.update([Some(1.0), None, Some(1.0)]);
    let mut source = DistinctVariance::default();
    source.update([Some(1.0), Some(3.0)]);
    values.merge(&source);
    assert_eq!(values.population_variance(), Some(1.0));
    assert_eq!(values.sample_variance(), Some(2.0));
    assert_eq!(stddev_population_distinct(&values), Some(1.0));
    assert_eq!(stddev_sample_distinct(&values), Some(2.0_f64.sqrt()));

    let empty = DistinctVariance::default();
    assert_eq!(empty.population_variance(), None);
    assert_eq!(stddev_population_distinct(&empty), None);
    assert_eq!(empty.sample_variance(), None);
    assert_eq!(stddev_sample_distinct(&empty), None);
}

#[test]
fn test_parallel_distinct_group_concat() {
    // Multi-argument rows are represented by evaluated concatenated row bytes.
    let mut parallel = GroupConcat::new(b",".to_vec(), 1024, true);
    parallel.update([Some(b"ax".to_vec()), None, Some(b"ax".to_vec())]);
    let mut source = GroupConcat::new(b",".to_vec(), 1024, true);
    source.update([Some(b"ax".to_vec()), Some(b"by".to_vec())]);
    parallel.merge(&source);
    let result = parallel.result().unwrap();
    assert!(result == b"ax,by" || result == b"by,ax");

    let mut all_null = GroupConcat::new(b",".to_vec(), 1024, true);
    all_null.update([None, None]);
    assert_eq!(all_null.result(), None);
}
