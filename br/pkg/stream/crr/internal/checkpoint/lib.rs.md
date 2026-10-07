# `br/pkg/stream/crr/internal/checkpoint/lib.rs`

## 文件定位

[`lib.rs`](./lib.rs) 是 Cargo 包 `astersql-br-pkg-stream-crr-internal-checkpoint` 的 crate 根。`Cargo.toml` 以 `[lib] path = "lib.rs"` 明确这一边界，并在 `package.metadata.porting.go-package` 中把它对应到 Go 包 `br/pkg/stream/crr/internal/checkpoint`。它位于 CRR（跨区域复制）检查点服务的内核层：上层 `br/pkg/stream/crr/service/service.rs` 通过此 crate 的公开接口驱动检查点计算，`br/pkg/stream/crr/config/config.rs` 通过此 crate 取得计算器配置类型与默认值。

该文件本身是门面而不是算法实现：它不定义业务函数或状态，只负责挂载 `calculator`、`doc`、`progress`、`storage` 四个生产模块，挂载七个仅测试可见的独立测试模块，并把核心公开 API 汇总到 crate 根。

## 核心职责

1. 用 `#[path = "..."] pub mod ...` 固定四个生产模块的物理文件与公开模块名：`calculator.rs` 定义公共契约和主入口，`doc.rs` 保存包级算法说明，`progress.rs` 实现单轮推进，`storage.rs` 实现增量 backupmeta 扫描与解析。
2. 用 `pub use calculator::*` 扁平导出计算器 API，使调用方可直接导入 `Calculator`、`NewCalculator`、`CheckpointCalculatorConfig`、`Context`、`Observer`、`PersistentState` 等，而不必经过 `calculator::`。
3. 用 `pub use astersql_br_pkg_streamhelper::Store` 统一再导出 PD store 描述类型。这样实现 `PDMetaReader::Stores` 的服务层与测试夹具不需要额外依赖该类型的原始模块路径。
4. 在 `cfg(test)` 下把测试放在独立文件中，满足测试逻辑不与生产源文件混编的仓库约束；这些模块只进入本 crate 的测试构建，不构成下游公开 API。
5. 通过 crate 级 `#![allow(...)]` 容纳从 Go 迁移而来的命名和暂未使用接口，包括 `non_snake_case`、`non_camel_case_types`、`unused_imports` 与 `clippy::all`。这是迁移兼容措施，也意味着 lint 不会替本 crate 发现这些类别的问题。

## 主要符号

- `pub mod calculator`：公共 API 的真实定义处。主要符号包括 `NewCalculator(...) -> Result<Calculator, Error>`、有状态的 `Calculator`、`CalculatorDeps`、`CheckpointCalculatorConfig`、`CheckpointEvent`、`EventType`、`PersistentState`，以及 `PDMetaReader`、`UpstreamStorageReader`、`ObjectSyncChecker`、`Observer` 等注入契约。
- `pub mod doc`：只承载 CRR 安全检查点算法的包级说明，无运行时代码。其核心不变量是不能仅由上游 checkpoint 或单个 `flushTS` 判定复制安全，必须等待本轮发现的对象全部同步。
- `pub mod progress`：在 `impl Calculator` 上提供 `poll_upstream_checkpoint`、`load_alive_stores`、`plan_round`、`wait_object_sync`、`advance_synced_state` 等内部步骤；这些不是 `lib.rs` 直接再导出的独立顶层函数。
- `pub mod storage`：提供 backupmeta 增量扫描和加载实现；其内部类型与函数大多为 `pub(crate)` 或私有，因此即使模块本身公开，下游也只能使用显式公开的项。
- `pub use calculator::*`：crate 的主要兼容门面。新增到 `calculator.rs` 的任何 `pub` 项都会自动成为 crate 根导出，属于需要审查的 API 面变化。
- `pub use astersql_br_pkg_streamhelper::Store`：唯一来自外部 crate 的显式再导出。
- `parity_test`、`checkpoint_calculator_test`、`integration_test`、`progress_test`、`storage_test`、`randomized_integration_test`、`storage_internal_test`：七个 `#[cfg(test)]` 私有模块，对应各自独立的 `*_test.rs` 文件。

## 执行流程

`lib.rs` 被依赖方链接时只完成模块组织和名称解析，不执行初始化函数，也不创建线程、锁或 I/O 资源。实际运行链如下：

1. `br/pkg/stream/crr/config/config.rs` 构造 `CheckpointCalculatorConfig`，使用本 crate 导出的 `DefaultPollInterval` 与 `DefaultMetaReadConcurrency`。
2. `br/pkg/stream/crr/service/service.rs::New` 组装 `CalculatorDeps`，把 `StatusObserver` 装箱为 `Observer`，再调用 crate 根导出的 `NewCalculator`。
3. `service.rs::Service::run_once` 在互斥锁保护下调用 `Calculator::ComputeNextCheckpoint`。该方法依次读取上游全局 checkpoint、加载存活 store、规划待同步对象、等待对象同步、推进按 store 水位与 `last_checkpoint`。
4. `progress.rs` 调用 `storage.rs` 的增量扫描/加载逻辑；成功或失败事件经 `Calculator::observe` 送往 `br/pkg/stream/crr/service/status.rs::StatusObserver`，供状态与指标消费。
5. 服务层在检查点推进后读取 `StateSnapshot` 并持久化；重启时先调用 `RestorePersistentState`，且必须发生在首次有效计算之前。

因此，`lib.rs` 的作用是让第 1～5 步共享一个稳定的导入面；任何计算分支都发生在子模块或上层服务中。

## 数据与状态

本文件没有静态可变数据、全局单例或实例字段。经门面暴露的关键状态由 `calculator.rs::Calculator` 持有：配置 `cfg`、依赖 `deps`、可选观察者 `observer`，以及包含 `last_checkpoint`、`synced_ts`、`synced_by_store` 的内部 `calculatorState`。`PersistentState` 是该进度的可持久化快照；`StateSnapshot` 对 map 做深拷贝，避免调用方修改内部状态。

需要区分两个水位：`last_checkpoint` 是最近成功返回的上游全局 checkpoint，`synced_ts` 是对象复制验证完成后的全局扫描水位。后者由按 store 进度的约束计算，不能因为上游 checkpoint 变大而直接提升。`Store` 再导出承载 `PDMetaReader::Stores` 返回的存活 store 信息，用于阻止缺失 store 进度时错误推进。

## 依赖与调用关系

crate 的直接 Cargo 依赖为：

- `astersql-br-pkg-stream-backupmetas`：`storage.rs` 调用其 `ParseName` 解析 backupmeta 文件名。
- `astersql-br-pkg-streamhelper`：`calculator.rs` 使用并由 `lib.rs` 再导出 `Store`。
- `serde` 与 `serde_json`：`storage.rs` 反序列化 backupmeta JSON。

直接上游依赖包括 `br/pkg/stream/crr/service` 和 `br/pkg/stream/crr/config`，它们的 `Cargo.toml` 以路径依赖引用本 crate。代码调用边以 `service.rs::New -> NewCalculator`、`Service::run_once -> Calculator::ComputeNextCheckpoint`、`Service::initialize_resume_state -> Calculator::RestorePersistentState` 为主；`status.rs` 实现本 crate 的 `Observer`，`metrics.rs` 使用 `EventType`。下游调用边由 `ComputeNextCheckpoint` 进入 `progress.rs`，再进入 `storage.rs` 和注入的 PD、上游存储、同步检查接口。

RustCodeGraph 对 `lib.rs` 的文件级“used by”只识别到 `tools/tazel/parity_test.rs`，但基于 crate 名称的跨 crate Rust 引用由 `rg` 明确定位到 `crr/service`、`crr/config` 及其测试；因此不能把单一文件级图边误解为该 crate 未接线。

## 错误处理与边界

`lib.rs` 不产生或捕获运行时错误。错误契约由再导出的 `calculator::Error` 和 `Result<_, Error>` 接口提供：构造阶段拒绝空任务名及不支持增量 meta 扫描的存储 URI；计算阶段会包装 PD、Walk、读取、解析和同步检查错误，并发出 `EventCalculationFailed`；上下文取消和超时分别稳定返回 `context canceled` 与 `context deadline exceeded`。

门面层有三个重要边界：第一，`pub use calculator::*` 只提升 `calculator.rs` 中的公开项，不会绕过 `pub(crate)`/私有可见性；第二，测试模块受 `cfg(test)` 限制，生产依赖无法导入其中夹具；第三，crate 级宽泛 `allow` 可能掩盖未使用导出或命名漂移，扩展时必须靠测试与人工 API 审查补足。

## 并发与资源生命周期

本文件不启动并发任务，也不拥有资源。并发与生命周期由它挂载的实现及上层服务承担：`progress.rs::plan_round` 使用有界线程并发读取 meta，并用 `Mutex`、`Condvar` 和 RAII guard 限制 `MetaReadConcurrency`；下游对象同步轮询保持单线程。`calculator.rs::Context` 用 `Arc<AtomicBool>` 表示取消链与 deadline，子取消不反向取消父上下文。

`Calculator` 应跨轮复用以保留水位状态；`RestorePersistentState` 只允许在计算尚未开始时调用。服务层用 `Mutex<Calculator>` 串行化计算与恢复，用 `RunStopGuard` 在退出时尽力刷写待持久化状态。观察者回调由计算路径同步调用，因此 `Observer` 实现必须避免阻塞或回写同一个 `Calculator`，否则会拖慢轮询，甚至与服务层锁形成重入风险。

## 与 Go 版本的对应关系

Go 包没有与 Rust `lib.rs` 一一对应的入口文件；Go 通过同目录文件天然组成 `package checkpoint`，Rust 必须用 crate 根显式完成同样的聚合。对应关系是：Rust `calculator.rs` ↔ Go `calculator.go`，Rust `doc.rs` ↔ Go `doc.go`，Rust `progress.rs` ↔ Go `progress.go`，Rust `storage.rs` ↔ Go `storage.go`。

Rust `doc.rs`/Go `doc.go` 共同描述同一安全模型：扫描 `flushTS > syncedTS` 的 meta，等待关联对象完成复制，并以仍有约束力的各 store 进度最小值推进全局 `syncedTS`。Rust 使用 trait 对象、`Result`、`HashMap` 和自建 `Context` 映射 Go 的 interface、error、map 与 `context.Context`；`SyncedByStoreSet` 则显式补偿 Rust `HashMap` 无法表达 Go nil map 的差异。Rust 的 `lib.rs` 额外承担公开 API 扁平再导出和独立测试模块挂载，这些在 Go 中无需显式代码。

测试意图也保持对应：`checkpoint_calculator_test.rs` 对照 Go `checkpoint_calculator_test.go`，`integration_test.rs` 对照 Go `integration_test.go`，`randomized_integration_test.rs` 对照 Go 随机化集成测试，`storage_internal_test.rs` 对照 Go 同名内部测试；Rust 另外有 `parity_test.rs`、`progress_test.rs`、`storage_test.rs` 用于公开契约、首错取消与存储解析的补充验证。

## 扩展指南

- 增加新的公共计算器类型或函数时，优先放入 `calculator.rs`；注意 `pub use calculator::*` 会自动扩大 crate 根 API，应同步检查 `service`、`config` 与公开契约测试 `parity_test.rs`。
- 新增轮次算法步骤应放入 `progress.rs` 的 `impl Calculator`，并同步 `progress_test.rs` 或 `checkpoint_calculator_test.rs`；不要把实现写进门面文件。
- 新增 backupmeta 格式、扫描规则或存储限制应放入 `storage.rs`，并同步 `storage_test.rs`、`storage_internal_test.rs` 及相应 Go 测试。格式兼容改动需特别检查文件名字典序、`StartAfter`、store ID 一致性和历史 JSON 字段。
- 新增生产子模块时需在此声明 `pub mod` 或私有 `mod`，同时更新 `Cargo.toml` 依赖（如有）与 Bazel 元数据；是否公开应按真正的跨 crate 调用需求决定，不能仅为测试方便扩大可见性。
- 新增测试必须继续放在独立 `*_test.rs` 文件，通过 `#[cfg(test)] #[path = "..."] mod ...;` 挂载。不要把测试内嵌进 `lib.rs` 或生产实现文件。
- 修改观察事件或状态字段时，应同步 `status.rs`、`metrics.rs`、`parity_test.rs` 和 Go 对照；修改并发策略时应保留首错取消、有界读取和观察者不阻塞等约束，并评估对象存储读放大与轮询延迟。

## 验证依据

- RustCodeGraph 索引状态：11,467 个文件、7,032 个 Rust 文件；读取了 `lib.rs` 全部 73 行，并定位 `calculator.rs::Calculator`、`CalculatorDeps`、`NewCalculator` 及其实现流程。
- 源码与配置：`br/pkg/stream/crr/internal/checkpoint/{lib.rs,calculator.rs,progress.rs,storage.rs,Cargo.toml}`。
- 直接调用与生命周期：`br/pkg/stream/crr/service/{service.rs,status.rs,metrics.rs,Cargo.toml}`，以及 `br/pkg/stream/crr/config/{config.rs,Cargo.toml}`。
- Go 对照：`br/pkg/stream/crr/internal/checkpoint/{doc.go,calculator.go,progress.go,storage.go}` 和同目录 `checkpoint_calculator_test.go`、`integration_test.go`、`randomized_integration_test.go`、`storage_internal_test.go`。
- Rust 独立测试：`parity_test.rs`、`checkpoint_calculator_test.rs`、`integration_test.rs`、`progress_test.rs`、`storage_test.rs`、`randomized_integration_test.rs`、`storage_internal_test.rs`。这些测试覆盖公开契约、构造校验、上游不变、存活/移除 store、对象同步错误、持久状态恢复、读取并发限制、取消传播、增量扫描游标与随机时序。
- 本任务是纯文档分析，按计划未运行 Cargo。交付前使用任务指定命令验证目标文档存在且恰含 11 个固定二级章节，并人工复核文档中的路径、导出和调用边均来自上述直接证据。
