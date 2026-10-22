# `br/pkg/utils/iter/source_types.rs`

## 文件定位

这些类型只负责“单次拉取如何改变本地状态”。

## 核心职责

文件把五类来源统一适配到 `iter.rs` 定义的 `TryNextor<T>` 拉取协议：

- `SliceIter<T>` 按原顺序消费一个拥有所有权的有限序列。
- `OfRangeIter<T>` 每次递增一，生成整数区间。
- `EmptyIter<T>` 稳定返回结束态，用于空输入和组合器清空状态。
- `FailureIter<T>` 稳定返回同一错误文本，用于显式失败源和错误传播。
- `FuncIter<T>` 把有状态 `FnMut` 闭包适配为惰性来源，由闭包决定 `Emit`、`Done` 或 `Throw`。

这些类型只负责“单次拉取如何改变本地状态”。结果三态、上下文模型与收集行为属于 `iter.rs`：`Emit` 携带元素，`Done` 表示正常耗尽，`Throw` 携带字符串错误。

## 主要符号

- `SliceIter<T> { items: VecDeque<T> }`：公开类型、私有存储。`TryNext` 调用 `pop_front`，因此元素移动出队而非克隆；`new_slice(Vec<T>)` 是 crate 内工厂。
- `OfRangeIter<T> { pub end, pub endExclusive, pub current }`：公开且字段可见的区间状态。`impl_range!` 在本文件为 `i32`、`i64`、`u32`、`u64`、`usize`、`isize` 生成 `TryNextor` 实现；`source.rs` 用相同状态机补充 `i8`、`i16`、`u8`、`u16`。`new_range` 默认设置 `endExclusive = true`。
- `EmptyIter<T>`：只持有 `PhantomData<T>`，不分配元素；`empty<T>()` 返回装箱后的 trait object。
- `FailureIter<T>`：保存私有 `String` 与 `PhantomData<T>`；`new_failure` 仅 crate 内可见。
- `FuncIter<T>`：保存 `Box<dyn FnMut(&Context) -> IterResult<T> + Send>`；`new_func` 将闭包装箱，允许闭包跨调用保留可变状态。
- `impl_range!`：模块内宏，只生成整数类型实现，不作为 crate 公共 API。

所有 `TryNextor` 实现都要求 `T: Send`，工厂在需要装箱长期持有时进一步要求 `'static`。这是因为 `TryNextor<T>: Send`，且组合器可能把来源移动到工作线程。

## 执行流程

1. `FromSlice` 把 `Vec<T>` 交给 `new_slice`，后者一次性转换为 `VecDeque<T>`。每次拉取从队头 `pop_front`：有值则 `Emit(item)`，空队列则 `Done()`；耗尽后的后续拉取仍为 `Done()`。
2. `OfRange` 调用 `new_range(begin, end)` 建立 `current = begin` 的默认半开区间。每次拉取先判断 `current > end`，或 `current == end && endExclusive`；命中则结束，否则先保存当前值为 `Emit`，再把 `current` 加一。
3. `empty()` 创建无数据的 `EmptyIter`，任意次数拉取都直接 `Done()`。`FlatMap`/`ConcatAll` 用它初始化 `JoinIter.current`；`JoinIter` 遇到当前子流错误时还会把后续 `inner` 替换为空源，禁止错误之后继续拼接。
4. `Fail` 经 `new_failure` 保存错误字符串。每次拉取克隆该字符串并 `Throw`，不会自动转为 `Done`。
5. `Func` 经 `new_func` 保存调用者的 `FnMut`。每次拉取原样传入当前 `Context` 并直接返回闭包结果；例如 `parity_test.rs` 中的无状态闭包会重复产出，而捕获 `done` 的闭包能在一次产出后结束。

## 数据与状态

`SliceIter` 的唯一可变状态是剩余队列，空间复杂度为输入元素总量，单次队头移除为 `VecDeque::pop_front` 的常数时间；构造时会从 `Vec` 转换容器。`OfRangeIter` 保存上界、边界模式和下一候选值，不预先分配区间元素，空间为常数。`EmptyIter` 与 `FailureIter` 的 `PhantomData<T>` 只表达泛型所有权/类型关系，不保存 `T` 值。

`FailureIter` 每次失败会克隆 `String`。`FuncIter` 的状态完全由闭包捕获环境决定；`FnMut` 表明每次拉取可以更新该环境，也意味着同一个实例必须通过 `&mut self` 串行调用。文件中没有全局变量、缓存或外部持久状态。

区间不变量是：若尚未结束，本次产出拉取前的 `current`，随后 `current += 1`。默认工厂建立半开区间 `[begin, end)`；若外部直接把公开字段 `endExclusive` 设为 `false`，则状态机变成闭区间 `[current, end]`。

## 依赖与调用关系

直接标准库依赖只有 `VecDeque`、`PhantomData` 和装箱闭包；crate 内依赖是 `iter.rs` 的 `Context`、`IterResult`、`TryNextor` 以及 `Done`/`Emit`/`Throw` 构造器。`Cargo.toml` 没有声明第三方依赖或 feature，因此本文件不跨越网络、存储或运行时边界。

已核对的直接上游为：

- `source.rs::FromSlice -> new_slice -> SliceIter::TryNext`；
- `source.rs::OfRange -> new_range -> OfRangeIter::TryNext`；
- `source.rs::Fail -> new_failure -> FailureIter::TryNext`；
- `source.rs::Func -> new_func -> FuncIter::TryNext`；
- `combinators.rs::{FlatMap, ConcatAll} -> empty`，以及 `combinator_types.rs::JoinIter::TryNext -> empty` 的错误后清空路径。

RustCodeGraph 将 `source.rs` 标为被恢复日志相关代码及测试等 7 个文件使用，将核心 `iter.rs` 标为被 60 个文件使用；这说明本文件通过公开工厂和通用 trait 间接进入 BR 流水线。对本文件内部工厂执行精确 `callers/callees` 查询时图未返回函数边，所以上述细粒度边以已索引的 `source.rs`/组合器源码为直接证据，而不是把空查询误解为无调用者。

## 错误处理与边界

- `SliceIter`、`EmptyIter` 与 `OfRangeIter` 自身不产生 `Throw`。空切片、`begin == end` 的默认半开区间以及 `begin > end` 都在首次拉取返回 `Done`。
- `FailureIter` 每次都返回 `Throw(error.clone())`，其中 `Finished == false`；调用者若在错误后继续拉取，会再次得到相同文本。
- `FuncIter` 不捕获 panic，也不改写闭包返回值；错误、结束、非法 `IterResult` 组合和取消检查均由闭包负责。
- 除 `FuncIter` 把 `Context` 传给闭包外，其余四种来源都忽略上下文，不会因取消自动停止；协作式取消必须由上层组合器或函数源实现。
- 区间在成功产出后执行普通整数加一。默认半开区间在 `end` 处先结束，通常不会递增上界；但若手工构造包含最大整数上界的闭区间，产出最大值后的加一在开启溢出检查时会 panic、在关闭检查时会按 Rust 编译配置回绕。这是公开字段允许的边界风险，新增闭区间 API 时必须专门处理。

## 并发与资源生命周期

本文件不创建线程、锁、通道、任务或事务。来源实例由拥有它的 `Box`/结构体控制生命周期，释放时队列元素、错误字符串或闭包捕获资源随之释放；没有显式 `close`。

`Send` 约束允许实例被移动到其他线程，但接口要求 `&mut self`，这些类型也没有提供共享并发拉取保证。尤其 `FuncIter` 保存的是 `FnMut + Send` 而非 `Sync`：安全用法是让单一所有者顺序调用，若需多个线程消费，应由上层并发组合器协调。`SliceIter` 和 `OfRangeIter` 的状态更新也依赖独占可变借用。

`empty()` 产生的新装箱对象是独立的无状态哨兵。它在 `JoinIter` 中既避免 `Option` 分支，也确保子流失败后后续拉取不会重新访问被屏蔽的来源。

## 扩展指南

- 新增来源类型时，在本文件实现独立结构体及 `TryNextor` 状态机，把公开装箱工厂放在 `source.rs`；保持实现与测试分文件，不要把单元测试内嵌进本文件。
- 修改区间语义时必须同步检查 `impl_range!` 和 `source.rs::impl_missing_go_integer_range!`，避免不同整数宽度漂移；若开放闭区间构造，应为最大值溢出设计明确结束策略。
- 修改空源行为时同步审查 `JoinIter::TryNext`、`FlatMap` 与 `ConcatAll`，因为空源既是初态也是错误后的终止哨兵。
- 修改错误表示时同步检查 `iter.rs::{IterError, Throw, CollectAll}` 和组合器错误传播；从 Go `error` 到 Rust `String` 的兼容限制不能悄然改变。
- 修改 `FuncIter` 时保留 `FnMut` 的状态能力、`Context` 透传和 `Send` 约束；若引入共享并发，需另行定义同步契约，不能仅添加 `Sync` 约束。
- 测试应扩展同目录独立文件：基础公开契约放在 `parity_test.rs`，整数宽度放在 `source_test.rs`，组合链和失败传播放在 `combinator_test.rs`；Go 语义变化还应同步相应 `*.go` 测试。

兼容性风险集中在结果三态、错误文本、区间边界和错误后是否继续产出；性能风险主要是切片构造时的容器转换、失败字符串的逐次克隆，以及函数源闭包内由调用者引入的阻塞或分配。
