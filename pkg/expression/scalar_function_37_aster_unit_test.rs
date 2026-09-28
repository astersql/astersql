// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

// ScalarFunction 及相关元数据/向量辅助的 Aster 单元测试。
//
// 覆盖格式化工具、带时区二进制时间戳解码、Schema 列解析、
// 字段名消歧、常量向量化、向量检索表达式解释，以及 NULL 类型推导。

use crate::builtin_vec_kernel::vecL2DistanceFunctionClass;
use crate::util_kernel::{GetFormatBytes, GetFormatNanoTime, binaryTimestampWithTZ};
use crate::vectorized_kernel::genVecFromConstExpr;
use crate::vs_helper_kernel::InterpretVectorSearchExpr;
use crate::*;
use std::sync::Arc;

/// 空用户变量表：测试路径不依赖会话变量。
struct EmptyUserVars;

impl exprctx::UserVarsReader for EmptyUserVars {
    fn GetUserVarVal(&self, _name: &str) -> Option<types::Datum> {
        None
    }

    fn GetUserVarType(&self, _name: &str) -> Option<types::FieldType> {
        None
    }

    fn Clone(&self) -> Box<dyn exprctx::UserVarsReader> {
        Box::new(Self)
    }
}

/// 最小求值上下文桩，仅实现元数据测试所需的固定返回值。
struct TestEvalContext(EmptyUserVars);

impl contextutil::WarnAppender for TestEvalContext {
    fn AppendWarning(&self, _error: contextutil::errors::SharedError) {}

    fn AppendNote(&self, _error: contextutil::errors::SharedError) {}
}

impl contextutil::WarnHandler for TestEvalContext {
    fn WarningCount(&self) -> usize {
        0
    }

    fn TruncateWarnings(&self, _start: isize) -> Vec<contextutil::SQLWarn> {
        Vec::new()
    }

    fn CopyWarnings(&self, destination: Vec<contextutil::SQLWarn>) -> Vec<contextutil::SQLWarn> {
        destination
    }
}

impl exprctx::ParamValues for TestEvalContext {
    fn GetParamValue(&self, _index: usize) -> Result<types::Datum, exprctx::ParamError> {
        Err(exprctx::ParamError::IndexExceedsParamCount)
    }
}

impl EvalContext for TestEvalContext {
    fn CtxID(&self) -> u64 {
        1
    }

    fn SQLMode(&self) -> mysql::SQLMode {
        mysql::SQLMode::default()
    }

    fn TypeCtx(&self) -> types::Context {
        panic!("not used by these scalar metadata tests")
    }

    fn ErrCtx(&self) -> errctx::Context {
        panic!("not used by these scalar metadata tests")
    }

    fn Location(&self) -> chrono_tz::Tz {
        chrono_tz::UTC
    }

    fn CurrentTime(
        &self,
    ) -> Result<chrono::DateTime<chrono_tz::Tz>, contextutil::errors::SharedError> {
        panic!("not used by these scalar metadata tests")
    }

    fn CurrentDB(&self) -> String {
        String::new()
    }

    fn GetMaxAllowedPacket(&self) -> u64 {
        64 << 20
    }

    fn GetTiDBRedactLog(&self) -> String {
        "OFF".to_owned()
    }

    fn GetDefaultWeekFormatMode(&self) -> String {
        "0".to_owned()
    }

    fn GetDivPrecisionIncrement(&self) -> i32 {
        4
    }

    fn GetUserVarsReader(&self) -> &dyn exprctx::UserVarsReader {
        &self.0
    }

    fn GetOptionalPropSet(&self) -> exprctx::OptionalEvalPropKeySet {
        exprctx::OptionalEvalPropKeySet::default()
    }

    fn GetOptionalPropProvider(
        &self,
        _key: exprctx::OptionalEvalPropKey,
    ) -> Option<&dyn exprctx::OptionalEvalPropProvider> {
        None
    }
}

/// 包装 TestEvalContext 的构建上下文桩。
struct TestBuildContext(TestEvalContext);

impl BuildContext for TestBuildContext {
    fn GetEvalCtx(&self) -> &dyn EvalContext {
        &self.0
    }

    fn GetCharsetInfo(&self) -> (String, String) {
        ("utf8mb4".to_owned(), "utf8mb4_bin".to_owned())
    }

    fn GetDefaultCollationForUTF8MB4(&self) -> String {
        "utf8mb4_bin".to_owned()
    }

    fn GetBlockEncryptionMode(&self) -> String {
        "aes-128-ecb".to_owned()
    }

    fn GetSysdateIsNow(&self) -> bool {
        false
    }

    fn GetNoopFuncsMode(&self) -> i32 {
        0
    }

    fn Rng(&self) -> &exprctx::mathutil::MysqlRng {
        panic!("not used by these scalar metadata tests")
    }

    fn IsUseCache(&self) -> bool {
        false
    }

    fn SetSkipPlanCache(&self, _reason: &str) {}

    fn AllocPlanColumnID(&self) -> i64 {
        1
    }

    fn IsInNullRejectCheck(&self) -> bool {
        false
    }

    fn IsConstantPropagateCheck(&self) -> bool {
        false
    }

    fn ConnectionID(&self) -> u64 {
        1
    }

    fn IsReadonlyUserVar(&self, _name: &str) -> bool {
        false
    }
}

/// 构造默认测试用 BuildContext。
fn context() -> TestBuildContext {
    TestBuildContext(TestEvalContext(EmptyUserVars))
}

/// 构造带 UniqueID 的 BIGINT 列。
fn int_column(unique_id: i64) -> Column {
    Column::new(
        *types::NewFieldType(mysql::TypeLonglong),
        unique_id,
        unique_id,
        0,
    )
}

/// 字节/纳秒格式化输出应与 Go 侧一致。
#[test]
fn formats_bytes_and_nanoseconds_like_go() {
    assert_eq!(GetFormatBytes(1024.0), "1.00 KiB");
    assert_eq!(GetFormatBytes(-1_048_576.0), "-1.00 MiB");
    assert_eq!(GetFormatNanoTime(1_000.0), "1.00 us");
    assert_eq!(GetFormatNanoTime(86_400_000_000_000.0), "1.00 d");
}

/// 带时区偏移的二进制时间戳应按 Go 宽度解码。
#[test]
fn binary_timestamp_timezone_uses_go_width() {
    // 布局：年月日时分秒 + 微秒(u32 LE) + 时区分钟偏移(i16 LE)。
    let mut encoded = vec![0xe8, 0x07, 7, 15, 9, 8, 7];
    encoded.extend_from_slice(&123_456_u32.to_le_bytes());
    encoded.extend_from_slice(&480_i16.to_le_bytes());
    let (position, value) = binaryTimestampWithTZ(0, &encoded);
    assert_eq!(position, 13);
    assert_eq!(value, "2024-07-15 09:08:07.123456+8:00");
}

/// Schema 列索引优先完整列，并保留唯一键语义。
#[test]
fn schema_prefers_full_columns_and_preserves_key_semantics() {
    let mut prefix = int_column(7);
    prefix.IsPrefix = true;
    let full = int_column(7);
    let key = int_column(9);
    let mut schema = NewSchema(vec![prefix, full, key.CloneColumn()]);
    schema.SetKeys(vec![vec![key.CloneColumn()]]);

    // 同 UniqueID 时跳过前缀列，命中完整列索引 1。
    assert_eq!(schema.ColumnIndex(&int_column(7)), Some(1));
    assert!(schema.IsUnique(true, &[int_column(100), key]));
    assert_eq!(
        schema.ColumnsIndices(&[int_column(7), int_column(9)]),
        Some(vec![1, 2])
    );
}

/// 字段名解析应跳过冗余列，并在歧义时返回错误。
#[test]
fn field_name_resolution_matches_redundant_and_ambiguous_rules() {
    let field = |redundant| types::FieldName {
        DBName: ast::NewCIStr("db"),
        TblName: ast::NewCIStr("tbl"),
        ColName: ast::NewCIStr("col"),
        Redundant: redundant,
        ..Default::default()
    };
    let column = ast::ColumnName {
        Schema: ast::NewCIStr("db"),
        Table: ast::NewCIStr("tbl"),
        Name: ast::NewCIStr("col"),
    };
    let names = |values: Vec<types::FieldName>| {
        types::NameSlice(
            values
                .into_iter()
                .map(|value| Some(Arc::new(value)))
                .collect(),
        )
    };
    // 冗余列被跳过，命中第二个非冗余字段。
    assert_eq!(
        FindFieldName(&names(vec![field(true), field(false)]), &column).unwrap(),
        Some(1)
    );
    assert!(FindFieldName(&names(vec![field(false), field(false)]), &column).is_err());
    assert_eq!(FindFieldNameIdxByColName(&[field(false)], "missing"), None);
}

/// 常量向量化应按输入行数重复值；空输入重置结果列。
#[test]
fn constant_vectorization_repeats_values_and_resets_empty_input() {
    let ctx = TestEvalContext(EmptyUserVars);
    let constant = Constant::with_type(
        types::NewIntDatum(42),
        *types::NewFieldType(mysql::TypeLonglong),
    );
    let mut input = chunk::NewChunkWithCapacity(Vec::<types::FieldType>::new(), 3);
    input.SetNumVirtualRows(3);
    let mut result = chunk::Column::default();
    genVecFromConstExpr(&ctx, &constant, types::ETInt, Some(&input), &mut result).unwrap();
    assert_eq!(result.Int64s(), vec![42, 42, 42]);
    assert!((0..3).all(|index| !result.IsNull(index)));

    let empty = chunk::NewChunkWithCapacity(Vec::<types::FieldType>::new(), 0);
    genVecFromConstExpr(&ctx, &constant, types::ETInt, Some(&empty), &mut result).unwrap();
    assert_eq!(result.Rows(), 0);
    assert!(result.Int64s().is_empty());
}

/// 向量检索表达式需恰好一列向量列与一个向量常量。
#[test]
fn vector_search_requires_one_vector_column_and_one_vector_constant() {
    let ctx = context();
    let vector_type = *types::NewFieldType(mysql::TypeTiDBVectorFloat32);
    let column = Column::new(vector_type.clone(), 11, 11, 0);
    let constant = Constant::with_type(
        types::NewVectorFloat32Datum(types::ParseVectorFloat32("[1,2]").unwrap()),
        vector_type,
    );
    let class = vecL2DistanceFunctionClass {
        baseFunctionClass: baseFunctionClass::new(ast::VecL2Distance, 2, 2),
    };
    let function = class
        .getFunction(&ctx, vec![Box::new(column), Box::new(constant)])
        .unwrap();
    let expression = ScalarFunction {
        FuncName: ast::NewCIStr(ast::VecL2Distance),
        RetType: Some(function.getRetTp().clone()),
        Function: function,
        hashcode: Vec::new(),
        canonicalhashcode: Vec::new(),
    };
    let info = InterpretVectorSearchExpr(&expression).unwrap();
    assert_eq!(info.DistanceFnName.L, ast::VecL2Distance);
    assert_eq!(info.Vec.Elements(), &[1.0, 2.0]);
    assert_eq!(info.Column.UniqueID, 11);
}

/// GetSingleColumn 应识别减法方向（常量减列 → descending）。
#[test]
fn scalar_single_column_tracks_subtraction_direction() {
    let column = int_column(21);
    let constant = Constant::with_type(
        types::NewIntDatum(5),
        *types::NewFieldType(mysql::TypeLonglong),
    );
    let function = Box::new(crate::planner_bridge_kernel::BuiltinGroupingImplSig::new(
        vec![Box::new(constant), Box::new(column)],
    ));
    let expression = ScalarFunction {
        FuncName: ast::NewCIStr(ast::Minus),
        RetType: Some(*types::NewFieldType(mysql::TypeLonglong)),
        Function: function,
        hashcode: Vec::new(),
        canonicalhashcode: Vec::new(),
    };
    let (found, descending) = expression.GetSingleColumn(false);
    assert_eq!(found.unwrap().UniqueID, 21);
    assert!(descending);
}

/// NULL 类型推导克隆非空操作数类型并清除 NotNull 标志。
#[test]
fn null_type_inference_clones_type_and_removes_not_null() {
    let ctx = TestEvalContext(EmptyUserVars);
    let mut target_type = types::NewFieldType(mysql::TypeLonglong);
    target_type.AddFlag(mysql::NotNullFlag);
    let null = Constant::with_type(
        types::Datum::default(),
        *types::NewFieldType(mysql::TypeNull),
    );
    let value = Constant::with_type(types::NewIntDatum(1), *target_type);
    let mut args: Vec<Box<dyn Expression>> = vec![Box::new(null), Box::new(value)];
    typeInferForNull(&ctx, &mut args);
    assert_eq!(args[0].GetType(&ctx).GetType(), mysql::TypeLonglong);
    assert_eq!(args[0].GetType(&ctx).GetFlag() & mysql::NotNullFlag, 0);
}
