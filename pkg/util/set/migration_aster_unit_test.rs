// Copyright 2026 AsterSQL.

// set 包迁移回归测试：原语集合、字符串集合、泛型代数与内存追踪变体。
//
// 对照 Go 行为验证成员语义（含 float ±0/NaN）、集合运算稳定顺序，
// 以及带 Tracker 时增长字节计入与返回 delta 抑制。

use super::float64_set::NewFloat64Set;
use super::int_set::{NewInt64Set, NewIntSet};
use super::memory;
use super::set::{AndSet, CombSet, DiffSet, Key, ListToSet, NewSet, UnionSet};
use super::set_with_memory_usage::{
    NewFloat64SetWithMemoryUsage, NewInt64SetWithMemoryUsage, NewStringSetWithMemoryUsage,
    NewStringToDecimalMapWithMemoryUsage, NewStringToStringMapWithMemoryUsage,
};
use super::string_set::NewStringSet;

/// 测试用泛型集合成员：`key` 作为 `Key()`，`payload` 用于同 key 覆盖断言。
#[derive(Clone, Debug, Eq, PartialEq)]
struct Item {
    key: &'static str,
    payload: i32,
}

impl Key for Item {
    fn Key(&self) -> String {
        self.key.to_owned()
    }
}

/// 从元素列表提取稳定顺序的 key，便于与 Go ToList 结果对照。
fn keys(items: Vec<Item>) -> Vec<&'static str> {
    items.into_iter().map(|item| item.key).collect()
}

/// 覆盖 Int/Int64/Float64 集合的去重、边界值与 NaN 插入语义。
#[test]
fn primitive_sets_match_go_membership_and_float_key_semantics() {
    let mut ints = NewIntSet(&[1, 2, 2, -3]);
    ints.Insert(2);
    assert_eq!(ints.Count(), 3);
    assert!(ints.Exist(-3));
    assert!(!ints.Exist(4));

    let mut int64s = NewInt64Set(&[i64::MIN, 7, 7, i64::MAX]);
    int64s.Insert(7);
    assert_eq!(int64s.Count(), 3);
    assert!(int64s.Exist(i64::MAX));

    // -0.0 与 0.0 应视为同一成员；每次 Insert(NaN) 增加计数，Exist(NaN) 恒 false。
    let mut floats = NewFloat64Set(&[1.25, 1.25, -0.0]);
    assert_eq!(floats.Count(), 2);
    assert!(floats.Exist(0.0));
    floats.Insert(f64::INFINITY);
    assert!(floats.Exist(f64::INFINITY));
    floats.Insert(f64::NAN);
    floats.Insert(f64::NAN);
    assert_eq!(floats.Count(), 5);
    assert!(!floats.Exist(f64::NAN));
}

/// 覆盖 StringSet 交集、大小写折叠交集、迭代与 Clear。
#[test]
fn string_set_operations_case_conversion_clear_and_iteration_match_go() {
    let mut values = NewStringSet(&["One", "Two", "Two", "THREE"]);
    assert_eq!(values.Count(), 3);
    assert!(!values.Empty());

    let rhs = NewStringSet(&["Two", "missing", "THREE"]);
    assert_eq!(values.Intersection(&rhs), NewStringSet(&["Two", "THREE"]));

    // IntersectionWithLower：按 lower/upper 规则与另一侧做大小写不敏感交集。
    let lower = NewStringSet(&["one", "two"]);
    let mixed = NewStringSet(&["ONE", "Two", "THREE"]);
    assert_eq!(
        lower.IntersectionWithLower(&mixed, true),
        NewStringSet(&["ONE", "Two"])
    );
    let upper = NewStringSet(&["ONE", "TWO"]);
    assert_eq!(
        upper.IntersectionWithLower(&mixed, false),
        NewStringSet(&["ONE", "Two"])
    );

    let mut iterated = Vec::new();
    values.IterateWith(|value| iterated.push(value));
    iterated.sort();
    assert_eq!(iterated, vec!["One", "THREE", "Two"]);
    values.Clear();
    assert!(values.Empty());
    assert_eq!(values.Count(), 0);
}

/// 覆盖泛型 Set 的增删、按 Key 排序列表、同 key 覆盖与 Clone 独立性。
#[test]
fn generic_set_crud_sorting_clone_and_key_replacement_match_go() {
    let mut set = NewSet::<Item>();
    assert_eq!(set.Size(), 0);
    assert_eq!(set.String(), "{}");
    set.Add(&[
        Item {
            key: "q3",
            payload: 3,
        },
        Item {
            key: "q1",
            payload: 1,
        },
        Item {
            key: "q2",
            payload: 2,
        },
        Item {
            key: "q2",
            payload: 20,
        },
    ]);
    assert_eq!(set.Size(), 3);
    assert_eq!(keys(set.ToList()), vec!["q1", "q2", "q3"]);
    assert_eq!(set.ToList()[1].payload, 20);
    assert_eq!(set.String(), "{q1, q2, q3}");

    // Clone 后修改原集合不应影响副本。
    let cloned = set.Clone();
    set.Remove(&Item {
        key: "q1",
        payload: 0,
    });
    assert!(!set.Contains(&Item {
        key: "q1",
        payload: 999
    }));
    assert!(cloned.Contains(&Item {
        key: "q1",
        payload: 999
    }));
    assert_eq!(cloned.Size(), 3);
}

/// 覆盖并集、交集、差集与 CombSet 组合枚举的稳定顺序。
#[test]
fn generic_set_algebra_and_combinations_match_go_stable_order() {
    let left = ListToSet(&[
        Item {
            key: "q1",
            payload: 1,
        },
        Item {
            key: "q2",
            payload: 2,
        },
        Item {
            key: "q3",
            payload: 3,
        },
    ]);
    let right = ListToSet(&[
        Item {
            key: "q2",
            payload: 20,
        },
        Item {
            key: "q3",
            payload: 30,
        },
        Item {
            key: "q4",
            payload: 40,
        },
    ]);

    let union = UnionSet(&[left.as_ref(), right.as_ref()]);
    assert_eq!(keys(union.ToList()), vec!["q1", "q2", "q3", "q4"]);
    let intersection = AndSet(&[left.as_ref(), right.as_ref()]);
    assert_eq!(keys(intersection.ToList()), vec!["q2", "q3"]);
    let difference = DiffSet(left.as_ref(), right.as_ref());
    assert_eq!(keys(difference.ToList()), vec!["q1"]);

    let combinations = CombSet(left.as_ref(), 2);
    let rendered: Vec<String> = combinations.iter().map(|set| set.String()).collect();
    assert_eq!(rendered, vec!["{q1, q2}", "{q1, q3}", "{q2, q3}"]);
    assert_eq!(CombSet(left.as_ref(), 0)[0].String(), "{}");
    assert!(CombSet(left.as_ref(), -1).is_empty());
    assert!(CombSet(left.as_ref(), 4).is_empty());
}

/// 覆盖带内存用量的集合/映射：初始字节、成员计数与 float NaN 语义。
#[test]
fn memory_aware_sets_preserve_membership_counts_and_initial_bytes() {
    let (strings, string_bytes) =
        NewStringSetWithMemoryUsage(&["alpha".to_owned(), "beta".to_owned(), "alpha".to_owned()]);
    assert!(string_bytes > 0);
    assert_eq!(strings.Bytes(), string_bytes as u64);
    assert_eq!(strings.Count(), 2);
    assert!(strings.Exist("alpha"));

    let (ints, int_bytes) = NewInt64SetWithMemoryUsage(&[1, 2, 2, 3]);
    assert!(int_bytes > 0);
    assert_eq!(ints.Count(), 3);
    assert!(ints.Exist(2));

    let (mut floats, float_bytes) = NewFloat64SetWithMemoryUsage(&[-0.0]);
    assert!(float_bytes > 0);
    assert!(floats.Exist(0.0));
    floats.Insert(f64::INFINITY);
    assert!(floats.Exist(f64::INFINITY));
    floats.Insert(f64::NAN);
    floats.Insert(f64::NAN);
    assert_eq!(floats.Count(), 4);
    assert!(!floats.Exist(f64::NAN));

    let (mut decimals, decimal_bytes) = NewStringToDecimalMapWithMemoryUsage();
    assert!(decimal_bytes > 0);
    decimals.Insert("nil".to_owned(), std::ptr::null_mut());
    assert!(decimals.Exist("nil"));
    assert_eq!(decimals.Count(), 1);
}

/// 配置外部 Tracker 后，Insert 返回 0（delta 由 Tracker 吸收），且 BytesConsumed 增长。
#[test]
fn configured_tracker_consumes_growth_and_suppresses_returned_delta() {
    let tracker = std::sync::Arc::from(memory::NewTracker(403, -1));
    let (mut values, initial_bytes) = NewStringToStringMapWithMemoryUsage();
    assert!(initial_bytes > 0);
    values.SetTracker(Some(std::sync::Arc::clone(&tracker)));

    for index in 0..128 {
        assert_eq!(
            values.Insert(format!("key-{index}"), format!("value-{index}")),
            0
        );
    }
    assert_eq!(values.Count(), 128);
    assert!(tracker.BytesConsumed() > 0);
}
