# `br/pkg/utils/iter/combinators.rs`

## 文件定位

本文件属于 Cargo 包 `astersql-br-pkg-utils-iter`（见同目录 `Cargo.toml`），是 BR 惰性迭代器 crate 的公开“组合器工厂”层。`lib.rs` 以 `pub mod combinators` 挂载本文件，并通过 `pub use combinators::*` 将这里的 API 扁平导出；具体的 `TryNextor` 实现位于 `combinator_types.rs`，核心协议 `Context`、`IterResult`、`TryNextor` 与 `CollectAll` 位于 `iter.rs`，数据源工厂位于 `source.rs`/`source_types.rs`。

因此本文件不直接实现逐项拉取算法，而是把上游 `Box<dyn TryNextor<T>>`、用户闭包及配置组装为具体迭代器。RustCodeGraph 显示该文件被 15 个文件使用；实际业务调用集中在 `br/pkg/restore/log_client/{migration.rs,log_file_manager.rs}` 和 `br/pkg/restore/restorer.rs`，测试入口则包括 `combinator_test.rs`、`parity_test.rs`、`transform_backpressure_test.rs` 与 `migration_test.rs`。

## 核心职责

1. 提供同步惰性组合器：`FilterOut`、`TakeFirst`、`FlatMap`、`Map`、`MapFilter`、`TryMap`、`ConcatAll`、`Enumerate`。这些函数只构造状态机，真正工作发生在调用方后续调用 `TryNext` 时。
2. 提供并发、有副作用的映射入口 `Transform`，并用 `WithConcurrency`、`WithBufferSize` 配置工作池与反压窗口。
3. 提供终结操作 `CollectMany`：用 `TakeFirst` 截断后复用 `CollectAll`，一次性驱动并收集至多 `n` 项。
4. 统一以 `Box<dyn TryNextor<_>>` 隐藏具体实现类型，使恢复链路可以动态拼装不同来源和组合器；代价是堆分配、动态分派，并且输入迭代器所有权会被消费。

本文件没有模块级常量、条件编译项或私有函数；除导入外，全部声明都是公开类型别名或公开函数。

## 主要符号

- `TransformConfig = Box<dyn FnMut(&mut BufferedMappingCfg) + Send>`：可异构存入 `Vec` 的配置闭包。闭包按传入顺序执行，后出现的同类配置覆盖前值。
- `WithConcurrency(n)`：把 `cfg.quota` 设为名为 `transforming` 的 `WorkerPool`。`WorkerPool::new` 在 `combinator_types.rs` 中把零并发钳制为 1。
- `WithBufferSize(n)`：直接写入 `cfg.bufferSize`。`TransformIter::start` 最终把实际 buffer 钳制为至少 1。
- `Transform(it, with, cs)`：默认 `bufferSize=1`；应用配置后，未指定工作池时以 buffer 构造 `max-concurrency` 池；若工作池上限大于 buffer，则把 buffer 抬高到并发上限；最后构造 `TransformIter`。mapper 接收 `&Context`，返回 `Result<R, String>`。
- `FilterOut(it, f)`：构造 `FilterIter`；谓词为 `true` 表示丢弃，这与标准库 `Iterator::filter` 的保留语义相反。
- `TakeFirst(inner, n)`：构造 `TakeIter`，最多拉取 `n` 项；`n==0` 时不访问上游。
- `FlatMap(it, mapper)`：先以本文件的 `Map` 得到“子迭代器流”，再以 `JoinIter` 顺序展平；初始 `current` 是 `empty()`。
- `Map(it, mapper)`：构造顺序、不可失败的 `PureMapIter`。
- `MapFilter(it, mapper)`：构造 `FilterMapIter`；mapper 返回 `(value, skip)`，其中 `skip=true` 表示丢弃。
- `TryMap(it, mapper)`：构造 `TryMapIter`；mapper 的 `Err(String)` 在拉取时变为 `IterResult::Throw`。
- `ConcatAll(items)`：以 `FromSlice(items)` 产生子迭代器流，再由 `JoinIter` 顺序拼接；空 `Vec` 会立即耗尽。
- `Enumerate(it)`：构造初始索引为 0 的 `WithIndexIter`；索引类型是 `i32`，只在成功发出元素后递增。
- `CollectMany(ctx, it, n)`：唯一立即消费输入的函数；调用 `CollectAll(ctx, &mut *TakeFirst(it,n))`，返回 `IterResult<Vec<T>>`。

## 执行流程

同步链的共同流程是：调用工厂消费上游并保存闭包；调用方再以 `TryNext(ctx)` 拉取；具体状态机先从上游取得 `IterResult`，对 `Emit` 做映射、过滤、计数或展平，对 `Done`/`Throw` 则转换类型或原样传播。`FlatMap`/`ConcatAll` 的 `JoinIter` 会持续消费当前子迭代器，当前子流结束后才切换下一个；空子流通过再次拉取跳过。

`Transform` 的流程更复杂：

1. 工厂先折叠配置并维护“不允许并发上限大于缓冲上限”的不变量，然后仅创建尚未启动的 `TransformIter`。
2. 首次 `TryNext` 时，`TransformIter::start` 从调用方 context 派生可取消子 context，启动生产者线程，并建立结果 channel。
3. 生产者在 `outstanding < buffer` 时拉取上游，把每个元素提交给共享 `WorkerPool`；worker 并发执行 mapper，把 `Emit` 或 `Throw` 送回 channel。
4. 消费者每取走一个结果才递减 `outstanding`，从而释放下一次上游拉取额度。`transform_backpressure_test.rs::transform_does_not_pull_past_unconsumed_buffer` 验证 buffer 为 2 时，消费一项后上游最多被拉取 3 次。
5. 上游结束后生产者等待活动 worker 清空再退出；消费者检测到生产者结束且 channel 已空时返回 `Done`。并发结果不保证输入顺序，`combinator_test.rs::test_par_trans` 因而用排序后的多重集比较。

`CollectMany` 则构造 `TakeFirst` 后立即调用 `CollectAll`，所以它不是惰性返回组合器；当前 `CollectAll` 契约使成功结果的 `Finished` 为 `false`，该行为由 `parity_test.rs::go_rust_public_contract_matches` 明确断言。

## 数据与状态

本文件本身只创建配置和状态对象。同步组合器的重要状态分别是：`TakeIter.n` 的剩余配额、`JoinIter.current` 的当前子流、`WithIndexIter.index` 的下一个编号，以及各实现持有的 `FnMut` 闭包。由于闭包和上游都存入 trait object，API 要求元素可 `Send`，且返回的组合器通常要求捕获环境为 `'static`。

`Transform` 的配置状态包含 `BufferedMappingCfg.bufferSize` 与可选 `quota`。具体运行态在 `TransformIter`：`started`/`finished`、上游 `inner`、mapper、结果接收端、取消句柄、`outstanding` 原子计数、生产者 `JoinHandle` 及预留的 `pending` 队列。`WorkerPool` 的 clone 共享 `Arc<AtomicUsize>` 在飞计数。

所有组合器以 `IterResult<T>` 表达三态：`Emit` 携带元素，`Done` 表示耗尽，`Throw` 携带字符串错误。它们不缓存完整输入；除 `CollectMany` 收集到 `Vec`、`Transform` 保存有限在飞结果外，数据按需流动。

## 依赖与调用关系

直接下游依赖如下：

- `crate::combinator_types`：`BufferedMappingCfg`、`WorkerPool` 和九种具体迭代器状态机。
- `crate::iter`：`Context`、`TryNextor`、`Indexed`、`IterResult`、`CollectAll`。
- `crate::source::FromSlice`：把 `ConcatAll` 的 `Vec` 转成子迭代器源。
- `crate::source_types::empty`：为 `FlatMap`/`ConcatAll` 提供初始空子流。

Cargo 清单没有 `[dependencies]`；这些依赖均为同一 crate 内部模块。`lib.rs` 是公开入口，不要求调用方写出 `combinators` 模块路径。

上游业务证据包括：`migration.rs` 用 `FilterOut`、`MapFilter`、`ConcatAll`、`FlatMap` 组织日志迁移条目；`log_file_manager.rs` 用 `FilterOut`、`FlatMap`、`Enumerate` 与 `ConcatAll` 构造日志文件扫描和统计流；`restorer.rs` 用 `FilterOut` 后接 `TryMap` 处理拆分结果。RustCodeGraph 对 `MapFilter` 还识别到 `log_file_manager.rs::MapFilterFromSlice` 的直接包装。此文件本身不直接连接网络、磁盘或数据库，副作用来自上游 `TryNextor` 或调用方传入的闭包。

## 错误处理与边界

- `Map`、`FilterOut`、`TakeFirst`、`MapFilter`、`Enumerate` 在上游结束或出错时不调用用户闭包，并传播对应状态。
- `TryMap` 把 mapper 的 `Err(String)` 变为 `Throw`；它不会自动永久封闭迭代器，调用者应把错误视为终止信号。
- `JoinIter` 遇到当前子流错误时把剩余 `inner` 替换为空流，阻止错误后继续产生后续子流元素；`combinator_test.rs::test_failure` 验证 `CollectAll` 返回错误且不返回部分 `Item`。
- `Transform` 遇到上游错误、mapper 错误或 context 取消时触发子 context 取消。消费者观察到错误后标记 finished；之后拉取返回 `Done`。`test_error_during_transforming` 和 `test_error_before_transforming` 分别覆盖 mapper 错误与启动前上游错误，后者还以 1 秒超时防止死锁。
- `WithConcurrency(0)` 的实际工作池上限为 1；`WithBufferSize(0)` 在启动时实际 buffer 为 1。重复配置采用最后一次写入；配置顺序不会破坏最终的 `buffer >= concurrency` 校正。
- `TakeFirst` 在每次向上游拉取前先递减配额；若上游提前 `Done`/`Throw`，不会尝试补足数量。
- `Enumerate` 的索引是 `i32`，极端超过 `i32::MAX` 的流在调试构建可能溢出、发布构建可能回绕；当前实现没有显式保护。
- `JoinIter` 用递归跳过结束的子迭代器，大量连续空子流存在栈深风险；这是实现边界，不应在文档中误称为常量栈空间。
- 用户闭包 panic 没有转换为 `IterResult`；尤其 `WorkerPool::Apply` 未捕获 worker panic，可能令计数或结果生命周期异常。

## 并发与资源生命周期

除 `Transform` 外，组合器均在调用 `TryNext` 的线程中同步执行，不自行创建线程；所有权随 `Box` 链传递，组合器被 drop 时其上游与闭包随之释放。

`Transform` 首次拉取才懒启动一个生产者线程，并为每个映射任务通过 `WorkerPool::Apply` 启动 worker 线程。`quota` 限制同时执行的 worker 数，`outstanding` 与 buffer 限制尚未被消费者取走的结果数；两者共同提供并发上限和反压。工作池通过 `AtomicUsize` 加 `yield_now` 自旋，不保证公平，高负载时可能耗费 CPU。

父 context 取消会由消费者轮询发现并调用本地 cancel；mapper 和生产循环共享子 context。生产者退出前等待活动 worker 归零，避免 channel 过早断开而丢失末尾错误。当前类型没有 `Drop` 实现来显式 join 生产者；安全扩展时必须继续保证提前停止消费或丢弃迭代器不会留下永久阻塞的后台工作，并应在独立测试中覆盖取消、错误、buffer 满和 consumer 提前结束。

## 与 Go 版本的对应关系

`combinators.rs` 主要逐项对应 `br/pkg/utils/iter/combinators.go`：配置闭包、默认 buffer、默认 worker pool、`buffer >= quota.Limit()` 校正，以及 `FilterOut`、`TakeFirst`、`FlatMap`、`Map`、`MapFilter`、`TryMap`、`ConcatAll`、`Enumerate` 的组合结构均保持 Go 语义。`CollectMany` 的 Go 定义位于 `iter.go` 而非 `combinators.go`；Rust 为便于公开工厂归类把它放在本文件，仍保持 `TakeFirst + CollectAll` 的实现。

关键语言差异是：Go 用泛型接口值和可变参数，Rust 使用 `Box<dyn TryNextor<_>>` 与 `Vec<TransformConfig>` 并显式消费所有权；Go mapper 返回 `error`，Rust 使用 `Result<_, String>`；Go `FilterOut` 的谓词按值接收元素，Rust 为避免消费后无法保留而接收 `&T`；Go `ConcatAll(items ...TryNextor<T>)` 对应 Rust 的 `ConcatAll(Vec<Box<dyn TryNextor<T>>>)`。

`combinator_test.rs` 与 `combinator_test.go` 场景一一对照，覆盖并发变换、过滤/展平、枚举、拼接失败、有限收集和错误传播；`parity_test.rs::go_rust_public_contract_matches` 补充全部公开组合器的烟雾级契约。当前 Rust `TransformIter` 以标准线程、channel 和自旋配额模拟 Go context、worker pool 与 buffered mapping，结构不同但目标行为相同。

## 扩展指南

新增同步组合器时，应把公开工厂放在本文件，把独立状态机及 `TryNextor` 实现放在 `combinator_types.rs`，不要把测试内嵌进生产源文件；在 `combinator_test.rs` 增加与 Go `combinator_test.go` 对齐的行为用例，并在 `parity_test.rs` 增加公开契约断言。如果 Go 版本先有对应 API，应同时核对同路径 Go 实现的结束态、错误态和谓词方向。

修改 `Transform` 配置或调度时，重点维持：实际 buffer 至少为 1、buffer 不小于并发上限、只有消费结果才释放 outstanding、任何错误/取消都能终止生产且不丢失已发送错误。除 `combinator_test.rs` 的正常/错误路径外，必须同步扩展 `transform_backpressure_test.rs`；涉及实际恢复使用方式时，还应检查 `restore/log_client` 与 `restorer.rs` 的调用链及其独立测试。

修改 `FlatMap`/`ConcatAll` 时应验证空输入、连续空子流、当前子流报错后不再消费后续子流；修改 `Enumerate` 时要明确索引类型与溢出策略；修改 `CollectMany` 时要保留 `CollectAll` 的 `Item`/`Err`/`Finished` 契约。性能评审应关注 trait-object 分派、每层 box 分配、`JoinIter` 递归深度以及 `Transform` 每任务建线程和自旋等待的成本。

## 验证依据

- RustCodeGraph：`status` 检查本地索引；`files --filter br/pkg/utils/iter` 定位模块；`node --file br/pkg/utils/iter/combinators.rs` 读取全部 172 行并确认 15 个使用文件；`query` 精确定位 `Transform`、`FlatMap`、`MapFilter`、`TryMap`、`CollectMany`；`node` 读取 `combinator_types.rs` 和 `transform_backpressure_test.rs`，核对下游状态机、取消、反压与错误路径。
- 源码与 crate 边界：`br/pkg/utils/iter/combinators.rs`、`combinator_types.rs`、`iter.rs`、`lib.rs`、`Cargo.toml`。
- Go 对照：`br/pkg/utils/iter/combinators.go` 与 `iter.go`。
- 独立测试：`br/pkg/utils/iter/combinator_test.rs`、`combinator_test.go`、`parity_test.rs`、`transform_backpressure_test.rs`；调用侧测试还包括 `br/pkg/restore/log_client/migration_test.rs`/`.go`。
- 业务调用搜索：`br/pkg/restore/log_client/migration.rs`、`log_file_manager.rs`、`br/pkg/restore/restorer.rs`。
- 本任务是纯文档分析，按计划不运行 Cargo；交付前使用任务指定命令验证目标文件存在且固定二级标题恰为 11 个，并人工复核只新增本文档、未修改 Rust/Go/Cargo/`plan.md`。
