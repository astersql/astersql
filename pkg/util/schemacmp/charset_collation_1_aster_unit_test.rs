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

// Aster 补充单元测试：charset/collation 与更广的格原语、表编码路径。
//
// 覆盖偏序、基础格元素、Map Join、MySQL 类型序以及 `Encode`/`Join` 表结构语义。

use super::*;
use std::collections::HashMap;

/// 构造大小写不敏感标识符（CIStr：O 原样、L 小写）。
fn ci_string(value: &str) -> ast::CIStr {
    ast::CIStr {
        O: value.to_owned(),
        L: value.to_lowercase(),
    }
}

/// 从格 `Unwrap` 出具体类型 T。
fn unwrapped<T: Clone + 'static>(value: &dyn Lattice) -> T {
    value
        .Unwrap()
        .downcast_ref::<T>()
        .expect("unexpected lattice value type")
        .clone()
}

/// charset/collation 偏序与 Join 行为对齐 Go。
#[test]
fn charset_and_collation_follow_go_partial_order() {
    assert_eq!(Charset("UTF8MB3").Compare(&Charset("utf8")).unwrap(), 0);
    assert_eq!(Charset("latin1").Compare(&Charset("utf8mb4")).unwrap(), -1);
    assert_eq!(Charset("utf8mb4").Compare(&Charset("utf8")).unwrap(), 1);
    assert_eq!(
        Charset("utf8")
            .Compare(&Charset("gbk"))
            .unwrap_err()
            .to_string(),
        "incompatible charset (utf8 vs gbk)"
    );
    assert_eq!(
        unwrapped::<String>(Charset("latin1").Join(&Charset("utf8")).unwrap().as_ref()),
        "utf8mb4"
    );

    assert_eq!(
        Collation("UTF8MB3_BIN")
            .Compare(&Collation("utf8_bin"))
            .unwrap(),
        0
    );
    assert_eq!(
        Collation("latin1_bin")
            .Compare(&Collation("utf8mb4_bin"))
            .unwrap(),
        -1
    );
    assert_eq!(
        Collation("utf8_bin")
            .Compare(&Collation("utf8_general_ci"))
            .unwrap_err()
            .to_string(),
        "incompatible collation (utf8_bin vs utf8_general_ci)"
    );
    assert_eq!(
        unwrapped::<String>(
            Collation("latin1_bin")
                .Join(&Collation("utf8_bin"))
                .unwrap()
                .as_ref()
        ),
        "utf8mb4_bin"
    );
}

/// Bool/Singleton/BitSet/Tuple/Maybe/StringList 等基础格规则。
#[test]
fn primitive_tuple_maybe_and_string_list_match_go_lattice_rules() {
    assert_eq!(Bool(false).Compare(&Bool(true)).unwrap(), -1);
    assert_eq!(
        unwrapped::<bool>(Bool(false).Join(&Bool(true)).unwrap().as_ref()),
        true
    );
    assert!(Singleton(1_i64).Compare(Singleton(2_i64).as_ref()).is_err());

    assert!(BitSet(0b010110).Compare(&BitSet(0b110001)).is_err());
    assert_eq!(
        unwrapped::<usize>(BitSet(0b010110).Join(&BitSet(0b110001)).unwrap().as_ref()),
        0b110111
    );

    let left = Tuple(vec![Box::new(Byte(123)), Box::new(Bool(false))]);
    let right = Tuple(vec![Box::new(Byte(67)), Box::new(Bool(true))]);
    assert_eq!(
        left.Compare(&right).unwrap_err().to_string(),
        "at tuple index 1: combining contradicting orders (1 && -1)"
    );
    let joined = left.Join(&right).unwrap();
    let joined = joined.as_any().downcast_ref::<Tuple>().unwrap();
    assert_eq!(unwrapped::<u8>(joined.0[0].as_ref()), 123);
    assert_eq!(unwrapped::<bool>(joined.0[1].as_ref()), true);

    assert_eq!(
        Maybe(None)
            .Compare(Maybe(Some(Box::new(Byte(3)))).as_ref())
            .unwrap(),
        -1
    );
    assert_eq!(
        StringList(vec!["a".into(), "b".into()])
            .Compare(&StringList(vec!["a".into(), "b".into(), "c".into()]))
            .unwrap(),
        -1
    );
}

/// 以 usize 为值的简单格，按数值大小比较，Join 取 max。
#[derive(Clone, Copy, Default)]
struct LatticeUsize(usize);

/// 字符串键到 `LatticeUsize` 的 Map 适配器，供 `Map` 格测试。
#[derive(Clone, Default)]
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
        self.0
            .insert(key, LatticeUsize(unwrapped::<usize>(value.as_ref())));
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

/// Map 比较不相容时 Join 按键合并取上确界。
#[test]
fn map_compare_and_join_match_go_rules() {
    let left = Map(Box::new(UintMap(HashMap::from([
        ("a".into(), LatticeUsize(123)),
        ("b".into(), LatticeUsize(678)),
        ("c".into(), LatticeUsize(456)),
    ]))));
    let right = Map(Box::new(UintMap(HashMap::from([
        ("a".into(), LatticeUsize(234)),
        ("b".into(), LatticeUsize(567)),
        ("d".into(), LatticeUsize(789)),
    ]))));
    assert!(left.Compare(right.as_ref()).is_err());
    let joined = left.Join(right.as_ref()).unwrap();
    let values = joined.Unwrap();
    let values = values.downcast_ref::<HashMap<String, AnyValue>>().unwrap();
    assert_eq!(*values["a"].downcast_ref::<usize>().unwrap(), 234);
    assert_eq!(*values["b"].downcast_ref::<usize>().unwrap(), 678);
    assert_eq!(values.len(), 4);
}

/// MySQL 整数 / BLOB 类型编号的语义序（非纯编号序）。
#[test]
fn mysql_integer_and_blob_types_keep_go_ordering() {
    assert_eq!(
        FieldTp(mysql::TypeTiny)
            .Compare(FieldTp(mysql::TypeLong).as_ref())
            .unwrap(),
        -1
    );
    assert_eq!(
        *FieldTp(mysql::TypeShort)
            .Join(FieldTp(mysql::TypeInt24).as_ref())
            .unwrap()
            .Unwrap()
            .downcast_ref::<u8>()
            .unwrap(),
        mysql::TypeInt24
    );
    assert_eq!(
        FieldTp(mysql::TypeBlob)
            .Compare(FieldTp(mysql::TypeMediumBlob).as_ref())
            .unwrap(),
        -1
    );
    assert!(
        FieldTp(mysql::TypeLong)
            .Compare(FieldTp(mysql::TypeSet).as_ref())
            .is_err()
    );
}

/// 表信息 Encode/Decode 使用真实 parser/model 类型并生成 CREATE TABLE 文本。
#[test]
fn table_encoding_uses_real_parser_and_model_types() {
    let mut id = model::ColumnInfo::default();
    id.Name = ci_string("id");
    id.FieldType = types::NewFieldType(mysql::TypeLong);
    id.FieldType.SetFlag(mysql::PriKeyFlag | mysql::NotNullFlag);

    let mut payload = model::ColumnInfo::default();
    payload.Name = ci_string("payload");
    payload.FieldType = types::NewFieldType(mysql::TypeVarchar);
    payload.FieldType.SetFlen(20);
    payload.FieldType.SetCharset("utf8".into());
    payload.FieldType.SetCollate("utf8_bin".into());

    let info = model::TableInfo {
        Name: ci_string("source"),
        Columns: vec![id, payload],
        ..Default::default()
    };
    let encoded = Encode(&info);
    let decoded = DecodeColumnFieldTypes(&encoded);
    assert_eq!(decoded["id"].GetType(), mysql::TypeLong);
    assert_eq!(decoded["payload"].GetCollate(), "utf8_bin");
    assert_eq!(encoded.Compare(encoded.clone()).unwrap(), 0);
    let sql = encoded.String().to_lowercase();
    assert!(sql.contains("create table `tbl`"));
    assert!(sql.contains("`payload` varchar(20) character set utf8 collate utf8_bin"));
}

/// 表 Join：缺失的 NOT NULL 无默认列补标准 DEFAULT（如 0）。
#[test]
fn table_join_fills_standard_default_for_missing_not_null_column() {
    let mut base = model::ColumnInfo::default();
    base.Name = ci_string("id");
    base.FieldType = types::NewFieldType(mysql::TypeLong);

    let left = model::TableInfo {
        Name: ci_string("left"),
        Columns: vec![base.clone()],
        ..Default::default()
    };

    let mut required = model::ColumnInfo::default();
    required.Name = ci_string("required_value");
    required.FieldType = types::NewFieldType(mysql::TypeLong);
    required
        .FieldType
        .SetFlag(mysql::NotNullFlag | mysql::NoDefaultValueFlag);
    let right = model::TableInfo {
        Name: ci_string("right"),
        Columns: vec![base, required],
        ..Default::default()
    };

    assert!(Encode(&left).Compare(Encode(&right)).is_err());
    let joined = Encode(&left).Join(Encode(&right)).unwrap();
    let decoded = DecodeColumnFieldTypes(&joined);
    assert!(mysql::HasNotNullFlag(decoded["required_value"].GetFlag()));
    assert!(!mysql::HasNoDefaultValueFlag(
        decoded["required_value"].GetFlag()
    ));
    assert!(joined.String().contains("DEFAULT 0"));
}
