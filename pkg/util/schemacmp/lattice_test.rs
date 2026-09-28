// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

// 格代数基础元素的单元测试。
//
// 对应 Go `lattice_test.go` 的 `TestCompatibilities`：覆盖 Bool、Singleton、
// BitSet、Tuple、Maybe、StringList、EqualitySingleton、有序整数、Map 与 FieldTp。

use super::*;
use std::collections::HashMap;

#[derive(Clone, Debug, PartialEq, Eq)]
/// 测试用字节切片相等语义，实现 `Equality`。
struct EqBytes(Vec<u8>);

impl Equality for EqBytes {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    fn Equals(&self, other: &dyn Equality) -> bool {
        other
            .as_any()
            .downcast_ref::<EqBytes>()
            .is_some_and(|value| value.0 == self.0)
    }
}

#[derive(Clone, Copy, Default)]
/// 测试用 usize 全序格（Compare 用 cmp，Join 取 max）。
struct LatticeUsize(usize);

impl Lattice for LatticeUsize {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }
    fn Unwrap(&self) -> AnyValue {
        AnyValue::new(self.0)
    }
    fn Compare(&self, other: LatticeRef<'_>) -> Result<i32, IncompatibleError> {
        let other = other
            .as_any()
            .downcast_ref::<LatticeUsize>()
            .ok_or_else(|| typeMismatchError(self, other))?;
        Ok(self.0.cmp(&other.0) as i32)
    }
    fn Join(&self, other: LatticeRef<'_>) -> Result<LatticeBox, IncompatibleError> {
        let other = other
            .as_any()
            .downcast_ref::<LatticeUsize>()
            .ok_or_else(|| typeMismatchError(self, other))?;
        Ok(Box::new(LatticeUsize(self.0.max(other.0))))
    }
    fn clone_box(&self) -> LatticeBox {
        Box::new(*self)
    }
}

#[derive(Clone, Default)]
/// 测试用字符串→usize 的 `LatticeMap`；缺失键视为更小，不相容 join 时删除。
struct UintMap(HashMap<String, LatticeUsize>);

impl LatticeMap for UintMap {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }
    fn New(&self) -> Box<dyn LatticeMap> {
        Box::new(Self::default())
    }
    fn Insert(&mut self, key: String, value: LatticeBox) {
        let number = value
            .Unwrap()
            .downcast_ref::<usize>()
            .copied()
            .unwrap_or_else(|| {
                value
                    .as_any()
                    .downcast_ref::<LatticeUsize>()
                    .map(|v| v.0)
                    .expect("UintMap values must be usize")
            });
        self.0.insert(key, LatticeUsize(number));
    }
    fn Get(&self, key: &str) -> Option<LatticeRef<'_>> {
        self.0.get(key).map(|value| value as LatticeRef<'_>)
    }
    fn ForEach(
        &self,
        f: &mut dyn FnMut(&str, LatticeRef<'_>) -> Result<(), IncompatibleError>,
    ) -> Result<(), IncompatibleError> {
        for (key, value) in &self.0 {
            f(key, value)?;
        }
        Ok(())
    }
    fn CompareWithNil(&self, _value: LatticeRef<'_>) -> Result<i32, IncompatibleError> {
        Ok(1)
    }
    fn JoinWithNil(&self, value: LatticeRef<'_>) -> Result<Option<LatticeBox>, IncompatibleError> {
        Ok(Some(value.clone_box()))
    }
    fn ShouldDeleteIncompatibleJoin(&self) -> bool {
        true
    }
    fn clone_box(&self) -> Box<dyn LatticeMap> {
        Box::new(self.clone())
    }
}

/// 由键值对构造测试用 map 格。
fn uint_map(entries: &[(&str, usize)]) -> LatticeBox {
    Map(Box::new(UintMap(
        entries
            .iter()
            .map(|(k, v)| ((*k).to_owned(), LatticeUsize(*v)))
            .collect(),
    )))
}

/// 断言双向 `Compare` 结果互为相反数。
fn assert_compare_ok(a: &dyn Lattice, b: &dyn Lattice, expected: i32) {
    assert_eq!(a.Compare(b).unwrap(), expected);
    assert_eq!(b.Compare(a).unwrap(), -expected);
}

/// 断言双向 `Compare` 均失败且错误信息匹配 needle。
fn assert_compare_err(a: &dyn Lattice, b: &dyn Lattice, needle: &str) {
    let err = a.Compare(b).unwrap_err().to_string();
    assert!(
        err.contains(needle) || regex_loose_match(needle, &err),
        "{err}"
    );
    let err = b.Compare(a).unwrap_err().to_string();
    assert!(
        err.contains(needle) || regex_loose_match(needle, &err),
        "{err}"
    );
}

/// 把 Go 测试里的简单正则片段拆成字面量子串匹配。
fn regex_loose_match(pattern: &str, text: &str) -> bool {
    // Go cases use simple regexp fragments; match the distinctive literal parts.
    pattern
        .split(".*")
        .filter(|part| !part.is_empty())
        .all(|part| text.contains(part.trim_matches('\\')))
}

/// 断言双向 `Join` 成功、等于指定最小上界，且结果不小于两侧。
fn assert_join_ok(a: &dyn Lattice, b: &dyn Lattice, expected: &dyn Lattice) {
    let joined = a.Join(b).unwrap();
    assert_eq!(joined.Compare(expected).unwrap(), 0);
    assert_eq!(expected.Compare(joined.as_ref()).unwrap(), 0);
    assert!(joined.Compare(a).unwrap() >= 0);
    assert!(joined.Compare(b).unwrap() >= 0);
    let joined = b.Join(a).unwrap();
    assert_eq!(joined.Compare(expected).unwrap(), 0);
    assert_eq!(expected.Compare(joined.as_ref()).unwrap(), 0);
    assert!(joined.Compare(a).unwrap() >= 0);
    assert!(joined.Compare(b).unwrap() >= 0);
}

/// 断言双向 `Join` 均失败且错误信息匹配 needle。
fn assert_join_err(a: &dyn Lattice, b: &dyn Lattice, needle: &str) {
    let err = match a.Join(b) {
        Ok(_) => panic!("expected join error containing {needle}"),
        Err(error) => error.to_string(),
    };
    assert!(
        err.contains(needle) || regex_loose_match(needle, &err),
        "{err}"
    );
    let err = match b.Join(a) {
        Ok(_) => panic!("expected join error containing {needle}"),
        Err(error) => error.to_string(),
    };
    assert!(
        err.contains(needle) || regex_loose_match(needle, &err),
        "{err}"
    );
}

// TestCompatibilities 对应 Go 的 TestCompatibilities。
#[test]
/// 覆盖各基础格元素的 Compare/Join 兼容性与错误路径。
fn test_compatibilities() {
    assert_compare_ok(&Bool(false), &Bool(false), 0);
    assert_join_ok(&Bool(false), &Bool(false), &Bool(false));
    assert_compare_ok(&Bool(false), &Bool(true), -1);
    assert_join_ok(&Bool(false), &Bool(true), &Bool(true));
    assert_compare_ok(&Bool(true), &Bool(true), 0);
    assert_join_ok(&Bool(true), &Bool(true), &Bool(true));

    assert_compare_ok(Singleton(123_i64).as_ref(), Singleton(123_i64).as_ref(), 0);
    assert_join_ok(
        Singleton(123_i64).as_ref(),
        Singleton(123_i64).as_ref(),
        Singleton(123_i64).as_ref(),
    );
    assert_compare_err(
        Singleton(123_i64).as_ref(),
        Singleton(2468_i64).as_ref(),
        "distinct singletons",
    );
    assert_join_err(
        Singleton(123_i64).as_ref(),
        Singleton(2468_i64).as_ref(),
        "distinct singletons",
    );

    assert_compare_err(
        &BitSet(0b010110),
        &BitSet(0b110001),
        "non-inclusive bit sets",
    );
    assert_join_ok(&BitSet(0b010110), &BitSet(0b110001), &BitSet(0b110111));
    assert_compare_ok(&BitSet(0xffffffff), &BitSet(0), 1);
    assert_join_ok(&BitSet(0xffffffff), &BitSet(0), &BitSet(0xffffffff));
    assert_compare_ok(&BitSet(0b10001), &BitSet(0b11011), -1);
    assert_join_ok(&BitSet(0b10001), &BitSet(0b11011), &BitSet(0b11011));
    assert_compare_ok(&BitSet(0x522), &BitSet(0x522), 0);
    assert_join_ok(&BitSet(0x522), &BitSet(0x522), &BitSet(0x522));

    assert_compare_ok(&Byte(123), &Byte(123), 0);
    assert_join_ok(&Byte(123), &Byte(123), &Byte(123));
    assert_compare_ok(&Byte(1), &Byte(23), -1);
    assert_join_ok(&Byte(1), &Byte(23), &Byte(23));
    assert_compare_ok(&Byte(123), &Byte(45), 1);
    assert_join_ok(&Byte(123), &Byte(45), &Byte(123));

    let left = Tuple(vec![Box::new(Byte(123)), Box::new(Bool(false))]);
    let right = Tuple(vec![Box::new(Byte(67)), Box::new(Bool(true))]);
    assert_compare_err(&left, &right, "combining contradicting orders");
    let expected = Tuple(vec![Box::new(Byte(123)), Box::new(Bool(true))]);
    assert_join_ok(&left, &right, &expected);

    assert_compare_ok(&Tuple(vec![]), &Tuple(vec![]), 0);
    assert_join_ok(&Tuple(vec![]), &Tuple(vec![]), &Tuple(vec![]));
    assert_compare_err(
        &Tuple(vec![Singleton(6_i64), Singleton(7_i64)]),
        &Tuple(vec![Singleton(6_i64), Singleton(8_i64)]),
        "distinct singletons",
    );
    assert_join_err(
        &Tuple(vec![Singleton(6_i64), Singleton(7_i64)]),
        &Tuple(vec![Singleton(6_i64), Singleton(8_i64)]),
        "distinct singletons",
    );
    assert_compare_err(
        &Tuple(vec![]),
        &Tuple(vec![Box::new(Bool(false))]),
        "tuple length mismatch",
    );
    assert_join_err(
        &Tuple(vec![]),
        &Tuple(vec![Box::new(Bool(false))]),
        "tuple length mismatch",
    );
    assert_compare_err(&Bool(false), Singleton(false).as_ref(), "type mismatch");
    assert_join_err(&Bool(false), Singleton(false).as_ref(), "type mismatch");

    assert_compare_err(
        Maybe(Some(Singleton(123_i64))).as_ref(),
        Maybe(Some(Singleton(678_i64))).as_ref(),
        "distinct singletons",
    );
    assert_join_err(
        Maybe(Some(Singleton(123_i64))).as_ref(),
        Maybe(Some(Singleton(678_i64))).as_ref(),
        "distinct singletons",
    );
    assert_compare_ok(
        Maybe(Some(Box::new(Byte(111)))).as_ref(),
        Maybe(Some(Box::new(Byte(222)))).as_ref(),
        -1,
    );
    assert_join_ok(
        Maybe(Some(Box::new(Byte(111)))).as_ref(),
        Maybe(Some(Box::new(Byte(222)))).as_ref(),
        Maybe(Some(Box::new(Byte(222)))).as_ref(),
    );
    assert_compare_ok(
        Maybe(None).as_ref(),
        Maybe(Some(Singleton(135_i64))).as_ref(),
        -1,
    );
    assert_join_ok(
        Maybe(None).as_ref(),
        Maybe(Some(Singleton(135_i64))).as_ref(),
        Maybe(Some(Singleton(135_i64))).as_ref(),
    );
    assert_compare_ok(Maybe(None).as_ref(), Maybe(None).as_ref(), 0);
    assert_join_ok(
        Maybe(None).as_ref(),
        Maybe(None).as_ref(),
        Maybe(None).as_ref(),
    );
    assert_compare_err(
        &Bool(false),
        Maybe(Some(Box::new(Bool(false)))).as_ref(),
        "type mismatch",
    );
    assert_join_err(
        &Bool(false),
        Maybe(Some(Box::new(Bool(false)))).as_ref(),
        "type mismatch",
    );

    assert_compare_ok(
        &StringList(vec!["one".into(), "two".into(), "three".into()]),
        &StringList(vec![
            "one".into(),
            "two".into(),
            "three".into(),
            "four".into(),
            "five".into(),
        ]),
        -1,
    );
    assert_join_ok(
        &StringList(vec!["one".into(), "two".into(), "three".into()]),
        &StringList(vec![
            "one".into(),
            "two".into(),
            "three".into(),
            "four".into(),
            "five".into(),
        ]),
        &StringList(vec![
            "one".into(),
            "two".into(),
            "three".into(),
            "four".into(),
            "five".into(),
        ]),
    );
    assert_compare_err(
        &StringList(vec!["one".into(), "two".into(), "three".into()]),
        &StringList(vec!["two".into(), "three".into()]),
        "distinct values",
    );
    assert_join_err(
        &StringList(vec!["one".into(), "two".into(), "three".into()]),
        &StringList(vec!["two".into(), "three".into()]),
        "distinct values",
    );
    assert_compare_err(
        &StringList(vec!["a".into(), "b".into(), "c".into()]),
        &StringList(vec![
            "a".into(),
            "e".into(),
            "i".into(),
            "o".into(),
            "u".into(),
        ]),
        "distinct values",
    );
    assert_join_err(
        &StringList(vec!["a".into(), "b".into(), "c".into()]),
        &StringList(vec![
            "a".into(),
            "e".into(),
            "i".into(),
            "o".into(),
            "u".into(),
        ]),
        "distinct values",
    );
    assert_compare_ok(&StringList(vec![]), &StringList(vec![]), 0);
    assert_join_ok(
        &StringList(vec![]),
        &StringList(vec![]),
        &StringList(vec![]),
    );

    let eq_a = EqualitySingleton(EqBytes(b"abcdef".to_vec()));
    let eq_b = EqualitySingleton(EqBytes(b"ABCDEF".to_vec()));
    assert_compare_ok(eq_a.as_ref(), eq_a.as_ref(), 0);
    assert_join_ok(eq_a.as_ref(), eq_a.as_ref(), eq_a.as_ref());
    assert_compare_err(eq_a.as_ref(), eq_b.as_ref(), "distinct singletons");
    assert_join_err(eq_a.as_ref(), eq_b.as_ref(), "distinct singletons");
    assert_compare_err(
        eq_a.as_ref(),
        Singleton(EqBytes(b"ABCDEF".to_vec())).as_ref(),
        "type mismatch",
    );
    assert_join_err(
        eq_a.as_ref(),
        Singleton(EqBytes(b"ABCDEF".to_vec())).as_ref(),
        "type mismatch",
    );

    assert_compare_ok(&Int64(234), &Int64(-5), 1);
    assert_join_ok(&Int64(234), &Int64(-5), &Int64(234));
    assert_compare_ok(&Uint(665544), &Uint(765), 1);
    assert_join_ok(&Uint(665544), &Uint(765), &Uint(665544));

    let map_a = uint_map(&[("a", 123), ("b", 678), ("c", 456)]);
    let map_b = uint_map(&[("a", 234), ("b", 567), ("d", 789)]);
    assert_compare_err(
        map_a.as_ref(),
        map_b.as_ref(),
        "combining contradicting orders",
    );
    let expected_map = uint_map(&[("a", 234), ("b", 678), ("c", 456), ("d", 789)]);
    assert_join_ok(map_a.as_ref(), map_b.as_ref(), expected_map.as_ref());
    let map_c = uint_map(&[("a", 1), ("c", 4)]);
    assert_compare_ok(map_a.as_ref(), map_c.as_ref(), 1);
    assert_join_ok(map_a.as_ref(), map_c.as_ref(), map_a.as_ref());

    let integer_order = [
        mysql::TypeTiny,
        mysql::TypeShort,
        mysql::TypeInt24,
        mysql::TypeLong,
        mysql::TypeLonglong,
    ];
    let blob_order = [
        mysql::TypeTinyBlob,
        mysql::TypeBlob,
        mysql::TypeMediumBlob,
        mysql::TypeLongBlob,
    ];
    for order in [&integer_order[..], &blob_order[..]] {
        for (i, &a) in order.iter().enumerate() {
            for (j, &b) in order.iter().enumerate() {
                assert_compare_ok(
                    FieldTp(a).as_ref(),
                    FieldTp(b).as_ref(),
                    match i.cmp(&j) {
                        std::cmp::Ordering::Less => -1,
                        std::cmp::Ordering::Equal => 0,
                        std::cmp::Ordering::Greater => 1,
                    },
                );
                let expected = if i >= j { a } else { b };
                assert_join_ok(
                    FieldTp(a).as_ref(),
                    FieldTp(b).as_ref(),
                    FieldTp(expected).as_ref(),
                );
            }
        }
    }
    assert_compare_err(
        FieldTp(mysql::TypeLong).as_ref(),
        Singleton(false).as_ref(),
        "type mismatch",
    );
    assert_join_err(
        FieldTp(mysql::TypeLong).as_ref(),
        Singleton(false).as_ref(),
        "type mismatch",
    );
    assert_compare_err(
        FieldTp(mysql::TypeLong).as_ref(),
        FieldTp(mysql::TypeSet).as_ref(),
        "incompatible mysql type",
    );
    assert_join_err(
        FieldTp(mysql::TypeLong).as_ref(),
        FieldTp(mysql::TypeSet).as_ref(),
        "incompatible mysql type",
    );
}
