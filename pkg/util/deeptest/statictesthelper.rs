// Copyright 2024 PingCAP, Inc.
// Copyright 2026 AsterSQL.
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

// 与 Go 版基于 `reflect` 的深克隆断言工具对应。
//
// Rust 不提供通用的运行时反射，因此调用方需要先用 [`DeepValue`] 描述待比较的值。
// 带独立存储的变体会显式保留地址，使本工具能像 Go 实现一样区分深克隆与共享存储。

use globset::{Glob, GlobMatcher};

#[derive(Clone, Debug)]
/// 可递归比较的动态值表示，覆盖 Go 版反射辅助工具支持的值种类。
///
/// 指针、切片和映射同时保存内容与地址；地址只用于判断底层存储是否共享，
/// 内容仍按结构递归比较。
pub enum DeepValue {
    Invalid,
    Struct {
        type_name: String,
        fields: Vec<(String, DeepValue)>,
    },
    Pointer {
        type_name: String,
        address: usize,
        value: Option<Box<DeepValue>>,
    },
    Slice {
        type_name: String,
        address: usize,
        value: Option<Vec<DeepValue>>,
    },
    Array {
        type_name: String,
        values: Vec<DeepValue>,
    },
    Bool(bool),
    Signed {
        type_name: &'static str,
        value: i64,
    },
    Unsigned {
        type_name: &'static str,
        value: u64,
    },
    Float {
        type_name: &'static str,
        value: f64,
    },
    String(String),
    Map {
        type_name: String,
        address: usize,
        value: Option<Vec<(String, DeepValue)>>,
    },
    Interface {
        type_name: String,
        value: Option<Box<DeepValue>>,
    },
    Function(Option<usize>),
    Channel(Option<usize>),
    Unsupported(String),
}

impl DeepValue {
    pub fn structure<K, V, I>(type_name: impl Into<String>, fields: I) -> Self
    where
        K: Into<String>,
        V: Into<DeepValue>,
        I: IntoIterator<Item = (K, V)>,
    {
        Self::Struct {
            type_name: type_name.into(),
            fields: fields
                .into_iter()
                .map(|(name, value)| (name.into(), value.into()))
                .collect(),
        }
    }

    pub fn pointer(
        type_name: impl Into<String>,
        address: usize,
        value: impl Into<DeepValue>,
    ) -> Self {
        Self::Pointer {
            type_name: type_name.into(),
            address,
            value: Some(Box::new(value.into())),
        }
    }

    pub fn nil_pointer(type_name: impl Into<String>) -> Self {
        Self::Pointer {
            type_name: type_name.into(),
            address: 0,
            value: None,
        }
    }

    pub fn slice<V, I>(address: usize, values: I) -> Self
    where
        V: Into<DeepValue>,
        I: IntoIterator<Item = V>,
    {
        let values: Vec<_> = values.into_iter().map(Into::into).collect();
        let type_name = values
            .first()
            .map_or("unknown", DeepValue::type_name)
            .to_owned();
        Self::Slice {
            type_name,
            address,
            value: Some(values),
        }
    }

    pub fn nil_slice(type_name: impl Into<String>) -> Self {
        Self::Slice {
            type_name: type_name.into(),
            address: 0,
            value: None,
        }
    }

    pub fn array<V, I>(type_name: impl Into<String>, values: I) -> Self
    where
        V: Into<DeepValue>,
        I: IntoIterator<Item = V>,
    {
        Self::Array {
            type_name: type_name.into(),
            values: values.into_iter().map(Into::into).collect(),
        }
    }

    pub fn map<K, V, I>(address: usize, entries: I) -> Self
    where
        K: Into<String>,
        V: Into<DeepValue>,
        I: IntoIterator<Item = (K, V)>,
    {
        Self::Map {
            type_name: "map".to_owned(),
            address,
            value: Some(
                entries
                    .into_iter()
                    .map(|(key, value)| (key.into(), value.into()))
                    .collect(),
            ),
        }
    }

    pub fn nil_map(type_name: impl Into<String>) -> Self {
        Self::Map {
            type_name: type_name.into(),
            address: 0,
            value: None,
        }
    }

    pub fn interface(type_name: impl Into<String>, value: impl Into<DeepValue>) -> Self {
        Self::Interface {
            type_name: type_name.into(),
            value: Some(Box::new(value.into())),
        }
    }

    pub fn nil_interface(type_name: impl Into<String>) -> Self {
        Self::Interface {
            type_name: type_name.into(),
            value: None,
        }
    }

    pub fn function(address: usize) -> Self {
        Self::Function(Some(address))
    }
    pub fn nil_function() -> Self {
        Self::Function(None)
    }
    pub fn channel(address: usize) -> Self {
        Self::Channel(Some(address))
    }
    pub fn nil_channel() -> Self {
        Self::Channel(None)
    }

    fn type_name(&self) -> &str {
        match self {
            Self::Invalid => "<invalid>",
            Self::Struct { type_name, .. }
            | Self::Pointer { type_name, .. }
            | Self::Slice { type_name, .. }
            | Self::Array { type_name, .. }
            | Self::Map { type_name, .. }
            | Self::Interface { type_name, .. }
            | Self::Unsupported(type_name) => type_name,
            Self::Bool(_) => "bool",
            Self::Signed { type_name, .. }
            | Self::Unsigned { type_name, .. }
            | Self::Float { type_name, .. } => type_name,
            Self::String(_) => "String",
            Self::Function(_) => "fn",
            Self::Channel(_) => "chan",
        }
    }

    fn same_type(&self, other: &Self) -> bool {
        std::mem::discriminant(self) == std::mem::discriminant(other)
            && self.type_name() == other.type_name()
    }
}

macro_rules! signed_from {
    ($($ty:ty),+ $(,)?) => {$(impl From<$ty> for DeepValue {
        fn from(value: $ty) -> Self { Self::Signed { type_name: stringify!($ty), value: value as i64 } }
    })+};
}
macro_rules! unsigned_from {
    ($($ty:ty),+ $(,)?) => {$(impl From<$ty> for DeepValue {
        fn from(value: $ty) -> Self { Self::Unsigned { type_name: stringify!($ty), value: value as u64 } }
    })+};
}
macro_rules! float_from {
    ($($ty:ty),+ $(,)?) => {$(impl From<$ty> for DeepValue {
        fn from(value: $ty) -> Self { Self::Float { type_name: stringify!($ty), value: value as f64 } }
    })+};
}

signed_from!(i8, i16, i32, i64, isize);
unsigned_from!(u8, u16, u32, u64, usize);
float_from!(f32, f64);
impl From<bool> for DeepValue {
    fn from(value: bool) -> Self {
        Self::Bool(value)
    }
}
impl From<String> for DeepValue {
    fn from(value: String) -> Self {
        Self::String(value)
    }
}
impl From<&str> for DeepValue {
    fn from(value: &str) -> Self {
        Self::String(value.to_owned())
    }
}

#[derive(Clone, Debug)]
enum OptionKind {
    Ignore,
    ComparePointer,
}

#[derive(Clone, Debug)]
/// 一组路径匹配规则及其比较策略。
pub struct TestOption {
    kind: OptionKind,
    patterns: Vec<String>,
}

#[allow(non_snake_case)]
/// 指定递归比较时完全跳过的路径 glob。
pub fn WithIgnorePath<S, I>(paths: I) -> TestOption
where
    S: Into<String>,
    I: IntoIterator<Item = S>,
{
    TestOption {
        kind: OptionKind::Ignore,
        patterns: paths.into_iter().map(Into::into).collect(),
    }
}

#[allow(non_snake_case)]
/// 指定只比较存储地址、不再递归比较内容的路径 glob。
pub fn WithPointerComparePath<S, I>(paths: I) -> TestOption
where
    S: Into<String>,
    I: IntoIterator<Item = S>,
{
    TestOption {
        kind: OptionKind::ComparePointer,
        patterns: paths.into_iter().map(Into::into).collect(),
    }
}

#[derive(Default)]
struct StaticTestHelper {
    ignore_path: Vec<GlobMatcher>,
    pointer_compare_path: Vec<GlobMatcher>,
}

impl StaticTestHelper {
    fn apply_options(&mut self, options: impl IntoIterator<Item = TestOption>) {
        for option in options {
            let matchers = option
                .patterns
                .into_iter()
                .map(|path| {
                    Glob::new(&path)
                        .unwrap_or_else(|error| panic!("invalid path glob {path:?}: {error}"))
                        .compile_matcher()
                })
                .collect();
            match option.kind {
                OptionKind::Ignore => self.ignore_path = matchers,
                OptionKind::ComparePointer => self.pointer_compare_path = matchers,
            }
        }
    }

    fn should_ignore(&self, path: &str) -> bool {
        self.ignore_path.iter().any(|glob| glob.is_match(path))
    }

    fn should_compare_pointer(&self, path: &str) -> bool {
        self.pointer_compare_path
            .iter()
            .any(|glob| glob.is_match(path))
    }

    fn assert_recursively_not_equal(&self, a: &DeepValue, b: &DeepValue, path: &str) {
        if self.should_ignore(path) {
            return;
        }
        if matches!(a, DeepValue::Invalid) || matches!(b, DeepValue::Invalid) {
            assert!(
                !(matches!(a, DeepValue::Invalid) && matches!(b, DeepValue::Invalid)),
                "{path} should not be zero value at the same time"
            );
            return;
        }
        if !a.same_type(b) {
            return;
        }

        // 类型不同已足以证明不相等；类型相同时继续验证每个可比较的子值均不同。
        match (a, b) {
            (DeepValue::Struct { fields: a, .. }, DeepValue::Struct { fields: b, .. }) => {
                for ((name, a), (_, b)) in a.iter().zip(b) {
                    self.assert_recursively_not_equal(a, b, &format!("{path}.{name}"));
                }
            }
            (
                DeepValue::Pointer {
                    address: aa,
                    value: av,
                    ..
                },
                DeepValue::Pointer {
                    address: ba,
                    value: bv,
                    ..
                },
            ) => {
                assert_ne!(aa, ba, "{path} should not be the same");
                // 按指针比较的路径以地址不同为准，不再检查其指向的内容。
                if !self.should_compare_pointer(path) {
                    self.assert_recursively_not_equal_option(av.as_deref(), bv.as_deref(), path);
                }
            }
            (
                DeepValue::Slice {
                    address: aa,
                    value: av,
                    ..
                },
                DeepValue::Slice {
                    address: ba,
                    value: bv,
                    ..
                },
            ) => {
                assert_ne!(aa, ba, "{path} should not be the same");
                if !self.should_compare_pointer(path) {
                    for (index, (a, b)) in values(av).iter().zip(values(bv)).enumerate() {
                        self.assert_recursively_not_equal(a, b, &format!("{path}[{index}]"));
                    }
                }
            }
            (DeepValue::Array { values: a, .. }, DeepValue::Array { values: b, .. }) => {
                for (index, (a, b)) in a.iter().zip(b).enumerate() {
                    self.assert_recursively_not_equal(a, b, &format!("{path}[{index}]"));
                }
            }
            (DeepValue::Bool(a), DeepValue::Bool(b)) => {
                assert_ne!(a, b, "{path} should not be the same")
            }
            (DeepValue::Signed { value: a, .. }, DeepValue::Signed { value: b, .. }) => {
                assert_ne!(a, b, "{path} should not be the same")
            }
            (DeepValue::Unsigned { value: a, .. }, DeepValue::Unsigned { value: b, .. }) => {
                assert_ne!(a, b, "{path} should not be the same")
            }
            (DeepValue::Float { value: a, .. }, DeepValue::Float { value: b, .. }) => {
                assert_ne!(a, b, "{path} should not be the same")
            }
            (DeepValue::String(a), DeepValue::String(b)) => {
                assert_ne!(a, b, "{path} should not be the same")
            }
            (
                DeepValue::Map {
                    address: aa,
                    value: av,
                    ..
                },
                DeepValue::Map {
                    address: ba,
                    value: bv,
                    ..
                },
            ) => {
                assert_ne!(aa, ba, "{path} should not be the same");
                if !self.should_compare_pointer(path) {
                    for (key, a) in map_values(av) {
                        if let Some((_, b)) = map_values(bv).iter().find(|(other, _)| other == key)
                        {
                            self.assert_recursively_not_equal(a, b, &format!("{path}[{key}]"));
                        }
                    }
                }
            }
            (DeepValue::Interface { value: a, .. }, DeepValue::Interface { value: b, .. }) => {
                if a.is_none() || b.is_none() {
                    assert!(
                        !(a.is_none() && b.is_none()),
                        "{path} should not be nil at the same time"
                    );
                    return;
                }
                self.assert_recursively_not_equal(
                    a.as_deref().unwrap(),
                    b.as_deref().unwrap(),
                    path,
                );
            }
            (DeepValue::Function(a), DeepValue::Function(b)) => {
                assert!(
                    self.should_compare_pointer(path),
                    "{path}: a function should be compared by pointer or ignored, because there's no way to compare its content"
                );
                assert_ne!(a, b, "{path} should be different");
            }
            _ => panic!("{path}: unsupported type {}", a.type_name()),
        }
    }

    fn assert_recursively_not_equal_option(
        &self,
        a: Option<&DeepValue>,
        b: Option<&DeepValue>,
        path: &str,
    ) {
        match (a, b) {
            (Some(a), Some(b)) => self.assert_recursively_not_equal(a, b, path),
            (None, None) => panic!("{path} should not be zero value at the same time"),
            _ => {}
        }
    }

    fn assert_deep_cloned_equal(&self, a: &DeepValue, b: &DeepValue, path: &str) {
        if self.should_ignore(path) {
            return;
        }
        assert!(
            a.same_type(b),
            "{path} should have the same type ({} != {})",
            a.type_name(),
            b.type_name()
        );
        match (a, b) {
            (DeepValue::Invalid, DeepValue::Invalid) => {}
            (DeepValue::Struct { fields: a, .. }, DeepValue::Struct { fields: b, .. }) => {
                assert_eq!(a.len(), b.len(), "{path} should have the same field count");
                for ((name, a), (other_name, b)) in a.iter().zip(b) {
                    assert_eq!(name, other_name, "{path} should have the same fields");
                    self.assert_deep_cloned_equal(a, b, &format!("{path}.{name}"));
                }
            }
            (
                DeepValue::Pointer {
                    address: aa,
                    value: av,
                    ..
                },
                DeepValue::Pointer {
                    address: ba,
                    value: bv,
                    ..
                },
            ) => {
                if av.is_none() && bv.is_none() {
                    return;
                }
                assert!(av.is_some() && bv.is_some(), "{path} should not be nil");
                // 默认要求地址不同且内容相等；显式按指针比较时则要求共享同一地址。
                if self.should_compare_pointer(path) {
                    assert_eq!(aa, ba, "{path} should be the same");
                } else {
                    assert_ne!(aa, ba, "{path} should be different");
                    self.assert_deep_cloned_equal(
                        av.as_deref().unwrap(),
                        bv.as_deref().unwrap(),
                        path,
                    );
                }
            }
            (
                DeepValue::Slice {
                    address: aa,
                    value: av,
                    ..
                },
                DeepValue::Slice {
                    address: ba,
                    value: bv,
                    ..
                },
            ) => {
                if av.is_none() && bv.is_none() {
                    return;
                }
                let (av, bv) = (values(av), values(bv));
                assert_eq!(av.len(), bv.len(), "{path} should have the same length");
                if self.should_compare_pointer(path) {
                    assert_eq!(aa, ba, "{path} should be the same");
                } else {
                    assert_ne!(aa, ba, "{path} should not be the same");
                    for (index, (a, b)) in av.iter().zip(bv).enumerate() {
                        self.assert_deep_cloned_equal(a, b, &format!("{path}[{index}]"));
                    }
                }
            }
            (DeepValue::Array { values: a, .. }, DeepValue::Array { values: b, .. }) => {
                assert_eq!(a.len(), b.len(), "{path} should have the same length");
                for (index, (a, b)) in a.iter().zip(b).enumerate() {
                    self.assert_deep_cloned_equal(a, b, &format!("{path}[{index}]"));
                }
            }
            (DeepValue::Bool(a), DeepValue::Bool(b)) => {
                assert_eq!(a, b, "{path} should be the same")
            }
            (DeepValue::Signed { value: a, .. }, DeepValue::Signed { value: b, .. }) => {
                assert_eq!(a, b, "{path} should be the same")
            }
            (DeepValue::Unsigned { value: a, .. }, DeepValue::Unsigned { value: b, .. }) => {
                assert_eq!(a, b, "{path} should be the same")
            }
            (DeepValue::Float { value: a, .. }, DeepValue::Float { value: b, .. }) => {
                assert_eq!(a, b, "{path} should be the same")
            }
            (DeepValue::String(a), DeepValue::String(b)) => {
                assert_eq!(a, b, "{path} should be the same")
            }
            (
                DeepValue::Map {
                    address: aa,
                    value: av,
                    ..
                },
                DeepValue::Map {
                    address: ba,
                    value: bv,
                    ..
                },
            ) => {
                if av.is_none() && bv.is_none() {
                    return;
                }
                let (av, bv) = (map_values(av), map_values(bv));
                assert_eq!(av.len(), bv.len(), "{path} should have the same length");
                if self.should_compare_pointer(path) {
                    assert_eq!(aa, ba, "{path} should be the same");
                } else {
                    assert_ne!(aa, ba, "{path} should not be the same");
                    for (key, a) in av {
                        let b = bv
                            .iter()
                            .find(|(other, _)| other == key)
                            .unwrap_or_else(|| panic!("{path}[{key}] should exist"));
                        self.assert_deep_cloned_equal(a, &b.1, &format!("{path}[{key}]"));
                    }
                }
            }
            (DeepValue::Interface { value: a, .. }, DeepValue::Interface { value: b, .. }) => {
                if a.is_none() && b.is_none() {
                    return;
                }
                assert!(
                    a.is_some() && b.is_some(),
                    "{path} should both be nil or non-nil"
                );
                self.assert_deep_cloned_equal(a.as_deref().unwrap(), b.as_deref().unwrap(), path);
            }
            (DeepValue::Function(a), DeepValue::Function(b)) => {
                if a.is_none() && b.is_none() {
                    return;
                }
                assert!(
                    self.should_compare_pointer(path),
                    "{path}: a function should be compared by pointer or ignored, because there's no way to compare its content"
                );
                assert_eq!(a, b, "{path} should be the same");
            }
            (DeepValue::Channel(a), DeepValue::Channel(b)) => {
                assert!(
                    a.is_none() && b.is_none(),
                    "{path} channels should both be nil"
                )
            }
            _ => panic!("{path}: unsupported type {}", a.type_name()),
        }
    }
}

fn values(value: &Option<Vec<DeepValue>>) -> &[DeepValue] {
    value.as_deref().unwrap_or(&[])
}
fn map_values(value: &Option<Vec<(String, DeepValue)>>) -> &[(String, DeepValue)] {
    value.as_deref().unwrap_or(&[])
}

#[allow(non_snake_case)]
/// 断言两个值从根路径 `$` 起递归地互不相等。
pub fn AssertRecursivelyNotEqual<A, B, O>(a: A, b: B, options: O)
where
    A: Into<DeepValue>,
    B: Into<DeepValue>,
    O: IntoIterator<Item = TestOption>,
{
    let mut helper = StaticTestHelper::default();
    helper.apply_options(options);
    helper.assert_recursively_not_equal(&a.into(), &b.into(), "$");
}

#[allow(non_snake_case)]
/// 断言两个值内容深度相等，同时默认不共享指针、切片或映射的底层存储。
pub fn AssertDeepClonedEqual<A, B, O>(a: A, b: B, options: O)
where
    A: Into<DeepValue>,
    B: Into<DeepValue>,
    O: IntoIterator<Item = TestOption>,
{
    let mut helper = StaticTestHelper::default();
    helper.apply_options(options);
    helper.assert_deep_cloned_equal(&a.into(), &b.into(), "$");
}
