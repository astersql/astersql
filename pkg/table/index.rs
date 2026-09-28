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

// Index contracts and the stateful index-KV generator.
//
// The transaction, handle, datum, metadata, error-context and chunk types are
// the formal repository crates. Rust borrows Go interface arguments that are
// only observed, and owns the values retained by `IndexKVGenerator`.
//
// 索引契约与有状态的索引 KV 键值生成器。
// 事务、行句柄（handle）、Datum、元数据、错误上下文与 chunk 来自正式仓库 crate；
// 对仅观察的 Go 接口参数采用借用，由 `IndexKVGenerator` 持有需保留的值。

use chrono_tz::Tz;
use chunk_dependency::Row;
use errctx_dependency::errctx::Context as ErrorContext;
use kv_dependency::{Handle, Transaction};
use model_dependency::{IndexInfo, TableInfo};
use types_dependency::datum::Datum;

/// 索引操作结果别名。
pub type IndexResult<T> = Result<T, errors_dependency::SharedError>;

/// Iterator over index data in the KV store.
/// 遍历 KV 中索引数据的迭代器。
pub trait IndexIterator {
    /// 取下一条索引列值与行句柄。
    fn Next(&mut self) -> IndexResult<(Vec<Datum>, Box<dyn Handle>)>;
    /// 关闭迭代器并释放资源。
    fn Close(&mut self);
}

/// Duplicate-key lookup policy shared by record and index mutation options.
/// 记录与索引变更选项共用的重复键（duplicate key）检查策略。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[repr(u8)]
pub enum DupKeyCheckMode {
    /// 就地立即检查。
    #[default]
    DupKeyCheckInPlace = 0,
    /// 惰性检查（延后到合适阶段）。
    DupKeyCheckLazy = 1,
    /// 跳过重复键检查。
    DupKeyCheckSkip = 2,
}

/// Store lookup timing for pessimistic lazy duplicate-key checks.
/// 悲观事务下惰性重复键检查的存储查找时机。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[repr(u8)]
pub enum PessimisticLazyDupKeyCheckMode {
    /// 在加锁阶段检查。
    #[default]
    DupKeyCheckInAcquireLock = 0,
    /// 在预写（prewrite）阶段检查。
    DupKeyCheckInPrewrite = 1,
}

/// Common state embedded by Go's add/update/create mutation options.
/// Go 增删改变更选项嵌入的公共状态。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct commonMutateOpt {
    /// 可选 KV 上下文。
    pub(crate) ctx: Option<kv_dependency::Context>,
    /// 重复键检查模式。
    pub(crate) dupKeyCheck: DupKeyCheckMode,
    /// 悲观惰性重复键检查时机。
    pub(crate) pessimisticLazyDupKeyCheck: PessimisticLazyDupKeyCheckMode,
}

impl commonMutateOpt {
    /// 返回可选 KV 上下文。
    pub fn Ctx(&self) -> Option<kv_dependency::Context> {
        self.ctx.clone()
    }

    /// 返回重复键检查模式。
    pub fn DupKeyCheck(&self) -> DupKeyCheckMode {
        self.dupKeyCheck
    }

    /// 返回悲观惰性重复键检查时机。
    pub fn PessimisticLazyDupKeyCheck(&self) -> PessimisticLazyDupKeyCheckMode {
        self.pessimisticLazyDupKeyCheck
    }
}

/// Options used when creating an index entry.
///
/// The common mutation options live in `table.rs`; this compilation unit owns
/// the two flags introduced by `index.go`. Common options implement the same
/// `CreateIdxOption` hook when the table interface unit is assembled.
/// 创建索引项时的选项；公共变更选项在 `table.rs`，本单元拥有 `index.go` 引入的两标志。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CreateIdxOpt {
    /// 公共变更选项。
    pub(crate) commonMutateOpt: commonMutateOpt,
    /// 是否忽略事务断言。
    ignoreAssertion: bool,
    /// 是否来自 DDL backfill 回填任务。
    fromBackFill: bool,
}

/// Applies Go-style variadic create-index options in order to one option value.
/// 按 Go 可变参数风格依次应用创建索引选项。
pub fn NewCreateIdxOpt(options: &[&dyn CreateIdxOption]) -> CreateIdxOpt {
    let mut option = CreateIdxOpt::default();
    for apply in options {
        apply.applyCreateIdxOpt(&mut option);
    }
    option
}

impl CreateIdxOpt {
    /// 由公共变更选项构造，其余标志取默认。
    pub(crate) fn from_common(common: commonMutateOpt) -> Self {
        Self {
            commonMutateOpt: common,
            ..Self::default()
        }
    }

    /// 可变借用公共变更选项。
    pub(crate) fn common_mutate_opt_mut(&mut self) -> &mut commonMutateOpt {
        &mut self.commonMutateOpt
    }

    /// 返回可选 KV 上下文。
    pub fn Ctx(&self) -> Option<kv_dependency::Context> {
        self.commonMutateOpt.Ctx()
    }

    /// 返回重复键检查模式。
    pub fn DupKeyCheck(&self) -> DupKeyCheckMode {
        self.commonMutateOpt.DupKeyCheck()
    }

    /// 返回悲观惰性重复键检查时机。
    pub fn PessimisticLazyDupKeyCheck(&self) -> PessimisticLazyDupKeyCheckMode {
        self.commonMutateOpt.PessimisticLazyDupKeyCheck()
    }

    /// Whether transaction assertions should be skipped.
    /// 是否跳过事务断言。
    pub fn IgnoreAssertion(&self) -> bool {
        self.ignoreAssertion
    }

    /// Whether the entry is created by a DDL backfill worker.
    /// 条目是否由 DDL 回填 worker 创建。
    pub fn FromBackFill(&self) -> bool {
        self.fromBackFill
    }
}

/// Option accepted by `Index::Create`.
/// `Index::Create` 接受的选项钩子。
pub trait CreateIdxOption {
    /// 将本选项应用到可变的 `CreateIdxOpt`。
    fn applyCreateIdxOpt(&self, option: &mut CreateIdxOpt);
}

/// 标记「忽略断言」选项的零大小类型。
#[derive(Clone, Copy, Debug, Default)]
pub struct withIgnoreAssertion;

impl CreateIdxOption for withIgnoreAssertion {
    fn applyCreateIdxOpt(&self, option: &mut CreateIdxOpt) {
        option.ignoreAssertion = true;
    }
}

/// Indicates that transaction assertions may be ignored.
/// 表示可忽略事务断言。
pub static WithIgnoreAssertion: withIgnoreAssertion = withIgnoreAssertion;

/// 标记「来自 backfill」选项的零大小类型。
#[derive(Clone, Copy, Debug, Default)]
pub struct fromBackfill;

impl CreateIdxOption for fromBackfill {
    fn applyCreateIdxOpt(&self, option: &mut CreateIdxOpt) {
        option.fromBackFill = true;
    }
}

/// Indicates that the entry comes from a DDL backfill worker.
///
/// During backfill-merge, DML entries may be redirected to the temporary
/// index, while entries produced by the backfill worker must not be redirected.
/// 表示条目来自 DDL 回填 worker；回填合并时 DML 可改写到临时索引，回填产物不可改写。
pub static FromBackfill: fromBackfill = fromBackfill;

impl CreateIdxOption for DupKeyCheckMode {
    fn applyCreateIdxOpt(&self, option: &mut CreateIdxOpt) {
        option.commonMutateOpt.dupKeyCheck = *self;
    }
}

impl CreateIdxOption for PessimisticLazyDupKeyCheckMode {
    fn applyCreateIdxOpt(&self, option: &mut CreateIdxOpt) {
        option.commonMutateOpt.pessimisticLazyDupKeyCheck = *self;
    }
}

/// Object-safe view of the formal table mutation context used by index code.
///
/// The blanket implementation delegates to `tblctx::MutateContext`; no state
/// or behavior is replaced. This only erases associated return types that the
/// index implementation does not retain.
/// 索引代码使用的表变更上下文的对象安全视图；blanket 实现委托给 `MutateContext`。
pub trait IndexMutateContext {
    /// 取得表达式求值上下文。
    fn GetExprCtx(&self) -> &dyn tblctx_dependency::exprctx::ExprContext;
    /// 当前连接 ID。
    fn ConnectionID(&self) -> u64;
    /// 可变借用变更缓冲。
    fn GetMutateBuffers(&mut self) -> &mut tblctx_dependency::MutateBuffers;
}

impl<T> IndexMutateContext for T
where
    T: tblctx_dependency::MutateContext,
    T::ExprContext: Sized,
{
    fn GetExprCtx(&self) -> &dyn tblctx_dependency::exprctx::ExprContext {
        tblctx_dependency::MutateContext::GetExprCtx(self)
    }

    fn ConnectionID(&self) -> u64 {
        tblctx_dependency::MutateContext::ConnectionID(self)
    }

    fn GetMutateBuffers(&mut self) -> &mut tblctx_dependency::MutateBuffers {
        tblctx_dependency::MutateContext::GetMutateBuffers(self)
    }
}

/// Index data operations over the formal KV and metadata APIs.
///
/// `Create` and `Delete` remain generic over the formal mutation context. This
/// preserves the associated types exposed by `tblctx::MutateContext` instead
/// of erasing them into an opaque object.
/// 基于正式 KV 与元数据 API 的索引数据操作。
pub trait Index {
    /// Returns index metadata.
    /// 返回索引元数据。
    fn Meta(&self) -> &IndexInfo;

    /// Returns table metadata.
    /// 返回表元数据。
    fn TableMeta(&self) -> &TableInfo;

    /// Reports whether a datum row satisfies the partial-index predicate.
    /// 判断 Datum 行是否满足部分索引（partial index）谓词。
    fn MeetPartialCondition(&self, row: &[Datum]) -> IndexResult<bool>;

    /// Reports whether a chunk row satisfies the partial-index predicate.
    /// 判断 chunk 行是否满足部分索引谓词。
    fn MeetPartialConditionWithChunk(&self, row: Row) -> IndexResult<bool>;

    /// Inserts one index entry. The caller checks the partial-index predicate.
    /// 插入一条索引项；调用方负责检查部分索引谓词。
    fn Create(
        &self,
        ctx: &mut dyn IndexMutateContext,
        txn: &mut dyn Transaction,
        indexed_values: &[Datum],
        handle: &dyn Handle,
        handle_restore_data: &[Datum],
        options: &[&dyn CreateIdxOption],
    ) -> IndexResult<Box<dyn Handle>>;

    /// Deletes one index entry. The caller checks the partial-index predicate.
    /// 删除一条索引项；调用方负责检查部分索引谓词。
    fn Delete(
        &self,
        ctx: &mut dyn IndexMutateContext,
        txn: &mut dyn Transaction,
        indexed_values: &[Datum],
        handle: &dyn Handle,
    ) -> IndexResult<()>;

    /// Builds the generator used by this index, including multi-valued indexes.
    /// 构造本索引的 KV 生成器（含多值索引）。
    fn GenIndexKVIter<'index>(
        &'index self,
        error_context: ErrorContext,
        location: Tz,
        indexed_values: Vec<Datum>,
        handle: Box<dyn Handle>,
        handle_restore_data: Vec<Datum>,
    ) -> IndexKVGenerator<'index, Self>
    where
        Self: Sized;

    /// Checks whether an index entry exists and returns its stored handle.
    /// 检查索引项是否存在并返回已存行句柄。
    fn Exist(
        &self,
        error_context: &ErrorContext,
        location: Tz,
        txn: &dyn Transaction,
        indexed_values: &[Datum],
        handle: &dyn Handle,
    ) -> IndexResult<(bool, Option<Box<dyn Handle>>)>;

    /// Generates an index key. Multi-valued indexes use `GenIndexKVIter`.
    /// 生成索引键；多值索引应走 `GenIndexKVIter`。
    fn GenIndexKey(
        &self,
        error_context: &ErrorContext,
        location: Tz,
        indexed_values: &[Datum],
        handle: &dyn Handle,
        buffer: Vec<u8>,
    ) -> IndexResult<(Vec<u8>, bool)>;

    /// Generates an index value.
    /// 生成索引值。
    fn GenIndexValue(
        &self,
        error_context: &ErrorContext,
        location: Tz,
        distinct: bool,
        untouched: bool,
        indexed_values: &[Datum],
        handle: &dyn Handle,
        restored_data: &[Datum],
        buffer: Vec<u8>,
    ) -> IndexResult<Vec<u8>>;

    /// Fetches index-column values, reusing the supplied output allocation.
    /// 取出索引列值，复用调用方提供的输出缓冲。
    fn FetchValues(&self, row: &[Datum], columns: Vec<Datum>) -> IndexResult<Vec<Datum>>;
}

/// Stateful key/value generator for plain and multi-valued indexes.
/// 普通与多值索引的有状态键值生成器。
pub struct IndexKVGenerator<'index, I: Index + ?Sized> {
    /// 所属索引实现。
    index: &'index I,
    /// 错误上下文。
    error_context: ErrorContext,
    /// 时区，用于时间类型编码。
    location: Tz,
    /// 行句柄。
    handle: Box<dyn Handle>,
    /// 用于重建 handle 的列数据。
    handle_restore_data: Vec<Datum>,

    /// 是否为多值索引生成路径。
    is_multi_value: bool,
    /// 多值索引的全部索引值组。
    all_index_values: Vec<Vec<Datum>>,
    /// 当前游标。
    cursor: usize,
    /// 普通索引的单组索引值。
    index_values: Vec<Datum>,
}

/// Creates a generator for a multi-valued index.
/// 为多值索引创建生成器。
pub fn NewMultiValueIndexKVGenerator<'index, I: Index + ?Sized>(
    index: &'index I,
    error_context: ErrorContext,
    location: Tz,
    handle: Box<dyn Handle>,
    handle_restore_data: Vec<Datum>,
    multi_value_index_data: Vec<Vec<Datum>>,
) -> IndexKVGenerator<'index, I> {
    IndexKVGenerator {
        index,
        error_context,
        location,
        handle,
        handle_restore_data,
        is_multi_value: true,
        all_index_values: multi_value_index_data,
        cursor: 0,
        index_values: Vec::new(),
    }
}

/// Creates a generator for a non-multi-valued index.
/// 为非多值索引创建生成器。
pub fn NewPlainIndexKVGenerator<'index, I: Index + ?Sized>(
    index: &'index I,
    error_context: ErrorContext,
    location: Tz,
    handle: Box<dyn Handle>,
    handle_restore_data: Vec<Datum>,
    index_data: Vec<Datum>,
) -> IndexKVGenerator<'index, I> {
    IndexKVGenerator {
        index,
        error_context,
        location,
        handle,
        handle_restore_data,
        is_multi_value: false,
        all_index_values: Vec::new(),
        cursor: 0,
        index_values: index_data,
    }
}

impl<I: Index + ?Sized> IndexKVGenerator<'_, I> {
    /// Returns the next key/value pair.
    ///
    /// Just like Go, the cursor advances only after both generators succeed.
    /// Calling `Next` after a multi-value generator is exhausted panics on the
    /// out-of-range access, so callers must observe `Valid`.
    /// 返回下一对键值；仅在键与值都生成成功后推进游标（对齐 Go）。
    pub fn Next(
        &mut self,
        key_buffer: Vec<u8>,
        value_buffer: Vec<u8>,
    ) -> IndexResult<(Vec<u8>, Vec<u8>, bool)> {
        let indexed_values = if self.is_multi_value {
            &self.all_index_values[self.cursor]
        } else {
            &self.index_values
        };
        let (key, distinct) = self.index.GenIndexKey(
            &self.error_context,
            self.location,
            indexed_values,
            self.handle.as_ref(),
            key_buffer,
        )?;
        let value = self.index.GenIndexValue(
            &self.error_context,
            self.location,
            distinct,
            false,
            indexed_values,
            self.handle.as_ref(),
            &self.handle_restore_data,
            value_buffer,
        )?;
        // 键值均成功后才推进，失败时可重试当前项。
        self.cursor += 1;
        Ok((key, value, distinct))
    }

    /// Reports whether another generated pair is available.
    /// 是否还有下一对待生成键值对。
    pub fn Valid(&self) -> bool {
        if self.is_multi_value {
            self.cursor < self.all_index_values.len()
        } else {
            self.cursor == 0
        }
    }
}
