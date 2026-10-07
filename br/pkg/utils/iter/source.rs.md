# `br/pkg/utils/iter/source.rs`

源码：[source.rs](source.rs)；底层状态类型：[source_types.rs](source_types.rs)；crate 入口：[lib.rs](lib.rs)。

## 文件定位

`source.rs` 位于 `astersql-br-pkg-utils-iter` crate 的“数据源公开工厂”层。`lib.rs` 先声明 `iter` 核心协议、再声明 `source_types` 和本模块，并通过 `pub use source::*` 将这里的 API 扁平导出。因此调用方通常直接从 crate 根引入 `FromSlice`、`OfRange`、`Fail`、`Func`，而不直接依赖底层构造函数 `new_slice`、`new_range`、`new_failure`、`new_func`。

该文件不负责消费、组合或调度迭代器；它把调用方给出的值、区间、错误或闭包装箱为统一的 `Box<dyn TryNextor<T>>`。真正保存状态并实现 `TryNext` 的类型位于 `source_types.rs`。例如，`combinators.rs::ConcatAll` 用 `FromSlice` 把一组迭代器变成外层迭代源；`br/pkg/restore/log_client/log_file_manager.rs` 则用 `FromSlice` 返回元数据流、用 `Fail` 把存储或反序列化错误变成流内错误。

## 核心职责

- 提供四个公开构造入口，使不同来源都服从 `iter.rs::TryNextor<T>` 的三态拉取协议：`Emit(T)`、`Done()`、`Throw(String)`。
- 隐藏 `SliceIter`、`OfRangeIter`、`FailureIter`、`FuncIter` 的构造细节，并用 trait object 统一返回类型，便于 `Map`、`FlatMap`、`Transform`、`ConcatAll` 等组合器接线。
- 补齐 Go `constraints.Integer` 覆盖而 `source_types.rs` 原有宏未覆盖的 `i8`、`i16`、`u8`、`u16` 区间实现。
- 以 `SliceSource<T>` 暴露 `SliceIter<T>` 的类型别名，供需要具体类型而非动态分发的测试或局部构造使用；它没有新增状态或行为。

本模块是薄工厂层，但并非占位或桩：生产路径 `log_file_manager.rs::{LoadLogFileSubcompactions, LoadMigrations}`、`migration.rs` 和 `restore/restorer.rs::PipelineFromSlice` 都直接使用它。

## 主要符号

- `impl_missing_go_integer_range!`：私有宏，为 `OfRangeIter<i8/i16/u8/u16>` 实现 `TryNextor`。每次先检查是否越过 `end`，或是否已到达排他上界；未结束时发出 `current`，再执行加一。其状态机与 `source_types.rs::impl_range!` 相同。
- `FromSlice<T: Send + 'static>(Vec<T>) -> Box<dyn TryNextor<T>>`：取得整个 `Vec<T>` 的所有权，调用 `new_slice`，最终由 `SliceIter` 内的 `VecDeque<T>` 按 FIFO 顺序逐个 `pop_front`。
- `OfRange<T>(begin, end) -> Box<dyn TryNextor<T>>`：要求 `OfRangeIter<T>: TryNextor<T>`，通过 `new_range` 创建默认 `endExclusive = true` 的半开区间 `[begin, end)`。当前公开可用整数由 `source_types.rs` 的 `i32/i64/u32/u64/usize/isize` 加上本文件的 `i8/i16/u8/u16` 组成。
- `Fail<T: Send + 'static>(err: impl Into<String>) -> Box<dyn TryNextor<T>>`：构造持有固定错误字符串的失败源。每次拉取都会返回 `Throw(error.clone())`，不会自动转成 `Done`。
- `Func<T: Send + 'static>(g) -> Box<dyn TryNextor<T>>`：保存一个 `FnMut(&Context) -> IterResult<T>`。每次拉取原样调用闭包，产出、结束、错误以及是否观察取消都由闭包决定。
- `SliceSource<T> = SliceIter<T>`：公开类型别名，只改变可引用名称，不改变布局、所有权或调度语义。

所有公开工厂返回的 trait object 都要求元素或闭包可 `Send` 且为 `'static`，以满足 `TryNextor<T>: Send` 以及组合器把迭代器移入工作线程或长期保存的需要。

## 执行流程

1. 调用方选择来源工厂；工厂调用 `source_types.rs` 中对应的 `new_*`，把具体实现装入 `Box<dyn TryNextor<T>>`。
2. 下游组合器或消费者持有这个可变 trait object，并以 `&mut self` 调用 `TryNext(&Context)`。
3. `FromSlice` 路径从队首移出一个值并返回 `Emit`；队列为空后，每次都返回 `Done`。
4. `OfRange` 路径先判断边界。`begin >= end` 时公开半开区间立即结束；否则按整数递增顺序返回 `begin` 到 `end - 1`。终止判断发生在递增前，公开构造不会发出 `end`。
5. `Fail` 路径不发出元素，也不设置 `Finished`，而是每次克隆固定字符串并返回 `Throw`。由下游决定错误后是否停止继续拉取。
6. `Func` 路径把当前 `Context` 原样交给闭包。无状态且始终 `Emit` 的闭包会无限产出；需要有限流时，闭包必须自行保存状态并在适当时返回 `Done`。

典型生产链是 `LoadLogFileSubcompactions` 先同步读取并解析所有元数据，再根据结果返回 `FromSlice(subs)`；任一步失败则返回 `Fail(...)`。下游因此可以用同一个 `TryNextor<LogFileSubcompaction>` 接口处理成功序列和初始化错误。

## 数据与状态

- 切片源的唯一可变状态是 `SliceIter.items: VecDeque<T>`。元素所有权随 `pop_front` 转移给调用方；已发出的元素不会保留，空间可随队列消费而释放。
- 区间源保存 `current`、`end` 和 `endExclusive`。`OfRange` 总是将后者设为 `true`；每次成功发出后只修改 `current`。
- 失败源保存一个 `String` 和 `PhantomData<T>`。错误字符串在每次 `TryNext` 时克隆，因此重复拉取可重复观察同一消息。
- 闭包源保存装箱的 `FnMut`。捕获状态的所有权属于 `FuncIter`，其生命周期与迭代器一致；状态变化完全由闭包定义。
- `SliceSource` 不创建额外状态，只是 `SliceIter` 的别名。

`IterResult<T>` 的三个字段不是由 Rust 枚举强制互斥，但本文件及底层状态机只通过 `Emit`、`Done`、`Throw` 构造合法三态。扩展实现应维持这一不变量，避免同时携带元素、错误或结束标志。

## 依赖与调用关系

向下依赖分两层：`crate::iter` 提供 `Context`、`TryNextor`、`IterResult` 和 `Done`/`Emit` 构造函数；`crate::source_types` 提供具体状态类型与 crate 内部 `new_*` 工厂。`Cargo.toml` 将本目录定义为独立库 crate `astersql-br-pkg-utils-iter`，`[lib] path = "lib.rs"`，未声明额外第三方依赖；其 porting 元数据明确对应 Go 包 `br/pkg/utils/iter`。

向上调用可分为三类：

- crate 内组合：`combinators.rs::ConcatAll` 用 `FromSlice(items)` 驱动多个子迭代器的顺序拼接。
- BR 生产流程：`log_file_manager.rs` 用 `FromSlice` 包装文件组、文件、迁移与反序列化结果，用 `Fail` 表示目录遍历、文件读取和 JSON 解码失败；`migration.rs` 用 `FromSlice` 接入映射链；`restore/restorer.rs::PipelineFromSlice` 为恢复流水线提供切片入口。
- 契约测试：`parity_test.rs` 覆盖四个工厂；`source_test.rs` 专门覆盖补齐的四种固定位整数；`as_seq_test.rs` 和 `transform_backpressure_test.rs` 用 `Func` 构造可控的多步、错误及背压来源；`combinator_test.rs` 大量用 `OfRange` 与 `Fail` 验证组合传播。

RustCodeGraph 对 `source.rs` 的文件关系报告 7 个直接使用文件，包括恢复日志客户端测试、`log_file_manager.rs`、`migration.rs` 和迁移测试；泛型工厂的精确 `callers/callees` 查询未生成静态边，因此上述具体引用以仓库符号搜索和索引文件内容交叉确认。

## 错误处理与边界

- `FromSlice(vec![])` 首次拉取即 `Done`，耗尽后继续拉取仍为 `Done`；`parity_test.rs::go_rust_public_contract_matches` 覆盖空源和正常耗尽。
- `OfRange(begin, end)` 是半开区间。`begin == end` 或 `begin > end` 时不产出元素。公开构造在发出 `end - 1` 后递增到 `end`，下一次拉取结束。
- 本文件的整数状态机使用普通 `+= 1`。公开半开区间不会发出并递增排他上界，因此正常的 `OfRange` 在 `end` 为类型最大值时也不会从最大值继续加一；但调用方若绕过工厂，直接构造公开字段的 `OfRangeIter` 并把 `endExclusive` 改为 `false`，在发出整数最大值后递增可能溢出。这是底层具体类型的扩展边界。
- `Fail` 接受任意 `Into<String>`，因此 Rust 侧只保留错误文本，不保留 Go `error` 的具体类型、包装链或可供 `errors.Is/As` 判断的身份。生产调用方通常显式 `err.to_string()`。
- `Fail` 的结果是错误而非结束；若消费者在错误后仍继续调用，会再次得到同一错误。组合器应按 `FinishedOrError` 约定停止或传播。
- `Func` 不替调用方检查 `Context::Done()`，也不验证闭包返回的 `IterResult` 是否为合法三态。需要取消响应、有限性或错误一次性语义时必须由闭包实现并由独立测试固定。
- 动态装箱和 `'static` 约束意味着这些工厂不能直接保存借用栈上数据的元素或闭包；应转移所有权，或在调用方使用拥有型共享句柄。

## 并发与资源生命周期

`TryNextor<T>: Send` 允许把整个源移动到另一个线程，但 `TryNext` 需要 `&mut self`，本模块不提供共享并发拉取能力，也没有锁。通常由一个消费者串行拉取；若多个任务需要共享，必须由更上层提供互斥和明确的顺序语义。

`FromSlice` 在构造时接管 `Vec`，随后由 `VecDeque` 管理剩余元素；迭代器释放时，未消费元素随队列一起析构。`Fail` 只拥有错误字符串。`Func` 拥有闭包及其捕获资源，释放迭代器即释放这些资源，但本模块没有显式关闭、回滚或取消回调；捕获文件、通道发送端等资源时，应让闭包状态或外层包装负责清理。

只有 `Func` 把 `Context` 交给用户逻辑；切片、区间和失败源均忽略上下文。这三类源单步工作有界且不阻塞，但不会因上下文已取消而改变返回值。取消敏感的流水线应在组合层停止拉取，或使用检查 `ctx.Done()` 的 `Func`。

## 与 Go 版本的对应关系

`source.go` 的四个公开函数与本文件一一对应：Go `FromSlice` 包装 `fromSlice`，`OfRange` 创建排他上界的 `ofRange`，`Fail` 包装固定 `error`，`Func` 包装函数；`source_types.go` 则给出各自的 `TryNext` 状态机。Rust 版本保持了 FIFO、半开区间、重复失败以及闭包逐次调用的核心行为。

主要语言适配差异如下：

- Go `constraints.Integer` 覆盖所有内建整数；Rust 通过两处宏的显式实现列表覆盖对应的有符号、无符号和指针宽度整数。本文件专门补上 `i8/i16/u8/u16`，`source_test.rs::of_range_supports_all_go_fixed_width_integer_types` 固定了这部分兼容性。
- Go `FromSlice` 保存切片描述符并逐步重切片；Rust 接管 `Vec<T>` 并转换为 `VecDeque<T>`。两者都保持顺序且单步移除队首，但 Rust API 明确转移所有权。
- Go 用 `error` 接口，可保留错误身份和包装；Rust 当前 `IterError = String`，只保留显示文本。
- Go 接受 `context.Context`；Rust 使用本 crate 自定义的 `Context`。源层同样只有函数型来源能自行观察上下文，其他三个状态机忽略它。
- Rust 为跨线程组合增加 `Send + 'static` 约束并返回 `Box<dyn TryNextor<T>>`；Go 接口值不表达这些编译期约束。

Go `combinator_test.go` 使用 `OfRange` 和 `Fail` 验证组合器的顺序与错误传播；Rust 的 `combinator_test.rs` 保留同类场景，另由 `parity_test.rs` 直接核对公开 source 契约。

## 扩展指南

- 新增一种来源时，把可变状态和 `TryNextor` 实现放在独立的生产文件（通常是 `source_types.rs` 或新的 source 类型文件），在本文件只增加薄工厂；测试继续放在独立 `*_test.rs`，不要内嵌进生产源文件。
- 扩展 `OfRange` 支持的数值类型时，必须保持与 Go `constraints.Integer` 的真实范围一致，同时验证空区间、单元素跨度、类型上下界及递增溢出。若想支持闭区间，应新增清晰 API 或安全的步进策略，不要静默改变 `OfRange` 的半开语义。
- 若要保留结构化错误，需要先评估 `iter.rs::IterError`、`IterResult`、所有组合器和 Go 对拍测试；仅修改 `Fail` 返回类型会破坏统一错误协议。
- 若来源需要取消或阻塞 I/O，优先让其显式检查 `Context` 并记录资源关闭契约；不要假设本工厂层会自动处理取消。
- 若需要多消费者并发拉取，应在上层设计同步包装并测试顺序、终止和错误可见性，不应给现有状态类型随意加内部锁而改变单消费者成本。
- 修改公开工厂后至少同步 `source_test.rs` 与 `parity_test.rs`；涉及 `Func` 的逐次或背压行为时同步 `as_seq_test.rs`、`transform_backpressure_test.rs`；涉及组合错误传播时同步 `combinator_test.rs`，并与 `combinator_test.go` 的原始意图对照。

## 验证依据

- RustCodeGraph 索引状态：项目索引包含 7,032 个 Rust 文件；`files --filter br/pkg/utils/iter` 确认本 crate 的 Rust/Go 对照文件与独立测试均已入图。
- RustCodeGraph 文件读取：`source.rs`（8 个符号、61 行）、`source_types.rs`、`iter.rs`、`lib.rs`、`source_test.rs`、`parity_test.rs`、`combinators.rs`、`log_file_manager.rs`、`restore/restorer.rs`。
- RustCodeGraph 符号查询：`FromSlice`、`OfRange`、`Fail`、`Func`、`SliceSource`；精确 `callers/callees` 对泛型工厂无输出，因此没有据此虚构调用边。
- 配置证据：`br/pkg/utils/iter/Cargo.toml` 的 crate 名、库入口和 Go porting 元数据。
- Go 对照：`br/pkg/utils/iter/source.go`、`source_types.go`、`iter.go`、`combinator_test.go`。
- Rust 测试：`source_test.rs` 验证四种补充整数；`parity_test.rs::go_rust_public_contract_matches` 验证切片顺序与耗尽、半开区间、失败三态和有/无状态闭包；`as_seq_test.rs`、`transform_backpressure_test.rs`、`combinator_test.rs` 提供函数源及组合传播证据。
- 直接引用搜索确认生产调用位于 `br/pkg/restore/log_client/log_file_manager.rs`、`migration.rs`、`stubs.rs` 和 `br/pkg/restore/restorer.rs`。本任务为纯文档分析，按计划不运行 Cargo。
