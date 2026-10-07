# [`br/pkg/checksum/lib.rs`](./lib.rs)

## 文件定位

`br/pkg/checksum/lib.rs` 是 Cargo 包 `astersql-br-pkg-checksum` 的 crate 根，而不是校验和算法的实现文件。`br/pkg/checksum/Cargo.toml` 通过 `[lib] path = "lib.rs"` 指向它，仓库根 `Cargo.toml` 又把 `br/pkg/checksum` 列为 workspace member；因此编译这个 Rust library 时，模块树从这里开始。

该文件把生产模块 `stubs.rs`、`executor.rs` 纳入 crate，并在测试构建中额外挂载三份独立测试模块。它最后通过 `pub use executor::*` 和 `pub use stubs::*` 提供扁平的 crate API，使调用方可以从 `astersql_br_pkg_checksum` 根命名空间取得执行器和适配类型。当前仓库搜索没有发现其他 Rust crate 在 Cargo manifest 中依赖 `astersql-br-pkg-checksum`，也没有发现生产 Rust 文件引用 `astersql_br_pkg_checksum`；现阶段可确认的消费面主要是 crate 自身测试，不能据此宣称已经接入 Rust BR 主链。

## 核心职责

该文件只负责装配和导出，不直接计算 checksum：

1. `pub mod stubs` 先声明本地类型、序列化、请求构造、客户端 trait、上下文和重试适配层。
2. `pub mod executor` 再声明实际请求展开与执行逻辑；`executor.rs` 通过 `crate::stubs` 使用前述适配层。
3. 三个 `#[cfg(test)]` 模块只在测试配置下参与编译，保持测试逻辑与生产源文件分离。
4. 两条 glob re-export 把两个公开模块的公开项提升到 crate 根，模拟 Go 包级符号的使用方式。

文件级 `#![allow(...)]` 放宽了机械移植代码常见的命名、未使用项和 Clippy 警告。这是整个 crate 的 lint 边界，不是运行时容错机制，也不意味着被抑制的代码已经获得真实 TiKV 集成验证。

## 主要符号

- `pub mod stubs`（`lib.rs:19-20`）：公开 `stubs.rs` 模块。主要公共面包括 `TableInfo`、`MetaTable`、`Request`、`ChecksumRequest`、`ChecksumResponse`、`Context`、`Client`、`Response`、`WithRetry` 和 `RequestBuilder`。这些是当前 crate 的本地适配/桩，不等同于 Go 版本依赖的完整 `kv`、`distsql`、`tipb` 和 metadata 实现。
- `pub mod executor`（`lib.rs:22-24`）：公开 `executor.rs` 模块。核心 API 是 `NewExecutorBuilder(TableInfo, u64) -> ExecutorBuilder`、链式 `ExecutorBuilder` setter、`Build() -> Result<Executor>`，以及 `Executor::{Len, Each, RawRequests, Execute}`。
- `mod parity_test`、`mod executor_nokit_test`、`mod executor_test`（`lib.rs:26-36`）：三个私有测试模块，分别覆盖 Go/Rust 契约矩阵、builder 的 request source，以及执行器正常/取消/聚合行为。
- `pub use executor::*` 与 `pub use stubs::*`（`lib.rs:38-40`）：crate 根的公开兼容面。新增同名公共项时可能产生 glob re-export 名称冲突，扩展时必须检查根命名空间。

`lib.rs` 自身没有常量、struct、enum、trait、函数或 `impl`；上述模块声明和重导出就是它的全部可观察职责。

## 执行流程

从 crate 根可追踪到的实际流程如下：

1. 调用方从根重导出取得 `NewExecutorBuilder` 以及 `TableInfo` 等输入类型。
2. `NewExecutorBuilder` 在 `executor.rs:65` 保存表元信息和快照 TS，并采用 `DefDistSQLScanConcurrency`；可选 setter 设置旧表、keyspace、并发、退避权重、资源组和 `RequestSource`。
3. `ExecutorBuilder::Build` 调用 `buildChecksumRequest`。后者为主表及每个分区生成一条 table 请求，并为每个 `StatePublic` 索引生成 index 请求；提供旧表时同时构造 record/index prefix rewrite 规则。
4. `Executor::Execute` 逐请求调用 `WithRetry` 和 `sendChecksumRequest`，经 `Client`/`Response` 抽象消费分片；每条请求成功后调用进度回调。
5. `updateChecksumResponse` 对 `Checksum` 做 XOR，对 `TotalKvs`、`TotalBytes` 做 wrapping addition；所有请求完成后，`checkContextDone` 再检查取消状态。

根模块不会主动创建 executor、线程或客户端，也没有初始化副作用。只有下游显式调用重导出的 API 时，上述流程才会发生。

## 数据与状态

`lib.rs` 没有全局可变状态，也不持有请求生命周期。数据状态位于它装配的子模块中：

- `ExecutorBuilder` 持有一次构建所需的 `TableInfo`、TS、可选旧表、并发、backoff、keyspace、资源组和请求来源。
- `Executor` 持有已经展开的 `Vec<Request>` 与 `backoff_weight`；请求数量遵循“主表及各分区 ×（一条 table 请求 + public 索引请求）”。
- `ChecksumResponse` 的三个聚合字段分别遵循 XOR、计数累加和字节累加语义；计数字段显式使用 wrapping arithmetic 对齐 Go `uint64`。
- `stubs.rs` 中用于测试的 failpoint/backoff 开关包含进程级原子状态；它们由测试控制，不由 `lib.rs` 初始化。

`Cargo.toml` 的 `[dependencies]` 为空，说明当前 crate 的模型、编码、请求和重试边界全部由本地 `stubs.rs` 承担。这是理解当前迁移状态的重要限制：接口形状与 Go 对齐不等于已经连接真实 DistSQL/TiKV 实现。

## 依赖与调用关系

编译依赖方向是 `lib.rs -> {stubs.rs, executor.rs}`，而 `executor.rs -> crate::stubs::*`。测试配置再增加 `lib.rs -> {parity_test.rs, executor_nokit_test.rs, executor_test.rs}`；测试通过 `crate::executor` 和 `crate::stubs` 访问内部模块，也可验证根重导出的公开契约。

RustCodeGraph 对 `NewExecutorBuilder` 的调用轨迹列出 `parity_test.rs` 中的整数句柄范围、索引/common-handle 范围、Close 错误覆盖、索引名严格匹配和公开契约测试。仓库文本搜索没有找到生产 Rust 调用方。Go 侧则有真实生产消费者，例如 `br/pkg/backup/schema.go`、`br/pkg/restore/snap_client/client.go`、`br/pkg/restore/log_client/client.go`、`br/pkg/task/operator/checksum_table.go` 与 `pkg/ingestor/ingestctrl/checksum.go`；这些导入的是 Go 包 `github.com/pingcap/tidb/br/pkg/checksum`，不是当前 Rust crate。

Go 的 `br/pkg/checksum/BUILD.bazel` 描述真实 Go 依赖（`distsql`、`kv`、`model`、`tablecodec`、`ranger`、`tipb` 等）。Rust `Cargo.toml` 没有对应外部依赖，进一步证明 Rust 实现目前是自包含移植边界，文档不把 Go 调用图误写成 Rust 已接线事实。

## 错误处理与边界

`lib.rs` 不产生错误；错误语义来自重导出的子模块：

- `Build` 将请求构造错误以 `Error::Trace` 传播。旧表缺少同名分区时返回错误；旧表缺少与新表 `CIStr` 完全相等的 public 索引时会 `panic`，对齐 Go 的 `log.Panic`。
- 非 `StatePublic` 索引会被跳过；common handle、整数 handle 和索引分别使用不同的全范围编码边界。
- `sendChecksumRequest` 对发送、读取、反序列化和关闭失败进行传播；若读取/反序列化与 `Close` 同时失败，Close 错误覆盖先前错误，以对齐 Go named-return defer。
- `Execute` 在单请求失败时停止后续请求，并通过 checksum backoff 策略重试；所有请求完成后若上下文已取消，则返回带 `context is cancelled by other error` 注释的错误。
- `RawRequests` 只反序列化已构建请求，不发送 DistSQL 请求。

根级 glob re-export 还有一个 API 边界：`executor` 与 `stubs` 若新增同名 public symbol，可能令根路径变得歧义或引发编译错误，因此不能把重导出视为无成本扩展。

## 并发与资源生命周期

`lib.rs` 没有线程、锁、异步任务、channel 或资源句柄。`Executor::Execute` 当前是按请求顺序执行；请求内的 `Concurrency` 字段只传递给 DistSQL 客户端抽象，不能解释成 Rust 本地并行调度。

每次发送得到的 `Response` 必须在成功、读取错误或反序列化错误路径上调用 `Close`。`parity_test.rs` 的 `close_error_overrides_response_read_error` 和综合契约测试分别验证 Close 错误优先级以及成功请求恰好关闭一次。`Context` 取消在重试/发送路径中传播，并在全部聚合后再次检查。

测试夹具使用 `Mutex`、`AtomicUsize` 和 `Arc` 记录响应模板、发送次数与 Close 次数；这些同步对象属于独立测试文件，不进入生产 crate 状态。`stubs.rs` 的测试注入开关为全局原子量，新增并行测试时需要避免跨测试污染，并在用例结束后恢复开关。

## 与 Go 版本的对应关系

Rust crate metadata 明确记录 `go-package = "br/pkg/checksum"`，实际逻辑对应 `br/pkg/checksum/executor.go`。主要对齐点包括 builder 字段和链式 setter、表/分区/public 索引展开、Crc64_Xor、低优先级和不填 block cache、keyspace/prefix rewrite、重试、进度回调，以及 XOR/累加聚合。

Rust 测试与 Go 测试保持独立文件对应：

- `executor_nokit_test.rs` 对照 `executor_nokit_test.go`，检查 `RequestSource` setter。
- `executor_test.rs` 对照 `executor_test.go`，覆盖聚合溢出语义、context 取消、builder 与 Execute 主路径。
- `parity_test.rs` 补充编码边界、Close 错误覆盖、`CIStr` 严格匹配、父子 context 和 protobuf 截断字段等契约。
- `main_test.go` 仅为 Go 测试提供 `goleak`/test setup；Rust 根没有对应运行时入口。

差异在于 Go 包直接依赖真实 TiDB/TiKV 类型和 DistSQL 实现，而 Rust crate 使用 `stubs.rs` 的本地替代类型，且尚未发现生产 Rust 消费者。因此当前 Rust 文件是结构和行为契约移植，不应描述为 Go 包的完整生产替换。

## 扩展指南

若扩展 checksum 功能，应按职责选择接入点：

- 新的请求展开、聚合或执行策略放在 `executor.rs`，并保持 `lib.rs` 仅负责模块装配；只有需要根级公开 API 时才调整 re-export。
- 新的 TiDB/TiKV 边界类型或适配行为放在 `stubs.rs`，但若要真正接入外部 Rust 客户端，必须按仓库规则在独立上游仓库移植、发布 tag，再由 Cargo Git tag 依赖接入，不能把依赖复制进本目录或用本地 `[patch]`。
- 新行为需同步独立 Rust 测试文件，并与 `executor.go`、`executor_test.go` 或 `executor_nokit_test.go` 的语义逐项核对；不要把单元测试嵌入生产 `lib.rs`/`executor.rs`。
- 新增模块时在 `lib.rs` 明确决定是否 public、是否测试专用、是否根级重导出；避免两个 glob 导出源出现同名符号。
- 改动请求顺序、range 编码、rewrite 规则、Close 顺序或 checksum 聚合方式都具有兼容性/正确性风险；提升本地并发或缓存请求还会改变内存与在线流量压力。

当前文件已经带有 `// Copyright 2026 AsterSQL.`。纯文档扩展不应修改 Rust/Go/Cargo 行为，也不应删除现有 PingCAP 版权声明。

## 验证依据

本说明基于以下直接证据：

- `br/pkg/checksum/lib.rs`：crate 属性、两个生产模块、三个 `cfg(test)` 测试模块与两条根级重导出。
- `br/pkg/checksum/Cargo.toml`：package 名、`[lib]` 路径、Go package metadata、空依赖表；根 `Cargo.toml`：workspace member。
- `br/pkg/checksum/executor.rs`：`ExecutorBuilder`、`NewExecutorBuilder`、`buildChecksumRequest`、`sendChecksumRequest`、`updateChecksumResponse`、`Executor::Execute` 和 `checkContextDone`。
- `br/pkg/checksum/stubs.rs`：本地数据模型、protobuf 编解码、key/range 编码、`Client`/`Response`、请求 builder、context 与重试策略。
- Go 对照：`br/pkg/checksum/executor.go`、`executor_nokit_test.go`、`executor_test.go`、`main_test.go`、`BUILD.bazel`。
- Rust 独立测试：`br/pkg/checksum/executor_nokit_test.rs`、`executor_test.rs`、`parity_test.rs`。
- RustCodeGraph：`status` 显示索引包含 7032 个 Rust 文件；`files --filter br/pkg/checksum` 列出目标、实现、桩和测试；`node --file br/pkg/checksum/lib.rs` 核对 40 行完整源码；`query/node NewExecutorBuilder` 核对定义及 `parity_test.rs` 调用轨迹。
- `rg`：核对 workspace/Cargo/Bazel 接线、Go 生产消费者、Rust crate 名称引用以及测试函数。未发现生产 Rust 调用方是当前仓库搜索结论，不推断仓库外使用情况。

本任务为纯文档分析，按计划不运行 Cargo。结构验收以目标文件存在且恰含规定的十一个二级标题为准。
