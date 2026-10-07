# `pkg/dxf/operator/pipeline.rs`

## 文件定位

本文件属于 `astersql-dxf-operator` crate，由 [`pkg/dxf/operator/lib.rs`](lib.rs) 以公开模块 `pipeline` 导出。它位于 DXF 算子抽象之上：[`pkg/dxf/operator/operator.rs`](operator.rs) 定义 `Operator`/`TunableOperator`，本文件则把一组 `Box<dyn Operator>` 组织成具有统一启动、关闭、状态查询和拓扑描述能力的 `AsyncPipeline`。

当前 Rust 生产代码搜索不到 `NewAsyncPipeline` 或 `GetReaderAndWriter` 的调用点；可见调用均位于独立测试 [`pkg/dxf/operator/pipeline_test.rs`](pipeline_test.rs) 和 [`pkg/dxf/operator/migration_aster_unit_test.rs`](migration_aster_unit_test.rs)。因此它是已实现、由 crate 公开但尚未在 Rust 生产主链接线的基础设施。Go 对照实现则已被 `pkg/dxf/importinto/task_executor.go` 使用。

## 核心职责

- `AsyncPipeline::Execute` 按 `ops` 下标从小到大调用 `Operator::Open`；只有全部成功后才把 `started` 置为 `true`。
- 某个 `Open` 失败时，按相反顺序关闭此前已经成功打开的算子，并原样返回该次打开错误。
- `AsyncPipeline::Close` 按正向顺序尝试关闭每个算子，即使前面已失败也继续收尾；最终只返回遇到的第一个关闭错误，并把 `started` 置为 `false`。
- `String` 将各算子的 `Operator::String` 结果连接成可诊断的管道拓扑。
- `GetReaderAndWriter` 为固定四段拓扑提供第 2、3 个算子的可调接口，供调用方调整 worker 池容量。

本文件只管理生命周期顺序和算子集合，不建立数据通道，也不执行具体数据变换；通道连接由 `compose.rs` 完成，算子行为由 `operator.rs`、`wrapper.rs` 中的实现提供。

## 主要符号

- `pub struct AsyncPipeline`：公开管道类型，但字段私有。
  - `ops: Vec<Box<dyn Operator>>`：异构算子列表；向量顺序同时决定数据流描述、启动顺序和常规关闭顺序。
  - `started: AtomicBool`：记录最近一次完整启动是否成功；不是完整生命周期状态机。
- `pub fn NewAsyncPipeline(ops: Vec<Box<dyn Operator>>) -> AsyncPipeline`：取得算子所有权，构造 `started == false` 的管道。与 Go 的可变参数构造不同，Rust 调用方显式传入装箱后的向量。
- `pub fn Execute(&mut self) -> Result<(), workerpool::Error>`：启动入口。失败算子本身不会被 `Close`，只回滚下标严格小于失败位置的算子。
- `pub fn IsStarted(&self) -> bool`：用原子读取返回启动标志。
- `pub fn Close(&mut self) -> Result<(), workerpool::Error>`：全量关闭入口；保留首个错误但不短路。
- `pub fn String(&self) -> String`：生成 `AsyncPipeline[A -> B]` 形式的拓扑文本；空管道为 `AsyncPipeline[]`。
- `pub fn GetReaderAndWriter(&mut self) -> (Option<&mut dyn TunableOperator>, Option<&mut dyn TunableOperator>)`：仅在算子数恰为 4 时检查下标 1、2 的可调能力。两个位置分别独立转换，四段长度并不保证二者都是 `Some`。

文件没有模块级常量、枚举、独立 trait、泛型参数或条件编译项；`#![allow(dead_code, non_snake_case)]` 保留了与 Go API 对齐的命名并允许当前尚未生产接线的接口。

## 执行流程

1. 调用方先在别处构造并连接算子，再以数据流顺序调用 `NewAsyncPipeline`。构造过程不打开任何资源。
2. `Execute` 从首算子开始逐个调用 `Open`。
3. 如果所有 `Open` 成功，以 `Ordering::SeqCst` 写入 `started = true` 并返回 `Ok(())`。
4. 如果下标 `index` 的 `Open` 失败，循环 `(0..index).rev()`，逆序调用已打开算子的 `Close`。这些回滚关闭错误被丢弃，函数返回原始打开错误，`started` 保持原值。
5. 运行期间，调用方可用 `IsStarted` 观察标志；对四算子管道还可通过 `GetReaderAndWriter` 获取中间可调算子。
6. `Close` 正向遍历全部算子：记住第一个错误，仍继续调用其余算子的 `Close`。遍历结束后先写入 `started = false`，再返回首错或成功。

空管道是合法输入：`Execute` 不进入循环并将其标为已启动，`Close` 将其恢复为未启动。实现没有阻止重复 `Execute`、启动失败后的再次执行、未启动时 `Close`，这些情形是否安全取决于各 `Operator` 实现的幂等性。

## 数据与状态

`AsyncPipeline` 独占 `Vec<Box<dyn Operator>>`，通过动态分派调用不同具体类型的共同生命周期接口。字段不公开，创建后不能通过本 API 增删或重排算子；唯一提供内部可变借用的接口是四段拓扑专用的 `GetReaderAndWriter`。

`started` 只有两处写入：完整 `Execute` 成功后写 `true`，每次 `Close` 完成遍历后写 `false`。打开失败不主动写 `false`，所以若调用方在一个已经成功启动过的管道上再次执行且第二次打开失败，标志可能仍为 `true`；该标志表达当前实现的最近写入，而非对每个算子真实资源状态的扫描。

`GetReaderAndWriter` 使用 `split_at_mut(2)` 将向量拆成不重叠切片，从而安全地同时返回下标 1 和 2 的可变 trait 对象引用。引用生命周期受 `&mut self` 限制，持有它们期间不能再次可变操作管道。

## 依赖与调用关系

- 上游模块：`pkg/dxf/operator/lib.rs` 公开 `pipeline`；当前 Rust 调用者仅见 `pipeline_test.rs` 与 `migration_aster_unit_test.rs`。RustCodeGraph 将文件标记为由 `migration_aster_unit_test.rs` 使用，但其精确 `callers`/`callees` 查询未返回边，因此调用点又用仓库文本搜索核实。
- 下游接口：`AsyncPipeline` 调用 `operator.rs` 中 `Operator::{Open, Close, String, as_tunable_operator_mut}`，并把可调对象暴露为 `TunableOperator`。
- 错误类型：公开结果使用 crate 重新导出的 `workerpool::Error`。`Cargo.toml` 通过路径依赖 `../../resourcemanager/pool/workerpool` 引入该 crate，包名为 `astersql-resourcemanager-pool-workerpool`。
- 标准库：`AtomicBool` 与 `Ordering` 管理启动标志；`Vec`、`Box<dyn Operator>` 管理有序异构集合。
- crate 边界：`pkg/dxf/operator/Cargo.toml` 没有 feature 声明；本文件自身不直接使用该 crate 的 `crossbeam-channel`、`fail`、`log` 或开发依赖 `regex`。
- 数据流关系：本文件假设传入顺序已经反映数据流方向，但不会验证 `Compose` 建立的通道是否与 `ops` 顺序一致。

## 错误处理与边界

- `Execute` 返回第一个遇到的 `Open` 错误，不包装也不合并错误。
- 启动回滚只关闭已经成功打开的前置算子，顺序为后开先关；失败算子和尚未访问的后续算子不会关闭。
- 回滚期间的所有 `Close` 错误都通过 `let _ = ...` 丢弃，以保留触发失败的 `Open` 错误。这意味着调用方无法从返回值判断回滚是否彻底。
- 常规 `Close` 不因错误短路，保证每个算子都有一次收尾机会；如果多个关闭失败，后续错误不进入返回值。
- 无论常规关闭是否报错，`started` 都被置为 `false`，所以该标志不表示所有资源均成功释放。
- `GetReaderAndWriter` 对非四段管道直接返回 `(None, None)`；四段管道中，不实现 `TunableOperator` 的对应算子也返回 `None`，不会 panic。
- `String` 完全信任各算子的字符串实现，不转义名称，也不保证名称唯一。
- API 不检查空管道、重复启动、重复关闭或并行生命周期调用；扩展时不能假设这些情况已经由本类型拒绝。

## 并发与资源生命周期

管道方法本身不生成线程或异步任务；真正的 worker 启停发生在具体 `Operator::Open/Close` 中。例如 `AsyncOperator::Open` 启动 `WorkerPool`，`Close` 调用 `Release` 等待并释放池资源。

启动标志使用 `SeqCst` 原子读写，提供全局最强原子顺序；不过 `Execute`、`Close` 和 `GetReaderAndWriter` 都要求 `&mut self`，而 `ops` 中的 `dyn Operator` 也没有在此处声明 `Send`/`Sync` 边界，所以本类型没有承诺可由多个线程并发驱动生命周期。原子标志主要复刻 Go 的 `atomic.Bool` 状态语义，不能替代对算子集合的同步。

资源顺序有两个不同规则：启动失败回滚是逆序，正常 `Close` 是正序。这与当前 Go 实现及迁移测试一致。调用方应在完成数据处理后显式调用 `Close`；`AsyncPipeline` 没有实现 `Drop`，离开作用域不会由本类型自动逐个关闭算子。

## 与 Go 版本的对应关系

直接对照文件是 [`pkg/dxf/operator/pipeline.go`](pipeline.go)。Rust 保留了 `AsyncPipeline`、`Execute`、`IsStarted`、`Close`、`NewAsyncPipeline`、`String`、`GetReaderAndWriter` 的名称和主要控制流：顺序打开、失败逆序关闭前置算子、正常关闭全量遍历并返回首错、四段拓扑选择下标 1/2。

主要语言差异如下：

- Go 构造器接收 `ops ...Operator` 并返回指针；Rust 接收 `Vec<Box<dyn Operator>>` 并按值返回所有者。
- Go 通过运行时类型断言 `p.ops[i].(TunableOperator)` 获取可调能力；Rust 通过 `Operator::as_tunable_operator_mut` 返回 `Option<&mut dyn TunableOperator>`。
- Rust 为了同时返回两个可变引用使用 `split_at_mut`；Go 接口值没有对应的借用检查问题。
- Go `atomic.Bool` 与 Rust `AtomicBool` 均用于启动标志；Rust 明确使用 `SeqCst`。
- Go 生产调用 `pkg/dxf/importinto/task_executor.go` 中用两算子管道执行导入编码流程；仓库内尚无对应 Rust 生产调用，因此不能据 Go 接线宣称 Rust 管道已进入完整应用主链。

Rust 的 `pipeline_test.rs` 移植了 Go 的多算子词频集成场景；额外的 `migration_aster_unit_test.rs` 更细地覆盖了失败回滚顺序、全量关闭首错规则和四算子可调接口。

## 扩展指南

- 修改启动或回滚策略时，优先改 `AsyncPipeline::Execute`，并同步 `migration_aster_unit_test.rs::execute_rolls_back_opened_operators_in_reverse_order`；要明确失败算子是否也应关闭，以及如何报告回滚错误。
- 修改关闭顺序或错误聚合时，改 `AsyncPipeline::Close`，同步 `close_visits_every_operator_and_returns_the_first_error`，同时核对 Go `pipeline.go`，避免无意偏离其首错语义。
- 支持不同拓扑或不同可调位置时，不应继续硬编码下标；应重新设计 `GetReaderAndWriter` 的角色表达，并同步 `reader_and_writer_are_only_exposed_for_four_operator_pipelines` 以及 Go 调用约束。
- 新增可调算子类型时，必须在其 `Operator` 实现中正确覆盖 `as_tunable_operator_mut`，否则即使实现了 `TunableOperator`，管道仍只能得到 `None`。
- 若要支持并发共享管道，需要先为 `Operator` trait、内部对象和生命周期方法建立清晰的 `Send`/`Sync` 与锁策略；不能只依赖现有 `AtomicBool`。
- 若要保证异常路径自动释放，应评估 `Drop` 与可能阻塞/失败的 `Operator::Close` 是否兼容；Rust 的 `Drop` 不能返回关闭错误。
- 测试应继续放在独立文件 `pipeline_test.rs` 或 `migration_aster_unit_test.rs`，不要把测试模块内嵌回生产源文件。生产接线后还应在直接调用模块的独立测试中覆盖空管道、重复生命周期调用及关闭失败后的资源状态。

## 验证依据

- 源码：`pkg/dxf/operator/pipeline.rs`，核对全部 1–113 行以及 `AsyncPipeline` 的七个公开符号。
- trait 与真实资源行为：`pkg/dxf/operator/operator.rs`，核对 `Operator`、`TunableOperator`、`AsyncOperator` 的 `Open/Close` 和可调接口转换。
- crate 声明与入口：`pkg/dxf/operator/Cargo.toml`、`pkg/dxf/operator/lib.rs`，核对 crate 名、路径形式的 `workerpool` 依赖、无 feature 声明及公开模块/测试装配。
- Go 对照：`pkg/dxf/operator/pipeline.go`、`pkg/dxf/operator/pipeline_test.go`，以及生产调用 `pkg/dxf/importinto/task_executor.go:346-351`。
- Rust 测试：`pkg/dxf/operator/pipeline_test.rs` 验证五段真实异步数据流、拓扑字符串和关闭时上下文错误；`pkg/dxf/operator/migration_aster_unit_test.rs` 验证逆序回滚、首个关闭错误、启动标志和四段 reader/writer。
- RustCodeGraph：`status` 显示索引包含 `pkg/dxf/operator/pipeline.rs`（8 个符号）；`files --filter pkg/dxf/operator` 列出 Rust/Go 源与测试；`query AsyncPipeline`、`query NewAsyncPipeline --kind function` 定位 Rust/Go 对照符号；`node --file pkg/dxf/operator/pipeline.rs --offset 1 --limit 180` 返回完整源码并报告测试文件使用关系。对限定名执行 `callers`/`callees` 未得到调用边，因此以 `rg` 补充验证生产调用缺失和测试调用位置。
- 人工复核边界：确认正常关闭为正序而非逆序、回滚关闭错误被忽略、启动失败不重置旧标志、四算子长度不保证可调接口存在，且当前 Rust 侧未生产接线。
