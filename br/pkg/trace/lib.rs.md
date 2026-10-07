# `br/pkg/trace/lib.rs`

## 文件定位

`br/pkg/trace/lib.rs` 是 Cargo 包 `astersql-br-pkg-trace` 的库入口，而不是追踪算法的实现文件。`br/pkg/trace/Cargo.toml` 通过 `[lib] path = "lib.rs"` 将它指定为 crate root，并用 `package.metadata.porting.go-package = "br/pkg/trace"` 标明对应的 Go 包。根工作区 `Cargo.toml` 将 `br/pkg/trace` 列为 workspace member；BR 命令 crate 则在 `br/cmd/br/Cargo.toml` 中以路径依赖 `astersql-br-pkg-trace` 接入它。

本文件的职责边界很窄：第 17—20 行声明 `tracing.rs` 并公开重导出其接口，第 22—32 行只在测试构建中挂载三个独立测试模块。真正的 span、内存存储、树形输出和文件生命周期都位于同目录的 `tracing.rs`。

## 核心职责

1. 用 `#[path = "tracing.rs"] pub mod tracing;` 把非标准目录布局下的实现文件注册为公开模块。
2. 用 `pub use tracing::*;` 建立兼容门面，使调用方可以从 crate 根直接使用 `TracerStartSpan`、`TracerFinishSpan`、`Context`、`MemoryStore` 等公开项，而不必写 `tracing::...`。
3. 用三个 `#[cfg(test)] #[path = "..."] mod ...;` 声明独立测试模块，遵守 Rust 源码与测试逻辑分文件的仓库约定。
4. 在 crate 根统一允许迁移代码中暂时存在的 `dead_code`、Go 风格命名和未使用项。该 lint 放宽作用于整个 crate，但不改变任何运行时行为。

它不是另一个 tracing 后端，也不自行初始化全局 tracer。`tracing.rs` 明确说明当前实现只复刻 BR 使用的 appdash/OpenTracing 子集，并非生产级通用 tracing 后端。

## 主要符号

- `pub mod tracing`：公开模块声明，源码通过 `#[path = "tracing.rs"]` 显式定位。下游既可以访问 `astersql_br_pkg_trace::tracing::TracerStartSpan`，也可以使用根级重导出。
- `pub use tracing::*`：通配重导出 `tracing.rs` 的全部公开符号。当前关键 API 包括 `Context`、`MemoryStore`、`Queryer`、`Tracer`、`Span`、`Trace`、`Tabby`、`timestampTraceFileName`、`TracerStartSpan`、`TracerFinishSpan`、`dfsTree` 和 `format_go_duration`。
- `mod parity_test`：测试构建时挂载 `parity_test.rs`，覆盖 Go/Rust 公开契约、文件命名、空 trace、不可写路径和资源关闭。
- `mod tracing_serial_test`：测试构建时挂载 `tracing_serial_test.rs`，对应 Go `tracing_serial_test.go::TestSpan` 的真实文件输出路径。
- `mod tracing_test`：测试构建时挂载 `tracing_test.rs`，覆盖时长格式、Unicode 列宽、子节点排序、时区列以及缺失活动 span 的 panic 语义。
- crate 级 `#![allow(...)]`：允许 `non_snake_case` 等六类 lint，以容纳与 Go 导出名一致的 API；它不是 feature gate，也没有条件编译分支。

本文件没有常量、结构体、trait、函数或 `impl` 的本地定义；这些符号均来自 `tracing.rs` 的公开重导出。

## 执行流程

运行时加载此 crate 时，`lib.rs` 只完成静态模块装配，不执行初始化代码。实际主链为：

1. BR 命令层读取配置中的 `EnableOpenTracing`。例如 `br/cmd/br/backup.rs`、`restore.rs`、`stream.rs` 和 `abort.rs` 调用 `br/cmd/br/cmd.rs::with_tracing`。
2. tracing 关闭时，`with_tracing` 直接执行传入闭包；本 crate 不参与。
3. tracing 开启时，`with_tracing` 通过本文件的根级重导出调用 `TracerStartSpan(Context::Background())`，取得含根 span 的上下文和共享 `MemoryStore`。
4. 业务闭包运行后，`with_tracing` 用 `TracerStartSpan` 返回的 tracing 上下文调用 `TracerFinishSpan`。后者查询已完成的子 span、结束根 span，并在存在已收集 trace 时把第一棵树写入临时文件。
5. 测试构建额外编译三个 `cfg(test)` 模块；普通库构建不会包含这些模块。

需要注意 `br/cmd/br/cmd.rs::with_tracing` 当前把原业务 `ctx` 传给闭包，而把新建的 `next_ctx` 只用于结束追踪；这是直接调用证据所示的现状，本门面不改写或协调这两个上下文。

## 数据与状态

`lib.rs` 自身没有可变数据或运行时状态。它暴露的状态由 `tracing.rs` 管理：

- `Context` 持有可选的 `Arc<Span>`；`ContextWithSpan` 和 `SpanFromContext` 分别写入、读取活动 span。
- `MemoryStore` 以 `Mutex<Vec<CollectedSpan>>` 保存已完成 span，并在查询时按 `parent_id` 重建 trace 森林。
- `Tracer` 用 `AtomicU64` 分配单调递增的 span id；`Span` 用 `AtomicBool` 保证 `Finish` 只收集一次。
- trace 文件名测试覆盖保存在 thread-local `TRACE_FILE_NAME_OVERRIDE` 中；默认路径由 `timestampTraceFileName` 生成到系统临时目录。
- `Tabby` 在内存中缓冲三列表格行，`Print` 时计算字符宽度、写入并刷新输出目标。

由于 `pub use tracing::*` 暴露上述类型，调整实现文件的 `pub` 集合会直接改变 crate 根 API；门面没有额外封装来隔离这种变化。

## 依赖与调用关系

上游直接证据：

- `br/cmd/br/Cargo.toml` 依赖本 crate。
- `br/cmd/br/cmd.rs::with_tracing` 直接调用根级 `TracerStartSpan` 和 `TracerFinishSpan`；RustCodeGraph 显示该包装器由备份、恢复、流式任务等命令路径调用。
- `br/cmd/br/cmd.rs` 还直接调用根级 `timestampTraceFileName`。
- `br/cmd/br/parity_test.rs::contract_resource_cleanup` 直接验证开始/结束接口及 `with_tracing` 对 tracing 上下文的使用。

下游直接证据：

- `TracerStartSpan` 依次调用 `MemoryStore::NewMemoryStore`、`Tracer::NewTracer`、`Tracer::StartSpan("trace")` 和 `ContextWithSpan`。
- `TracerFinishSpan` 依次使用 `SpanFromContext`、`Queryer::Traces`、`Span::Finish`、文件创建、`Tabby`、`dfsTree`；`dfsTree` 再使用 `Trace::TimespanEvent` 并按开始时间原地排序子节点。
- 实现只依赖 Rust 标准库；`br/pkg/trace/Cargo.toml` 没有 `[dependencies]`。Go 对照则依赖 appdash、opentracing、tabby、PingCAP log 和 zap。

RustCodeGraph 对 `lib.rs` 仅识别到一个模块级符号，并显示其静态“used by”信息很少；真实 API 调用边落在被重导出的 `tracing.rs` 符号上，因此不能据 `lib.rs` 自身的图边误判此 crate 未被使用。

## 错误处理与边界

门面层不产生 `Result`、不捕获错误，也不执行 I/O。其公开实现的关键边界由 `tracing.rs` 定义：

- `Queryer::Traces` 失败时，`TracerFinishSpan` 写 stderr 后返回，不创建文件。
- 查询成功但上下文没有活动 span 时，`expect("TracerFinishSpan requires an active span")` 会 panic；`tracing_test.rs::finish_without_active_span_matches_go_nil_span_panic` 固化了与 Go nil span 调用一致的行为。
- 查询发生在根 span `Finish` 之前，所以只有根 span、没有先完成子 span 时，查询结果为空，函数结束根 span 后直接返回且不落盘；`parity_test.rs` 验证该边界。
- trace 文件创建失败仅记录错误并返回；已有 span 数据仍只存在内存中，不向调用方传播错误。
- `Tabby::Print` 忽略 `write_all` 与 `flush` 的错误；调用方无法从当前 API 判断输出是否完整。
- `MemoryStore` 的互斥锁中毒通过 `into_inner` 恢复；重复调用 `Span::Finish` 被原子状态静默忽略。
- 非 Unix 平台的本地 UTC 偏移固定为零；路径转换使用 lossy UTF-8。这些均是实现限制，不由 `lib.rs` 补偿。

## 并发与资源生命周期

`lib.rs` 不创建线程、任务、锁、通道或事务。通过它导出的实现具有以下生命周期：

- `Arc<MemoryStore>` 在 tracer、span 和调用者间共享；内部 `Mutex` 保护并发收集与查询。
- span id 用 `AtomicU64(Relaxed)` 分配，完成状态用 `AtomicBool` 的顺序一致 CAS 控制，保证同一 span 只进入 store 一次。
- 文件句柄由 `TracerFinishSpan` 的局部 `File` 持有，函数离开作用域时自动关闭；测试在结束后重新读取文件验证资源已释放。
- 测试文件名覆盖是 thread-local，而非 Go 的进程级包变量，避免 Rust 并行 libtest 相互覆盖。调用测试 hook 后仍应执行 `set_get_trace_file_name_for_test(None)`，避免污染同一测试线程的后续用例。
- `cfg(test)` 模块只进入测试二进制；生产库不会携带 sleep、临时目录创建或测试 writer。

## 与 Go 版本的对应关系

Go 包没有单独的 `lib` 门面文件；Rust `lib.rs` 是把 Go 包映射成 Cargo crate 所需的装配层。`Cargo.toml` 的 `go-package = "br/pkg/trace"` 明确记录这种归属。

实现对应关系为：Rust `tracing.rs` 对照 Go `tracing.go`，Rust `tracing_serial_test.rs` 对照 Go `tracing_serial_test.go`，而 `parity_test.rs` 与 `tracing_test.rs` 增加公开契约和对抗性回归覆盖。核心语义保持一致：创建内存 store 和根 span、从上下文建立子 span、先查询已完成 span 再结束根 span、仅输出第一棵 trace、按开始时间排序、以树形三列文本写入临时文件。

已验证的实现差异包括：Rust 用本地标准库类型复刻 appdash/OpenTracing 的 BR 子集；Go 的 `getTraceFileName` 是进程级变量，Rust 测试覆盖为 thread-local；Go 使用日志库，Rust 当前写 stderr；Go `context.Context` 被简化为只携带 span 的 `Context`；Rust 非 Unix 时区偏移回退为 UTC。它们是当前移植状态，不应在文档中描述成完整的 Go 依赖替代品。

Go `main_test.go` 还负责公共测试初始化和 goroutine 泄漏检查；Rust crate root 没有等价的自定义测试 runner，三个独立测试模块只覆盖本 crate 的同步资源与行为契约。

## 扩展指南

- 新增 tracing 行为应在 `tracing.rs` 实现，并优先保持 Go `tracing.go` 的流程、失败语义和输出格式；只有需要改变公开表面或测试挂载时才修改 `lib.rs`。
- 新公开项会因 `pub use tracing::*` 自动出现在 crate 根。若未来要收紧 API，需先搜索 `astersql_br_pkg_trace::...` 和 `astersql_br_pkg_trace::tracing::...` 两种调用形式，再将通配重导出改为显式清单；这是兼容性变更。
- 回归测试必须放在独立文件。Go 主路径对应测试放入 `tracing_serial_test.rs`，跨语言公开契约放入 `parity_test.rs`，Rust 特有边界与格式回归放入 `tracing_test.rs`；新增文件时再用 `#[cfg(test)] #[path = ...] mod ...` 挂载。
- 若增加外部 crate、feature 或平台实现，需要同步 `br/pkg/trace/Cargo.toml`，并检查 workspace 与调用 crate 的依赖锁定；当前无外部 Rust 依赖。
- 修改 `TracerFinishSpan` 的“先查询、后结束根 span”顺序、首树选择、写失败不传播或无 span panic，都会改变已由测试和 Go 源码确认的兼容契约。
- 性能敏感点在 store 全量加锁/克隆、树重建与子节点排序，而不在门面。扩展高流量采集前应补独立压力或并发测试，不能把当前轻量桩当作生产级 tracing 系统。

## 验证依据

- RustCodeGraph：`status` 确认索引包含 7,032 个 Rust 文件；`files --filter br/pkg/trace` 确认本目录的源与测试集合；`node --file br/pkg/trace/lib.rs` 确认 32 行 crate root、模块声明和测试挂载。
- RustCodeGraph：`explore "br/pkg/trace/lib.rs crate root module tracing public exports"` 给出 `TracerStartSpan`、`TracerFinishSpan`、`dfsTree`、`MemoryStore` 等调用关系；`node --file br/pkg/trace/tracing.rs` 核对实际实现和资源生命周期；`node` 分别读取 `parity_test.rs`、`tracing_serial_test.rs`、`tracing_test.rs` 核对边界断言。
- Cargo/入口：读取 `br/pkg/trace/Cargo.toml`、根 `Cargo.toml`、`br/cmd/br/Cargo.toml` 和 `br/cmd/br/cmd.rs::with_tracing`，确认 crate 边界、workspace 成员、唯一直接 Rust 依赖方及命令包装流程。
- Go 对照：读取 `br/pkg/trace/tracing.go`、`tracing_serial_test.go`、`main_test.go`，核对 API、树形输出、测试 hook 和 Go 测试初始化；用 `rg` 搜索 BR 内 `EnableOpenTracing`、`TracerStartSpan`、`TracerFinishSpan` 与 crate 名确认调用入口。
- Rust 测试：`br/pkg/trace/parity_test.rs`、`tracing_serial_test.rs`、`tracing_test.rs`。本任务是纯文档分析，按计划不运行 Cargo；结论来自静态源码、索引调用边和现有测试断言。
