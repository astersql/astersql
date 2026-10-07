# `br/pkg/utils/iter/lib.rs`

## 文件定位

`lib.rs` 是 Cargo 包 `astersql-br-pkg-utils-iter` 的 crate 根文件。`br/pkg/utils/iter/Cargo.toml` 通过 `[lib] path = "lib.rs"` 明确指定它为库入口，并用 `package.metadata.porting.go-package = "br/pkg/utils/iter"` 标记对应的 Go 包。根工作区 `Cargo.toml` 将 `br/pkg/utils/iter` 列为成员；`br/pkg/restore/Cargo.toml`、`br/pkg/restore/log_client/Cargo.toml` 和 `br/pkg/stream/Cargo.toml` 通过路径依赖使用该 crate。

这个文件是门面而不是算法实现文件：它声明五个生产模块，再把这些模块的公开项平铺重导出。调用方因此可以写 `use astersql_br_pkg_utils_iter::{CollectAll, FromSlice, Transform, TryNextor};`，不必知道符号实际位于哪个子模块。恢复链中的直接例子包括 `br/pkg/restore/restorer.rs`、`br/pkg/restore/log_client/client.rs`、`br/pkg/restore/log_client/log_file_manager.rs` 和 `br/pkg/restore/log_client/migration.rs`。

## 核心职责

1. 以 `#[path = "..."] pub mod ...` 组装 `iter`、`source_types`、`source`、`combinator_types`、`combinators` 五层生产实现。
2. 通过四条 `pub use ...::*` 将核心协议、数据源和组合器类型/工厂暴露为稳定的 crate 根 API；这里没有单独重导出语句指向测试模块。
3. 只在 `cfg(test)` 下挂载 `parity_test.rs`、`source_test.rs`、`combinator_test.rs`、`as_seq_test.rs` 和 `transform_backpressure_test.rs`，保证测试实现不进入普通库构建。
4. 在 crate 级允许 Go 移植代码常见的命名与暂未使用项，包括 `non_snake_case`、`non_camel_case_types`、`unused_*` 和 `clippy::all`。这使公开 API 能保留 `TryNext`、`CollectAll`、`WithConcurrency` 等 Go 风格名字，但也会降低编译器和 Clippy 对整个 crate 的告警覆盖。

## 主要符号

`lib.rs` 自身不定义常量、结构体、trait、函数或 `impl`；它定义的是模块边界和导出面：

- `pub mod iter`：核心协议。主要公开项包括三态结果 `IterResult<T>`、拉取接口 `TryNextor<T>`、取消上下文 `Context`/`CancelFunc`，构造器 `Done`/`Emit`/`Throw`，消费器 `CollectAll`，标准迭代器适配器 `AsSeq`，以及 `Tap`/`WithEmitSizeTrace`。
- `pub mod source_types`：源的具体状态类型 `SliceIter`、`OfRangeIter`、`EmptyIter`、`FailureIter`、`FuncIter`；其中 `new_*` 构造函数保持 `pub(crate)`，但具体公开类型会被根门面重导出。
- `pub mod source`：公开源工厂 `FromSlice`、`OfRange`、`Fail`、`Func`，以及测试/直接构造可用的 `SliceSource` 别名。
- `pub mod combinator_types`：`TransformIter`、`FilterIter`、`TakeIter`、`PureMapIter`、`FilterMapIter`、`TryMapIter`、`JoinIter`、`WithIndexIter`、`WorkerPool` 和 `BufferedMappingCfg` 等状态类型。
- `pub mod combinators`：公开工厂与配置 `Transform`、`WithConcurrency`、`WithBufferSize`、`FilterOut`、`TakeFirst`、`FlatMap`、`Map`、`MapFilter`、`TryMap`、`ConcatAll`、`Enumerate`、`CollectMany`。
- 五个私有测试模块只在 `cfg(test)` 生效；测试通过 `use crate::{...}` 验证根级重导出本身可用。

## 执行流程

该入口不执行初始化代码；运行行为由调用方从根 API 选择并组合后发生。典型流程如下：

1. 调用方用 `FromSlice`、`OfRange`、`Fail` 或 `Func` 创建 `Box<dyn TryNextor<T>>`。源在每次 `TryNext(&Context)` 时返回 `Emit`、`Throw` 或 `Done`，而不是预先计算整条序列。
2. 调用方按需包裹 `Map`、`FilterOut`、`TryMap`、`FlatMap`、`Enumerate`、`TakeFirst` 或 `Transform`。顺序组合器每次拉取上游一个或若干结果；结束态和错误态通过 `DoneBy`/`convertDoneOrErrResult` 跨元素类型传播。
3. `CollectAll` 循环拉取直到 `Done`；遇到 `Throw` 时返回错误且不保留已收集的部分结果。`CollectMany` 先用 `TakeFirst` 截断，再复用 `CollectAll`。另一入口 `AsSeq` 把协议适配为标准 `Iterator<Item = Result<T, String>>`。
4. `Transform` 是特殊的并发分支：首次 `TryNext` 才启动生产者线程，生产者拉取上游并通过 `WorkerPool` 派发 mapper，结果经 `mpsc` 通道返回；`bufferSize` 同时限制未消费结果的在飞数量，错误或取消会触发子 context 的 `Cancel`。
5. BR 恢复代码通过这个根门面构造实际流水线。例如 `br/pkg/restore/log_client/log_file_manager.rs` 使用多个组合器生成惰性文件元数据流，`br/pkg/restore/log_client/client.rs` 则直接循环 `TryNext` 或调用 `CollectAll` 消费结果。

## 数据与状态

`lib.rs` 不持有全局变量或运行时状态。状态均属于被重导出的实例：

- `IterResult<T>` 用 `Item: Option<T>`、`Err: Option<String>`、`Finished: bool` 表达互斥的 Emit/Throw/Done 三态；正常构造函数维持互斥，但字段公开，外部代码理论上能构造非法组合。
- `Context` 用 `Arc<AtomicBool>` 保存本节点取消位，并以父链传播祖先取消；子取消不会反向影响父节点。
- 切片源用 `VecDeque` 按 FIFO 消费；区间源保存 `current`、`end` 与 `endExclusive`；索引组合器保存递增的 `i32` 索引；`TakeIter` 保存剩余配额。
- 顺序组合器拥有上游 `Box<dyn TryNextor<_>>` 和闭包，因而每条流水线通常由单个可变消费者驱动。
- `TransformIter` 额外保存启动/结束标志、channel 接收端、在飞计数、取消句柄和生产者 `JoinHandle`；`WorkerPool` 的克隆共享原子在飞计数。

根文件的 `pub use` 会把公开实现类型与字段一并暴露。因此修改子模块的公开性、名字或泛型约束，实际上就是修改这个 crate 的根 API，而不只是内部重构。

## 依赖与调用关系

向下依赖按文件内声明形成清晰层次：

`lib.rs` → `iter.rs`（协议）→ `source_types.rs`（基础源）；`source.rs` 在二者之上提供工厂；`combinator_types.rs` 依赖核心协议与空源；`combinators.rs` 再把具体组合器包装成公开工厂。`Cargo.toml` 没有列出外部 crate 依赖，当前实现只使用标准库的集合、原子、线程和 `mpsc`。

向上调用者主要位于 BR 恢复和流处理：RustCodeGraph 对 `iter.rs`、`source.rs`、`combinators.rs` 分别显示 60、7、15 个引用文件；仓库文本搜索进一步确认根 crate 名被 `br/pkg/restore/**` 的生产代码和测试直接导入。`br/pkg/restore/log_client/stubs.rs` 还在公开签名中返回 `Box<dyn astersql_br_pkg_utils_iter::TryNextor<IngestedSSTsGroup>>`，说明该 trait 是跨 crate 接口的一部分，而非仅供本目录内部使用。

RustCodeGraph 的 `node Transform` 显示本文件族中的 `Transform` 定义位于 `combinators.rs:41`，其直接构造 `TransformIter::new`；`combinators.rs` 内部还显示 `FlatMap` 调用 `Map`、`CollectMany` 调用 `TakeFirst` 和 `CollectAll`。对宽泛的 `callers CollectAll` 查询在本次检查中未在 30 秒内返回，因此上游证据采用 RustCodeGraph 文件引用统计与精确 crate 名搜索交叉验证，不据此声称调用者清单穷尽。

## 错误处理与边界

- 错误类型当前是 `String`。`Throw` 设置 `Err` 且不设置 `Finished`；`CollectAll` 一遇错误就用 `DoneBy` 返回，只保留错误/结束信息，不返回已经累积的元素。
- `Fail` 每次拉取都会克隆并返回同一个错误；它不会在第一次错误后自动变成 `Done`。`TryMap` 的 mapper 错误同样变为 `Throw`，是否继续拉取由调用方决定。
- `AsSeq` 遇到错误会产出一个 `Err`，但不会把源标记为耗尽；若消费者继续调用 `next`，它会继续拉取。这由 `as_seq_yields_error_and_continues_when_consumer_continues` 固化。
- `FilterOut` 中谓词为 `true` 表示丢弃，与标准 Rust `Iterator::filter` 的保留语义相反；`MapFilter` 的布尔值同样以 `true` 表示跳过。
- `OfRange(begin, end)` 是半开区间，`begin >= end` 时不产出；固定宽度整数支持由 `source.rs` 与 `source_types.rs` 的实现宏共同提供。步进使用普通 `+ 1`，靠先判断终止避免正常路径越过上界，但极端整数上界的溢出行为没有在已读测试中覆盖。
- `JoinIter` 的当前子流报错后会把剩余外层流替换为空流，避免错误后继续串接；空子流通过递归跳过，很多连续空子流可能增加栈深度。
- `Transform` 会把零并发修正为至少一；零缓冲在启动时修正为至少一。并发结果顺序不稳定，测试必须比较集合或显式排序。Worker 线程 panic 没有被捕获，可能使共享在飞计数无法归还；这是当前实现边界，不应描述为已具备 panic 隔离。

## 并发与资源生命周期

普通源和组合器本身只要求实现 `Send`，由持有 `&mut dyn TryNextor<T>` 的消费者串行推进；闭包也带 `Send` 约束以允许流水线跨线程移动。

`TransformIter` 在首次拉取时懒启动一个生产者线程，并为每个映射任务经 `WorkerPool::Apply` 再启动工作线程。`bufferSize` 对 `outstanding` 原子计数施加背压；消费者收到结果后才递减，因此未消费结果不会无限拉取上游。生产者退出前自旋等待活动 worker 清空，使 channel 关闭前尽量保留末尾结果或错误。消费者以 5ms `recv_timeout` 周期检查调用方取消和生产者结束状态。

`Context::WithCancel` 创建共享原子取消位的子 context 和可克隆、幂等的 `CancelFunc`。父取消会通过父链被子节点观察，子取消不影响父。Transform 在上游错误、mapper 错误或消费者 context 取消时触发内部取消；正常耗尽则等待工作线程结束。当前类型没有 `Drop` 实现，也不会显式 `join` 保存的生产者句柄；调用方应持续拉取至 `Done`/`Throw`，测试 `transform_does_not_pull_past_unconsumed_buffer` 也在断言后主动 drain，以免测试退出时遗留后台工作。

## 与 Go 版本的对应关系

Rust 的五个生产文件对应同目录 Go 包的 `iter.go`、`source.go`、`source_types.go`、`combinators.go` 和 `combinator_types.go`。核心语义保持一致：`TryNextor` 是可能阻塞或失败的拉取接口；`IterResult` 表达单步结果；源与组合器保持惰性；`CollectAll` 出错不返回部分结果；`FilterOut`/`MapFilter` 的真值表示丢弃；`Transform` 使用取消、并发配额、结果缓冲和 outstanding 背压；并发结果不承诺输入顺序。

Rust 为 Go 泛型和运行时设施采用了本地表达：Go 的 `error` 映射为 `String`，零值 item 映射为 `Option<T>`；`context.Context` 映射为本地 `Context`；`util.WorkerPool` 映射为基于原子计数和 `thread::spawn` 的 `WorkerPool`；Go channel 映射为 `std::sync::mpsc`；Go `iter.Seq2[error, T]` 映射为标准 `Iterator<Item = Result<T, String>>`。

还存在可观察差异：Go `WithEmitSizeTrace` 接收 Prometheus `Counter`，Rust 版本接收 `FnMut(f64)`，把具体指标后端留给调用方；Rust `CollectAll` 的空成功结果是 `Some(Vec::new())`，而 Go 的零值 slice 可能是 `nil`，但两者都保持 `Finished == false`；Go 的 `AsSeq` 由 yield 返回值控制停止，Rust 则由标准 Iterator 消费器停止继续调用；Rust `WorkerPool` 使用自旋/yield 且未复用线程，性能和 panic 生命周期不等同于 Go `util.WorkerPool`。

## 扩展指南

- 新增公开源时，将状态机放在 `source_types.rs`，将易用构造函数放在 `source.rs`；新增组合器时，将具体 `TryNextor` 实现放在 `combinator_types.rs`，工厂放在 `combinators.rs`。只要项为 `pub`，现有 glob 重导出会自动把它加入根 API，因此必须审查命名冲突和意外扩大公开面。
- 新增生产模块时需在本文件显式声明；只有测试模块才应使用 `#[cfg(test)]`。测试逻辑继续放在独立 `*_test.rs` 文件，不要嵌入生产源文件。
- 改动三态协议时，要同步审查所有 `DoneBy`/`convertDoneOrErrResult` 路径、`CollectAll`、`AsSeq` 以及跨 crate 的 `TryNextor` 签名。公开字段允许外部构造，收紧不变量会是兼容性变更。
- 改动并发 Transform 时，至少同步 `combinator_test.rs` 的成功、上游错误、mapper 错误和超时场景，以及 `transform_backpressure_test.rs` 的未消费缓冲界限。应特别评估取消响应、在飞计数泄漏、线程 panic、结果丢失和高负载自旋 CPU 风险。
- 改动 Go 对照 API 时，应同时核对同目录五个 Go 生产文件及 `combinator_test.go`，并在 Rust 的 `parity_test.rs` 增补对等契约。`as_seq_test.rs` 和 `source_test.rs` 覆盖 Rust 特定适配与整数类型完整性。
- crate 级 `allow(clippy::all)` 会隐藏新代码的许多 lint；扩展时应靠局部代码审查和针对性测试弥补，而不应把无告警视为正确性证据。

## 验证依据

- RustCodeGraph 索引状态：项目索引包含 7,032 个 Rust 文件；`files --filter br/pkg/utils/iter` 返回 17 个 Go/Rust 文件。`node --file br/pkg/utils/iter/lib.rs` 验证入口共 64 行、五个生产模块、五个条件测试模块和四组 glob 重导出。
- RustCodeGraph 源码证据：读取了 `iter.rs`、`source.rs`、`source_types.rs`、`combinators.rs`、`combinator_types.rs`；`node Transform` 验证 `Transform` 的定义及 `TransformIter::new` 下游构造关系。文件引用统计与仓库精确搜索共同确认恢复链调用者。
- crate/调用方证据：读取 `br/pkg/utils/iter/Cargo.toml`，并检查根 `Cargo.toml` 的 workspace 成员及 `br/pkg/restore/Cargo.toml`、`br/pkg/restore/log_client/Cargo.toml`、`br/pkg/stream/Cargo.toml` 的路径依赖；精确搜索了 `astersql_br_pkg_utils_iter` 的生产调用点。
- Go 对照证据：读取 `br/pkg/utils/iter/iter.go`、`source.go`、`source_types.go`、`combinators.go`、`combinator_types.go`，逐项核对公开 API、三态结果、组合语义、并发和背压。
- 独立测试证据：读取 `parity_test.rs`、`source_test.rs`、`combinator_test.rs`、`as_seq_test.rs`、`transform_backpressure_test.rs`；同时用 `rg` 确认 `combinator_test.go` 的九个 Go 测试入口。测试覆盖源、公开根导出、顺序组合、错误传播、重复 Done、并发 Transform、取消、AsSeq 继续/停止和背压。
- 按任务约束这是纯文档分析，未运行 Cargo 或代码测试。交付前另行执行任务指定的 11 章节结构校验，并人工检查本文只描述可由上述源码、调用点和测试支持的当前事实。
