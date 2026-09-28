// Copyright 2026 AsterSQL.
// Copyright 2018 PingCAP, Inc.
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

// COUNT / COUNT(DISTINCT) / APPROX_COUNT_DISTINCT 单元测试。
//
// 测试覆盖 COUNT 的 NULL/partial/slide，以及多列 DISTINCT 跳过含 NULL 行。


use crate::func_count::CountAggregator;
use crate::func_count_distinct::{CountDistinctMulti, DistinctValue};

/// 校验 COUNT 忽略 NULL、合并 partial、滑动窗口，以及多列 DISTINCT 去重语义。
#[test]
fn count_handles_null_partial_merge_slide_and_multi_distinct() {
    // 两个非空 + partial 3 => 5；slide 移出非空 1、移入 9 后仍为 5（None 移出不减）。
    let mut count = CountAggregator::default();
    count.update([Some(1), None, Some(2)]).unwrap();
    count.update_partial([Some(3), None]).unwrap();
    assert_eq!(count.value(), 5);
    count.slide([Some(1), None], [Some(9)]).unwrap();
    assert_eq!(count.value(), 5);

    // 两行相同 (1,"a") 去重为 1；含 None 的行整行丢弃。
    let mut distinct = CountDistinctMulti::default();
    distinct
        .update([
            vec![
                Some(DistinctValue::Int(1)),
                Some(DistinctValue::String(b"a".to_vec())),
            ],
            vec![
                Some(DistinctValue::Int(1)),
                Some(DistinctValue::String(b"a".to_vec())),
            ],
            vec![Some(DistinctValue::Int(2)), None],
        ])
        .unwrap();
    assert_eq!(distinct.count(), 1);
}

/// Go 的 int64 聚合计数使用补码回绕；Rust 不应在边界处引入额外错误契约。
#[test]
fn count_wraps_like_go_int64_arithmetic() {
    let mut count = CountAggregator::default();
    count.update_partial([Some(i64::MAX)]).unwrap();
    count.update([Some(())]).unwrap();
    assert_eq!(count.value(), i64::MIN);

    count.reset();
    count
        .slide([Some(())], std::iter::empty::<Option<()>>())
        .unwrap();
    assert_eq!(count.value(), -1);
}
