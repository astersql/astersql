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

// 内置函数向量化回退路径与列缓冲池的单元测试。
//
// 对应 Go `builtin_vectorized_test.go`：用 mock 内置函数验证 `vecEvalIntByRows` /
// `vecEvalStringByRows` 的值/NULL/错误传播，以及本地与全局列池的复用与并发借还。

use std::any::Any;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::builtin_vectorized_kernel::{
    GetColumn, PutColumn, columnBufferAllocator, emptyLocalColumnPoolSize, newLocalColumnPool,
    vecEvalIntByRows, vecEvalStringByRows,
};
use crate::*;

/// 空用户变量表，测试 EvalContext 不依赖会话变量。
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

/// 仅实现向量化回退测试所需的最小 EvalContext。
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
        11
    }

    fn SQLMode(&self) -> mysql::SQLMode {
        mysql::SQLMode::default()
    }

    fn TypeCtx(&self) -> types::Context {
        panic!("not used by vectorized fallback tests")
    }

    fn ErrCtx(&self) -> errctx::Context {
        panic!("not used by vectorized fallback tests")
    }

    fn Location(&self) -> chrono_tz::Tz {
        chrono_tz::UTC
    }

    fn CurrentTime(
        &self,
    ) -> Result<chrono::DateTime<chrono_tz::Tz>, contextutil::errors::SharedError> {
        panic!("not used by vectorized fallback tests")
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

/// Mirrors Go's mockBuiltinDouble, including NULL and error rows.
/// 对应 Go mockBuiltinDouble：整数翻倍、字符串拼接，并注入 NULL 与错误行。
struct MockBuiltinDouble {
    collation: collationInfo,
    calls: AtomicUsize,
    args: Vec<Box<dyn Expression>>,
    ret_type: types::FieldType,
    pb_code: i32,
    collator: Box<dyn collate::Collator>,
}

impl Default for MockBuiltinDouble {
    fn default() -> Self {
        Self {
            collation: collationInfo::default(),
            calls: AtomicUsize::new(0),
            args: Vec::new(),
            ret_type: *types::NewFieldType(mysql::TypeLonglong),
            pb_code: 0,
            collator: collate::GetBinaryCollator(),
        }
    }
}

impl CollationInfo for MockBuiltinDouble {
    fn HasCoercibility(&self) -> bool {
        self.collation.HasCoercibility()
    }

    fn Coercibility(&self) -> Coercibility {
        self.collation.Coercibility()
    }

    fn SetCoercibility(&self, value: Coercibility) {
        self.collation.SetCoercibility(value);
    }

    fn Repertoire(&self) -> Repertoire {
        self.collation.Repertoire()
    }

    fn SetRepertoire(&mut self, value: Repertoire) {
        self.collation.SetRepertoire(value);
    }

    fn CharsetAndCollation(&self) -> (String, String) {
        self.collation.CharsetAndCollation()
    }

    fn SetCharsetAndCollation(&mut self, charset: String, collation: String) {
        self.collation.SetCharsetAndCollation(charset, collation);
    }

    fn IsExplicitCharset(&self) -> bool {
        self.collation.IsExplicitCharset()
    }

    fn SetExplicitCharset(&mut self, explicit: bool) {
        self.collation.SetExplicitCharset(explicit);
    }
}

impl builtinFunc for MockBuiltinDouble {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn SafeToShareAcrossSession(&self) -> bool {
        true
    }

    fn getArgs(&self) -> &[Box<dyn Expression>] {
        &self.args
    }

    fn getArgsMut(&mut self) -> &mut [Box<dyn Expression>] {
        &mut self.args
    }

    fn equal(&self, _ctx: &dyn EvalContext, other: &dyn builtinFunc) -> bool {
        other.as_any().is::<Self>()
    }

    fn getRetTp(&self) -> &types::FieldType {
        &self.ret_type
    }

    fn setPbCode(&mut self, code: i32) {
        self.pb_code = code;
    }

    fn PbCode(&self) -> i32 {
        self.pb_code
    }

    fn setCollator(&mut self, collator: Box<dyn collate::Collator>) {
        self.collator = collator;
    }

    fn collator(&self) -> &dyn collate::Collator {
        self.collator.as_ref()
    }

    fn Clone(&self) -> Box<dyn builtinFunc> {
        Box::new(Self {
            collation: self.collation.clone(),
            calls: AtomicUsize::new(self.calls.load(Ordering::SeqCst)),
            args: self
                .args
                .iter()
                .map(|argument| argument.CloneExpr())
                .collect(),
            ret_type: self.ret_type.clone(),
            pb_code: self.pb_code,
            collator: self.collator.Clone(),
        })
    }

    fn MemoryUsage(&self) -> i64 {
        std::mem::size_of::<Self>() as i64
    }

    fn vectorized(&self) -> bool {
        true
    }

    fn evalInt(&self, _ctx: &dyn EvalContext, row: chunk::Row) -> Result<(i64, bool), Error> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if row.IsNull(0) {
            return Ok((0, true));
        }
        let value = row.GetInt64(0);
        if value == -99 {
            return Err(errors::New("mock vectorized integer error"));
        }
        Ok((value * 2, false))
    }

    fn evalString(&self, _ctx: &dyn EvalContext, row: chunk::Row) -> Result<(String, bool), Error> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if row.IsNull(0) {
            return Ok((String::new(), true));
        }
        let value = row.GetString(0);
        if value == "error" {
            return Err(errors::New("mock vectorized string error"));
        }
        Ok((format!("{value}{value}"), false))
    }
}

/// 构造仅含一列 Int64 的输入 Chunk，Some/None 对应值与 NULL。
fn integer_chunk(values: &[Option<i64>]) -> Box<chunk::Chunk> {
    let mut input = chunk::New(
        vec![*types::NewFieldType(mysql::TypeLonglong)],
        values.len(),
        values.len(),
    );
    for value in values {
        match value {
            Some(value) => input.AppendInt64(0, *value),
            None => input.AppendNull(0),
        }
    }
    input
}

/// 构造仅含一列字符串的输入 Chunk。
fn string_chunk(values: &[Option<&str>]) -> Box<chunk::Chunk> {
    let mut input = chunk::New(
        vec![*types::NewFieldType(mysql::TypeVarString)],
        values.len(),
        values.len(),
    );
    for value in values {
        match value {
            Some(value) => input.AppendString(0, value),
            None => input.AppendNull(0),
        }
    }
    input
}

/// 逐行整数回退结果与标量 evalInt 一致，含 NULL。
#[test]
fn test_double_row_to_vector_keeps_values_and_nulls() {
    let ctx = TestEvalContext(EmptyUserVars);
    let builtin = MockBuiltinDouble::default();
    let input = integer_chunk(&[Some(-3), None, Some(0), Some(7)]);
    let mut result = chunk::NewColumn(&types::NewFieldType(mysql::TypeLonglong), input.NumRows());

    vecEvalIntByRows(&ctx, &builtin, &input, &mut result).unwrap();

    for row_index in 0..input.NumRows() {
        let (row_value, row_is_null) = builtin.evalInt(&ctx, input.GetRow(row_index)).unwrap();
        assert_eq!(result.IsNull(row_index), row_is_null);
        if !row_is_null {
            assert_eq!(result.GetInt64(row_index), row_value);
        }
    }
    assert_eq!(builtin.calls.load(Ordering::SeqCst), input.NumRows() * 2);
}

/// 逐行字符串回退保留拼接结果与 NULL。
#[test]
fn test_double_vector_to_row_keeps_strings_and_nulls() {
    let ctx = TestEvalContext(EmptyUserVars);
    let builtin = MockBuiltinDouble::default();
    let input = string_chunk(&[Some("ab"), None, Some("西瓜")]);
    let mut result = chunk::NewColumn(&types::NewFieldType(mysql::TypeVarString), input.NumRows());

    vecEvalStringByRows(&builtin, &ctx, &input, &mut result).unwrap();

    assert_eq!(result.GetString(0), "abab");
    assert!(result.IsNull(1));
    assert_eq!(result.GetString(2), "西瓜西瓜");
}

/// 整数回退在首个错误行立即返回，此前写入的结果保留。
#[test]
fn test_vectorized_fallback_stops_at_first_error() {
    let ctx = TestEvalContext(EmptyUserVars);
    let builtin = MockBuiltinDouble::default();
    let input = integer_chunk(&[Some(2), None, Some(-99), Some(8)]);
    let mut result = chunk::NewColumn(&types::NewFieldType(mysql::TypeLonglong), input.NumRows());

    let error = vecEvalIntByRows(&ctx, &builtin, &input, &mut result).unwrap_err();

    assert!(error.to_string().contains("mock vectorized integer error"));
    assert_eq!(builtin.calls.load(Ordering::SeqCst), 3);
    assert_eq!(result.GetInt64(0), 4);
    assert!(result.IsNull(1));
}

/// 字符串回退同样在首个错误处传播，不继续求值后续行。
#[test]
fn test_string_vectorized_fallback_propagates_error() {
    let ctx = TestEvalContext(EmptyUserVars);
    let builtin = MockBuiltinDouble::default();
    let input = string_chunk(&[Some("ok"), Some("error"), Some("unreached")]);
    let mut result = chunk::NewColumn(&types::NewFieldType(mysql::TypeVarString), input.NumRows());

    let error = vecEvalStringByRows(&builtin, &ctx, &input, &mut result).unwrap_err();

    assert!(error.to_string().contains("mock vectorized string error"));
    assert_eq!(builtin.calls.load(Ordering::SeqCst), 2);
    assert_eq!(result.GetString(0), "okok");
}

/// put 后再 get 应复用同一列缓冲指针。
#[test]
fn test_local_column_pool_reuses_returned_column() {
    let allocator = newLocalColumnPool();
    assert_eq!(allocator.MemoryUsage(), emptyLocalColumnPoolSize);

    let mut column = allocator.get().unwrap();
    column.ResizeInt64(0, false);
    column.AppendInt64(42);
    let allocation = (&*column as *const chunk::Column) as usize;
    allocator.put(column);

    let reused = allocator.get().unwrap();
    assert_eq!((&*reused as *const chunk::Column) as usize, allocation);
    assert_eq!(reused.GetInt64(0), 42);
    allocator.put(reused);
}

/// 多线程并发 get/put 不应 panic，池仍可借出列。
#[test]
fn test_local_column_pool_parallel_get_put() {
    let allocator = Arc::new(newLocalColumnPool());
    let workers = (0..5)
        .map(|worker| {
            let allocator = Arc::clone(&allocator);
            std::thread::spawn(move || {
                for iteration in 0..128 {
                    let mut column = allocator.get().unwrap();
                    column.ResizeInt64(0, false);
                    column.AppendInt64(worker * 1_000 + iteration);
                    assert_eq!(column.GetInt64(0), worker * 1_000 + iteration);
                    allocator.put(column);
                }
            })
        })
        .collect::<Vec<_>>();

    for worker in workers {
        worker.join().unwrap();
    }

    let column = allocator.get().unwrap();
    assert_eq!(column.length, 1);
    allocator.put(column);
}

/// 全局 GetColumn/PutColumn 往返后仍可写入读取。
#[test]
fn test_global_column_pool_round_trip() {
    let mut column = GetColumn(types::ETInt, 16).unwrap();
    column.ResizeInt64(0, false);
    column.AppendInt64(1234);
    PutColumn(column);

    let mut reused = GetColumn(types::ETInt, 16).unwrap();
    reused.ResizeInt64(0, false);
    reused.AppendInt64(1234);
    assert_eq!(reused.GetInt64(0), 1234);
    PutColumn(reused);
}
