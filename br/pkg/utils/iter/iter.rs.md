# `br/pkg/utils/iter/iter.rs`

## 文件定位

`iter.rs` 是 `astersql-br-pkg-utils-iter` crate 的核心协议层：它定义可取消上下文、单步迭代结果和 `TryNextor<T>` trait，并提供收集、标准 `Iterator` 适配与观测包装。`br/pkg/utils/iter/lib.rs` 以 `#[path = "iter.rs"] pub mod iter` 挂载它，再通过 `pub use iter::*` 将公开符号扁平导出。

`br/pkg/utils/iter/Cargo.toml` 将该 crate 定义为不发布的 library，移植元数据指向 Go 包 `br/pkg/utils/iter`。它的直接使用者包括 `br/pkg/restore/Cargo.toml`、`br/pkg/restore/log_client/Cargo.toml` 和 `br/pkg/stream/Cargo.toml`；因此本文件处在 BR 恢复与流式日志处理的惰性数据管道底层，不是独立业务入口。

## 核心职责

- 用 `IterResult<T>` 表达一次拉取的三种互斥状态：`Emit` 产出值、`Throw` 产出错误、`Done` 表示耗尽。
- 用 `TryNextor<T>: Send` 统一可失败、可能阻塞的拉取式迭代器，为 `source.rs`、`combinator_types.rs` 和 `combinators.rs` 提供共同边界。
- 用 `Context`/`CancelFunc` 实现局部的 Go `context.WithCancel` 对等语义，使组合器可在跨线程管道中观测取消。
- 用 `CollectAll` 把惰性流完整物化；用 `AsSeq` 转换为 Rust 标准 `Iterator<Item = Result<T, String>>`。
- 用 `Tap` 和 `WithEmitSizeTrace` 在不改变元素的前提下注入统计或日志副作用。

## 主要符号

- `pub type IterError = String`：Rust 移植的统一错误载体，便于与 Go `error.Error()` 字符串对拍，但不保留结构化错误类型。
- `Context { cancelled, parent }`：`cancelled` 是共享的 `Arc<AtomicBool>`，`parent` 保留祖先链。`background`、`Done`、`Err` 和 `WithCancel` 分别创建根、检查取消、生成标准错误文案和创建子上下文。
- `CancelFunc`：持有 `Arc<dyn Fn() + Send + Sync>`；`Cancel` 将目标子 context 的标志写为 `true`，可克隆且重复调用幂等。
- `IterResult<T> { Item, Err, Finished }`：公开字段与 Go 结构对齐；`Display` 输出 `IterResult.Emit(...)`、`IterResult.Throw(...)` 或 `IterResult.Done()`；`FinishedOrError` 用于快速判断非产出态。
- `convertDoneOrErrResult` 与 `DoneBy`：都把其他元素类型的结束/错误状态转换到新类型，丢弃 `Item`。当前文件内 `CollectAll` 调用 `DoneBy`；前者保留为与 Go 实现和组合器结构对齐的转换辅助。
- `Done`、`Emit`、`Throw`：是三态结果的规范构造器。其不变式分别是只有 `Finished=true`、只有 `Item=Some(T)`、只有 `Err=Some(String)`。
- `Indexed<T> { Index: i32, Item: T }`：为 `combinators.rs::Enumerate` 提供带下标输出。Rust 下标使用 `i32`，而 Go 版是平台宽度的 `int`。
- `TryNextor<T>`：只有 `TryNext(&mut self, &Context) -> IterResult<T>` 一个方法；`&mut self` 使消费进度由单个调用者串行推进，`Send` 允许组合器把实现移入工作线程。
- `CollectAll`：循环调用 `TryNext`，成功时返回 `Item=Some(Vec<T>)` 且 `Finished=false`，出错时立即返回不含部分结果的错误态。
- `AsSeqIter<T>`/`AsSeq`：拥有 context 克隆和装箱的 `TryNextor`，将三态映射为 `Some(Ok(T))` / `Some(Err(String))` / `None`。
- 私有 `Tap<T>` 及公开 `Tap`：仅在正常 `Emit` 路径上以引用调用 `FnMut(&T)`，然后原样重新产出元素。
- `HasSize`/`WithEmitSizeTrace`：把 `Size() -> i32` 转为 `f64` 传给计数回调，其本质是 `Tap` 的一个特化。

## 执行流程

1. 上游通过 `source.rs` 的 `FromSlice`/`OfRange`/`Fail`/`Func` 或恢复模块自定义实现得到 `Box<dyn TryNextor<T>>`。
2. `combinators.rs` 的 `Map`、`FilterOut`、`FlatMap`、`Transform`等继续返回同一 trait 边界，从而可惰性组合。`lib.rs` 将核心协议、source 和 combinator 统一再导出。
3. 终端消费者逐次调用 `TryNext`，或选用 `CollectAll`/`AsSeq`。`CollectAll` 在 `Finished` 时结束，在 `Err` 时丢弃已收集值，否则对 `Item.unwrap()` 并追加。
4. `AsSeqIter::next` 每次只向上游拉取一次。看到错误时返回 `Some(Err)` 但不设置 `finished`，因此消费者若继续请求，仍会继续推进源；只有上游 `Done` 才将适配器永久标记为结束。
5. 若使用 `Tap`，它先拉取上游；`Done`/错误原样短路返回，只有 `Emit` 才执行副作用并保留元素。`WithEmitSizeTrace` 沿用该路径累加每个元素的 size。

## 数据与状态

`IterResult<T>` 用三个字段模拟和类型，而不是 Rust `enum`。合法构造器保证三态互斥，但字段是 `pub`，外部仍可构造“`Finished=true` 同时带 `Item`”之类非法组合。消费代码默认不变式成立：`Display`、`CollectAll`、`AsSeqIter` 和 `Tap` 都可在非法结果上忽略字段或 panic。

`Context::clone` 会复制 `parent` 链的结构，但每一层的 `Arc<AtomicBool>` 仍与原 context 共享。因此取消父节点会被已建立的子链观测到；子节点仅持有自己的取消闭包，反向不会影响父节点。`AsSeqIter` 的 `finished` 是适配器自身的单向状态，一旦看到 `Done` 就不再触发上游。

## 依赖与调用关系

本文件的直接标准库依赖只有 `std::fmt`、`Arc`、`AtomicBool` 和 `Ordering`，Cargo manifest 未声明第三方依赖。内部下游关系包括：

- `source_types.rs` 为 `SliceIter`、`OfRangeIter`、`EmptyIter`、`FailureIter` 和 `FuncIter` 实现 `TryNextor`。
- `combinator_types.rs` 用 `DoneBy` 传递跨元素类型的结束/错误，并用 `Context` 控制并发 `Transform`。
- `combinators.rs::CollectMany` 通过 `TakeFirst` 构建截断流，再调用本文件的 `CollectAll`。
- `br/pkg/restore/log_client/log_file_manager.rs`、`migration.rs` 和 `client.rs` 直接使用 `TryNextor`、`CollectAll` 及取消 context，用于迁移、压缩和日志文件流。
- Go 业务路径 `br/pkg/stream/stream_metas.go` 用 `AsSeq` 遍历 group/备份元数据，`br/pkg/task/stream.go` 用 `WithEmitSizeTrace` 记录加载和切分阶段的元素内存指标。RustCodeGraph 将 `iter.rs` 标记为被 60 个文件使用，说明该协议是跨 BR 子模块的共享边界。

## 错误处理与边界

`Throw` 与 context 错误都用 `String`，上层只能依赖文案而无法使用类型化 downcast。`Context::Err` 无条件返回 `"context canceled"`，调用方应先用 `Done` 确认已取消，不能把 `Err` 当作状态检查。

`CollectAll` 不设置拉取上限：无限源、从不返回 `Done` 的错误源，或忽略 context 的自定义源都可使它永不返回；大流也会将全部元素保留在内存。出错时它通过 `DoneBy` 丢弃已收集元素，这是已有 Rust/Go 测试固定的原子式结果语义。

`Display` 与产出分支都使用 `unwrap`/`expect`；因此实现者必须用 `Done`/`Emit`/`Throw` 构造合法结果。`AsSeq` 上的错误不代表源已耗尽；对 `collect::<Result<Vec<_>, _>>()` 而言标准收集会在首个错误处停止，手动继续 `next` 则可拉取后续元素。

## 并发与资源生命周期

`Context` 的取消标志使用 `Ordering::SeqCst`，取消闭包要求 `Send + Sync`，所以 `CancelFunc` 可在线程间共享。`br/pkg/utils/iter/combinator_test.rs::with_timeout` 就克隆 cancel handle 到后台线程，睡眠后发出取消。这个 context 不管理 deadline、waker 或资源回收，`Done` 也是递归轮询而非阻塞等待；具体 `TryNextor` 必须主动检查 context 才能响应取消。

`TryNextor: Send` 不意味着实现可被多线程并发调用：`TryNext` 需要 `&mut self`，所有权应由单一消费者或外部同步容器管理。`Tap` 的 `FnMut + Send + 'static` 和 `AsSeqIter` 都拥有上游；它们被 drop 时将沿所有权链释放上游。本文件不创建线程、channel、锁、事务或 I/O 句柄；并发生产管道由 `combinator_types.rs` 的 `Transform` 实现负责。

## 与 Go 版本的对应关系

`br/pkg/utils/iter/iter.go` 是直接对照文件。`IterResult`、`Indexed`、`TryNextor`、`DoneBy`、`Done`、`Emit`、`Throw`、`CollectAll`、`Tap`、`AsSeq` 和 `WithEmitSizeTrace` 均有一一对应。两版共同保持：成功 `CollectAll` 是带集合的产出态而非 `Finished`；中途错误不返回部分结果；`Tap` 仅观测产出值；`AsSeq` 把错误交给消费者，并由消费者是否继续决定后续拉取。

主要差异是：

- Go 使用标准 `context.Context` 和 `error`；Rust 在本文件中实现简化 context，错误降级为 `String`。
- Go `AsSeq` 返回 `iter.Seq2[error, T]` 的 yield 闭包；Rust 返回拥有源的标准 `Iterator<Item = Result<T, IterError>>`。Rust 的 `as_seq_test.rs` 明确验证错误后可继续与 `.take(2)` 不多拉取。
- Go `WithEmitSizeTrace` 直接接收 Prometheus `Counter`；Rust 用 `FnMut(f64)` 抽象，该 crate 因而无需 Prometheus Cargo 依赖。
- Go `Tap` 闭包按值接收 `T`；Rust 按 `&T` 观测，避免为副作用克隆或移动元素。
- Go 本文件还定义 `CollectMany`；Rust 将它放在 `combinators.rs`，以 `TakeFirst + CollectAll` 复用组合管道。

## 扩展指南

- 新增迭代源时，在独立源文件中实现 `TryNextor`，始终用 `Done`/`Emit`/`Throw` 生成合法三态，并在同目录独立 `*_test.rs` 中验证耗尽后行为、错误和取消；不应把测试内嵌到 `iter.rs`。
- 新增转换元素类型的组合器时，在 `combinator_types.rs`/`combinators.rs` 接线，并用 `DoneBy` 保留错误与结束态。除非同步迁移 Go 契约和所有调用者，不要改变 `CollectAll` 成功时 `Finished=false` 或错误时丢弃部分结果的语义。
- 要扩展 context 能力（deadline、原因、等待通知）时，需同时检查 `combinator_types.rs` 的并发处理和所有自定义 `TryNextor` 对 `Done`/`Err` 的使用，并增加父取消下传、子取消不上传、多次 `Cancel` 和线程间可见性测试。
- 要强化 `IterResult` 不变式，优先评估改为 enum 对 Go 字段对齐和现有直接字段访问的兼容影响；在完成全部调用点迁移前，构造器与字段优先级不应被随意改动。
- 性能风险集中在 `CollectAll` 的无界内存、`Context::Done` 的祖先链递归以及每元素 `Tap` 回调；兼容风险则集中在错误字符串、`Display` 格式和 Go/Rust 三态对齐。

## 验证依据

- RustCodeGraph `status`：项目索引有 7032 个 Rust 文件；`files --filter br/pkg/utils/iter` 列出本 crate 的 Rust/Go 源和独立测试；`node --file br/pkg/utils/iter/iter.rs --offset 1 --limit 420` 返回全部 273 行、29 个符号，并报告该文件被 60 个文件使用。
- RustCodeGraph 精确查询：`query AsSeq --kind function`、`query CollectAll --kind function`、`query WithEmitSizeTrace --kind function` 均定位到本文件；`query TryNextor --kind trait` 同时暴露 `source.rs`、`combinators.rs` 和 restore/stream 上层入口。`callers`/`callees` 命令两次在 30 秒限制内未返回，因此具体调用点改用定向 `rg` 核对。
- 已读生产与声明路径：`br/pkg/utils/iter/iter.rs`、`lib.rs`、`Cargo.toml`、`source.rs`、`source_types.rs`、`combinators.rs`，以及依赖 manifest `br/pkg/restore/Cargo.toml`、`br/pkg/restore/log_client/Cargo.toml`、`br/pkg/stream/Cargo.toml`。
- 已读 Go 对照与调用路径：`br/pkg/utils/iter/iter.go`、`combinator_types.go`、`combinators.go`、`br/pkg/stream/stream_metas.go`、`br/pkg/task/stream.go`。
- 已读独立测试：`br/pkg/utils/iter/as_seq_test.rs`、`parity_test.rs`、`combinator_test.rs`、`source_test.rs`、`transform_backpressure_test.rs` 和 Go `br/pkg/utils/iter/combinator_test.go`。其中 `as_seq_test.rs` 验证顺序、错误后继续和提前停止；`parity_test.rs`/`combinator_test.rs` 验证三态展示、空收集、中途错误不返回部分值、Tap 调用次数和 context 跨线程取消。
- 本任务是纯文档分析，按计划不运行 Cargo。交付时使用任务文件指定的 11 章节结构命令验证。
