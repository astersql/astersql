// Copyright 2026 AsterSQL.
// Copyright 2016 PingCAP, Inc.
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

// 索引选项累积与 `IndexKVGenerator` 键值生成行为的迁移单测。

use std::cell::Cell;

use super::index::*;
use chrono_tz::UTC;
use errctx_dependency::errctx::Context;
use kv_dependency::{Handle, IntHandle};
use model_dependency::{IndexInfo, TableInfo};
use types_dependency::datum::Datum;

/// 记录 GenIndexKey/GenIndexValue 调用次数并可注入失败的测试用 Index。
#[derive(Default)]
struct RecordingIndex {
    /// GenIndexKey 调用计数。
    key_calls: Cell<usize>,
    /// GenIndexValue 调用计数。
    value_calls: Cell<usize>,
    /// 为真时 GenIndexKey 返回错误。
    fail_key: Cell<bool>,
    /// 为真时 GenIndexValue 返回错误。
    fail_value: Cell<bool>,
}

impl Index for RecordingIndex {
    fn Meta(&self) -> &IndexInfo {
        panic!("not used by generator tests")
    }

    fn TableMeta(&self) -> &TableInfo {
        panic!("not used by generator tests")
    }

    fn MeetPartialCondition(&self, _row: &[Datum]) -> IndexResult<bool> {
        panic!("not used by generator tests")
    }

    fn MeetPartialConditionWithChunk(&self, _row: chunk_dependency::Row) -> IndexResult<bool> {
        panic!("not used by generator tests")
    }

    fn Create(
        &self,
        _ctx: &mut dyn IndexMutateContext,
        _txn: &mut dyn kv_dependency::Transaction,
        _indexed_values: &[Datum],
        _handle: &dyn Handle,
        _handle_restore_data: &[Datum],
        _opts: &[&dyn CreateIdxOption],
    ) -> IndexResult<Box<dyn Handle>> {
        panic!("not used by generator tests")
    }

    fn Delete(
        &self,
        _ctx: &mut dyn IndexMutateContext,
        _txn: &mut dyn kv_dependency::Transaction,
        _indexed_values: &[Datum],
        _handle: &dyn Handle,
    ) -> IndexResult<()> {
        panic!("not used by generator tests")
    }

    fn GenIndexKVIter<'index>(
        &'index self,
        error_context: Context,
        location: chrono_tz::Tz,
        indexed_values: Vec<Datum>,
        handle: Box<dyn Handle>,
        handle_restore_data: Vec<Datum>,
    ) -> IndexKVGenerator<'index, Self> {
        NewPlainIndexKVGenerator(
            self,
            error_context,
            location,
            handle,
            handle_restore_data,
            indexed_values,
        )
    }

    fn Exist(
        &self,
        _ec: &Context,
        _loc: chrono_tz::Tz,
        _txn: &dyn kv_dependency::Transaction,
        _indexed_values: &[Datum],
        _handle: &dyn Handle,
    ) -> IndexResult<(bool, Option<Box<dyn Handle>>)> {
        panic!("not used by generator tests")
    }

    fn GenIndexKey(
        &self,
        _ec: &Context,
        _loc: chrono_tz::Tz,
        indexed_values: &[Datum],
        handle: &dyn Handle,
        mut buffer: Vec<u8>,
    ) -> IndexResult<(Vec<u8>, bool)> {
        self.key_calls.set(self.key_calls.get() + 1);
        if self.fail_key.get() {
            return Err(errors_dependency::New("key failure"));
        }
        // 编码：列数 + handle 大端字节；偶数 handle 视为 distinct。
        buffer.push(indexed_values.len() as u8);
        buffer.extend_from_slice(&handle.IntValue().to_be_bytes());
        Ok((buffer, handle.IntValue() % 2 == 0))
    }

    fn GenIndexValue(
        &self,
        _ec: &Context,
        _loc: chrono_tz::Tz,
        distinct: bool,
        untouched: bool,
        indexed_values: &[Datum],
        handle: &dyn Handle,
        restored_data: &[Datum],
        mut buffer: Vec<u8>,
    ) -> IndexResult<Vec<u8>> {
        self.value_calls.set(self.value_calls.get() + 1);
        if self.fail_value.get() {
            return Err(errors_dependency::New("value failure"));
        }
        buffer.extend_from_slice(&[
            u8::from(distinct),
            u8::from(untouched),
            indexed_values.len() as u8,
            restored_data.len() as u8,
            handle.IntValue() as u8,
        ]);
        Ok(buffer)
    }

    fn FetchValues(&self, row: &[Datum], mut columns: Vec<Datum>) -> IndexResult<Vec<Datum>> {
        columns.extend_from_slice(row);
        Ok(columns)
    }
}

/// 构造严格无警告的错误上下文。
fn context() -> Context {
    errctx_dependency::errctx::StrictNoWarningContext.clone()
}

/// 创建索引选项应按 Go 可变参数风格默认并累积。
#[test]
fn create_index_options_default_and_accumulate_like_go_variadic_options() {
    let default = NewCreateIdxOpt(&[]);
    assert!(!default.IgnoreAssertion());
    assert!(!default.FromBackFill());

    let options: [&dyn CreateIdxOption; 4] = [
        &WithIgnoreAssertion,
        &FromBackfill,
        &DupKeyCheckMode::DupKeyCheckLazy,
        &PessimisticLazyDupKeyCheckMode::DupKeyCheckInPrewrite,
    ];
    let configured = NewCreateIdxOpt(&options);
    assert!(configured.IgnoreAssertion());
    assert!(configured.FromBackFill());
    assert_eq!(configured.DupKeyCheck(), DupKeyCheckMode::DupKeyCheckLazy);
    assert_eq!(
        configured.PessimisticLazyDupKeyCheck(),
        PessimisticLazyDupKeyCheckMode::DupKeyCheckInPrewrite
    );
}

/// 编译期断言 Index 可作为对象安全 trait 对象使用。
fn assert_index_is_object_safe(_: &dyn Index) {}

/// Index 契约应对 table 接口切片保持对象安全。
#[test]
fn index_contract_remains_object_safe_for_table_interface_slices() {
    let index = RecordingIndex::default();
    assert_index_is_object_safe(&index);
}

/// 普通生成器复用调用方缓冲区、只生成一次，并保留传入前缀字节。
#[test]
fn plain_generator_reuses_buffers_generates_once_and_preserves_arguments() {
    let index = RecordingIndex::default();
    let mut generator = NewPlainIndexKVGenerator(
        &index,
        context(),
        UTC,
        Box::new(IntHandle(8)),
        vec![Datum::default()],
        vec![Datum::default(), Datum::default()],
    );

    assert!(generator.Valid());
    let (key, value, distinct) = generator.Next(vec![0xaa], vec![0xbb]).unwrap();
    assert!(!generator.Valid());
    assert!(distinct);
    assert_eq!(key[0], 0xaa);
    assert_eq!(key[1], 2);
    assert_eq!(&key[2..], &8_i64.to_be_bytes());
    assert_eq!(value, vec![0xbb, 1, 0, 2, 1, 8]);
    assert_eq!(index.key_calls.get(), 1);
    assert_eq!(index.value_calls.get(), 1);
}

/// 多值索引生成器应逐值产出，耗尽后变为无效。
#[test]
fn multi_value_generator_visits_every_value_and_then_becomes_invalid() {
    let index = RecordingIndex::default();
    let values = vec![
        vec![Datum::default()],
        vec![Datum::default(), Datum::default(), Datum::default()],
    ];
    let mut generator = NewMultiValueIndexKVGenerator(
        &index,
        context(),
        UTC,
        Box::new(IntHandle(3)),
        Vec::new(),
        values,
    );

    assert!(generator.Valid());
    let (first_key, first_value, first_distinct) = generator.Next(Vec::new(), Vec::new()).unwrap();
    assert_eq!(first_key[0], 1);
    assert_eq!(first_value[2], 1);
    assert!(!first_distinct);
    assert!(generator.Valid());

    let (second_key, second_value, _) = generator.Next(Vec::new(), Vec::new()).unwrap();
    assert_eq!(second_key[0], 3);
    assert_eq!(second_value[2], 3);
    assert!(!generator.Valid());
}

/// 键或值生成失败后不得推进迭代状态，以便调用方重试。
#[test]
fn generator_does_not_advance_after_key_or_value_failure() {
    let key_failing = RecordingIndex::default();
    key_failing.fail_key.set(true);
    let mut generator = NewPlainIndexKVGenerator(
        &key_failing,
        context(),
        UTC,
        Box::new(IntHandle(1)),
        Vec::new(),
        vec![Datum::default()],
    );
    assert!(generator.Next(Vec::new(), Vec::new()).is_err());
    assert!(generator.Valid());
    assert_eq!(key_failing.value_calls.get(), 0);

    let value_failing = RecordingIndex::default();
    value_failing.fail_value.set(true);
    let mut generator = NewPlainIndexKVGenerator(
        &value_failing,
        context(),
        UTC,
        Box::new(IntHandle(1)),
        Vec::new(),
        vec![Datum::default()],
    );
    assert!(generator.Next(Vec::new(), Vec::new()).is_err());
    assert!(generator.Valid());
    assert_eq!(value_failing.key_calls.get(), 1);
    assert_eq!(value_failing.value_calls.get(), 1);
}
