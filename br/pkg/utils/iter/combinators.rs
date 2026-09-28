// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc. Licensed under Apache-2.0.

//! 迭代器组合器工厂，对应 Go `br/pkg/utils/iter` 的公开组合 API。
//!
//! 返回值均为 `Box<dyn TryNextor<_>>`，便于链式组合；Transform 支持并发与缓冲选项。
//! 配置约束：concurrency > bufferSize 时抬高 buffer，避免池比缓冲更大导致空转。
//! 所有工厂函数消费上游所有权（Box），链式调用时注意移动语义。
//! TransformConfig 为可变闭包，允许在 Vec 中依次覆盖同名字段。

use crate::combinator_types::{
    BufferedMappingCfg, FilterIter, FilterMapIter, JoinIter, PureMapIter, TakeIter, TransformIter,
    TryMapIter, WithIndexIter, WorkerPool,
};
use crate::iter::{CollectAll, Context, Indexed, IterResult, TryNextor};
use crate::source::FromSlice;
use crate::source_types::empty;

/// Transform 配置闭包类型：就地修改 BufferedMappingCfg。
/// 使用 Box<dyn FnMut> 以便在 Vec 中异构存储多个选项。
pub type TransformConfig = Box<dyn FnMut(&mut BufferedMappingCfg) + Send>;

/// 设置并发度（WorkerPool 上限）。
/// 池名固定为 transforming，便于与默认池区分。
pub fn WithConcurrency(n: u32) -> TransformConfig {
    Box::new(move |c: &mut BufferedMappingCfg| {
        c.quota = Some(WorkerPool::new(n, "transforming"));
    })
}

/// 设置结果缓冲/在飞上限。
/// 与 WithConcurrency 联用时可能被抬升到 concurrency。
pub fn WithBufferSize(n: u32) -> TransformConfig {
    Box::new(move |c: &mut BufferedMappingCfg| {
        c.bufferSize = n;
    })
}

/// 并发有副作用映射；默认 buffer=1，未指定 quota 时用 buffer 作并发上限。
/// mapper 签名带 Context，便于协作式取消。
pub fn Transform<T, R>(
    it: Box<dyn TryNextor<T>>,
    with: impl Fn(&Context, T) -> Result<R, String> + Send + Sync + 'static,
    mut cs: Vec<TransformConfig>,
) -> Box<dyn TryNextor<R>>
where
    T: Send + 'static,
    R: Send + 'static,
{
    let mut cfg = BufferedMappingCfg {
        bufferSize: 1,
        quota: None,
    };
    for c in &mut cs {
        c(&mut cfg);
    }
    if cfg.quota.is_none() {
        cfg.quota = Some(WorkerPool::new(cfg.bufferSize, "max-concurrency"));
    }
    // Go 侧同样保证 buffer 不小于 concurrency，避免调度饥饿。
    // 抬升只发生在最终构造 TransformIter 之前。
    if cfg.quota.as_ref().unwrap().Limit() > cfg.bufferSize as i32 {
        cfg.bufferSize = cfg.quota.as_ref().unwrap().Limit() as u32;
    }
    Box::new(TransformIter::new(it, with, cfg))
}

/// 丢弃使谓词为真的元素（与 Rust Iterator::filter 相反：谓词表示“滤掉”）。
/// 命名 FilterOut 强调“滤出/丢掉”，避免与 filter 保留语义混淆。
pub fn FilterOut<T: Send + 'static>(
    it: Box<dyn TryNextor<T>>,
    f: impl FnMut(&T) -> bool + Send + 'static,
) -> Box<dyn TryNextor<T>> {
    Box::new(FilterIter {
        inner: it,
        filterOutIf: Box::new(f),
    })
}

/// 只取前 n 个元素。
pub fn TakeFirst<T: Send + 'static>(inner: Box<dyn TryNextor<T>>, n: u32) -> Box<dyn TryNextor<T>> {
    Box::new(TakeIter { n, inner })
}

/// 一对多展平：每个上游元素映射为一个子迭代器再串联。
/// 内部先 Map 成迭代器流，再交 JoinIter 展开。
pub fn FlatMap<T, R>(
    it: Box<dyn TryNextor<T>>,
    mapper: impl FnMut(T) -> Box<dyn TryNextor<R>> + Send + 'static,
) -> Box<dyn TryNextor<R>>
where
    T: Send + 'static,
    R: Send + 'static,
{
    Box::new(JoinIter {
        inner: Map(it, mapper),
        current: empty(),
    })
}

/// 纯函数一对一映射。
pub fn Map<T, R>(
    it: Box<dyn TryNextor<T>>,
    mapper: impl FnMut(T) -> R + Send + 'static,
) -> Box<dyn TryNextor<R>>
where
    T: Send + 'static,
    R: Send + 'static,
{
    Box::new(PureMapIter {
        inner: it,
        mapper: Box::new(mapper),
    })
}

/// 映射并可跳过：`(R, true)` 表示丢弃该项。
pub fn MapFilter<T, R>(
    it: Box<dyn TryNextor<T>>,
    mapper: impl FnMut(T) -> (R, bool) + Send + 'static,
) -> Box<dyn TryNextor<R>>
where
    T: Send + 'static,
    R: Send + 'static,
{
    Box::new(FilterMapIter {
        inner: it,
        mapper: Box::new(mapper),
    })
}

/// Fallible 映射；Err 转为迭代器 Throw。
pub fn TryMap<T, R>(
    it: Box<dyn TryNextor<T>>,
    mapper: impl FnMut(T) -> Result<R, String> + Send + 'static,
) -> Box<dyn TryNextor<R>>
where
    T: Send + 'static,
    R: Send + 'static,
{
    Box::new(TryMapIter {
        inner: it,
        mapper: Box::new(mapper),
    })
}

/// 顺序拼接多个迭代器，等价于 Join(FromSlice(items), empty)。
/// 空 Vec 时立即结束（current 为 empty 且 inner 空）。
pub fn ConcatAll<T: Send + 'static>(items: Vec<Box<dyn TryNextor<T>>>) -> Box<dyn TryNextor<T>> {
    Box::new(JoinIter {
        inner: FromSlice(items),
        current: empty(),
    })
}

/// 为元素附加从 0 开始的序号。
pub fn Enumerate<T: Send + 'static>(it: Box<dyn TryNextor<T>>) -> Box<dyn TryNextor<Indexed<T>>> {
    Box::new(WithIndexIter {
        inner: it,
        index: 0,
    })
}

/// 收集至多 n 个元素；内部复用 TakeFirst + CollectAll。
/// 成功时返回的 IterResult.Finished 为 false（与 CollectAll 一致）。
pub fn CollectMany<T: Send + 'static>(
    ctx: &Context,
    it: Box<dyn TryNextor<T>>,
    n: u32,
) -> IterResult<Vec<T>> {
    let mut taken = TakeFirst(it, n);
    CollectAll(ctx, &mut *taken)
}
