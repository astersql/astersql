# `pkg/ingestor/ingestctrl/local_windows.rs`

## 文件定位

`local_windows.rs` 属于 `astersql-ingestor-ingestctrl` crate；该 crate 由 `pkg/ingestor/ingestctrl/Cargo.toml` 定义，库入口是同目录的 `lib.rs`。`lib.rs` 通过 `pub mod local_windows` 公开本模块，并将独立测试放在 `local_windows_test.rs`，没有把测试嵌入生产源文件。

本文件是 Windows 平台的进程打开文件数限制（rlimit）兼容桩。它保留与 Go `local_windows.go` 相同的 API 形状和返回语义，但不访问 Windows API，也不读取或修改真实系统资源限制。尤其需要注意：当前 Rust 模块声明和函数本身没有 `#[cfg(windows)]`；同时 `local.rs::NewBackend` 直接调用 `crate::local_unix::VerifyRLimit`，没有根据平台选择本模块。因此本文描述的是已经实现并由单元测试覆盖的 Windows 契约，不表示该契约已经接入 Rust local backend 的生产创建路径。

## 核心职责

- 用 `RlimT` 为 Windows 分支提供与 Go `uint64` 对应的资源限制值类型。
- 用 `GetSystemRLimit` 返回一个代表“足够大/近似无限”的固定占位值 `i32::MAX as u64`，而不是查询操作系统。
- 用 `VerifyRLimit` 明确拒绝 Windows 上的 local backend 前置校验，并给出关闭 requirements 检查或更换平台的提示。
- 维持与 `local_windows.go` 的值、错误文本和“忽略估算值”行为一致，供后续平台接线复用。

## 主要符号

- `pub type RlimT = u64`：Windows rlimit 值的公开类型别名。它与 Go 的 `type RlimT = uint64` 数值域一致，也使 `VerifyRLimit` 能接受从配置估算出的文件数。
- `pub fn GetSystemRLimit() -> Result<RlimT>`：始终返回 `Ok(i32::MAX as u64)`。返回 `Result` 是为了保持平台实现的统一调用形状；当前函数体没有失败分支。
- `pub fn VerifyRLimit(_estimateMaxFiles: RlimT) -> Result<()>`：参数名前缀 `_` 明确表示当前实现不使用估算值。函数始终构造 `Error::InvalidData`，错误文本与 Go 版本一致。
- `crate::{Error, Result}`：模块唯一的生产依赖。`Error::InvalidData(String)` 和 crate 级 `Result<T>` 将平台桩纳入 ingestctrl 的统一错误模型。

文件没有常量、结构体、trait、`impl`、锁、异步任务或显式条件编译项。

## 执行流程

调用 `GetSystemRLimit` 时，流程只有一次无条件返回：把 `i32::MAX` 转为 `u64`，包装为 `Ok`。该值是 Go 兼容占位值，不是 Windows 可打开文件数的测量结果，也不应作为容量探测依据。

调用 `VerifyRLimit(estimateMaxFiles)` 时，传入值不会参与判断；无论它是 `0`、普通配置值还是 `u64::MAX`，函数都会立即返回相同的 `Error::InvalidData`。`local_windows_test.rs::windows_rlimit_matches_go_contract` 用 `0` 与 `u64::MAX` 两个端点验证了这一不变量。

预期的平台主链应在创建 local backend 的 requirements 检查阶段选择对应平台实现；但当前 `local.rs::NewBackend` 的实际边是 `crate::local_unix::VerifyRLimit(config.max_open_files as u64)`，RustCodeGraph 也只把 `local_windows.rs` 标为由 `local_windows_test.rs` 使用。因此当前 Windows 函数的可观察执行入口是直接调用和单元测试，生产平台分派仍未在本文件或相邻入口中实现。

## 数据与状态

模块没有持久状态或可变全局数据。`RlimT` 只是 `u64` 的别名，不引入新类型校验；所有 `u64` 值都可以传给 `VerifyRLimit`。

`GetSystemRLimit` 的 `2_147_483_647` 来自 `i32::MAX`，与 Go 的 `math.MaxInt32` 对齐。这个值的语义是 Windows 跳过真实 rlimit 验证时使用的高占位上限，不是 Windows 内核状态快照。`VerifyRLimit` 只分配错误字符串并把它放入 `Error::InvalidData`，不修改参数、配置或外部资源。

## 依赖与调用关系

- 模块装配：`lib.rs` 公开 `local_windows`，并在 `#[cfg(test)]` 下装配 `local_windows_test`。
- 上游证据：RustCodeGraph 对文件的索引报告 5 个符号，并标记它由 `local_windows_test.rs` 使用；仓库搜索没有发现生产 Rust 代码调用 `local_windows::GetSystemRLimit` 或 `local_windows::VerifyRLimit`。
- 生产主链的相邻事实：`local.rs::NewBackend` 在调整 `BackendConfig` 后调用 `local_unix::VerifyRLimit`，成功后才创建 `EngineManager` 和 `Backend`。这说明本模块目前没有替代该 Unix 调用边。
- 下游依赖：两个函数仅依赖 crate 根的 `Result` 和 `Error::InvalidData`，没有调用操作系统、外部 crate 或其他 ingestctrl 子模块。
- crate 边界：`Cargo.toml` 的 `[package.metadata.porting]` 将该 crate 对应到 Go 包 `pkg/ingestor/ingestctrl`；清单虽包含 `cfg(windows)` 依赖组，但本文件没有使用其中任何依赖。

## 错误处理与边界

`GetSystemRLimit` 当前不可能返回 `Err`；保留 `Result` 只为平台 API 一致性。调用者不能从它判断真实 Windows 文件句柄上限。

`VerifyRLimit` 没有成功路径，也不区分估算值是否合理。其错误分类是 `InvalidData`，错误消息固定为：`local-backend is not tested on Windows. Run with --check-requirements=false to disable this check, but you are on your own risk`。这与 Go `errors.New(...)` 的文本相同，但 Rust 多了一层 crate 统一错误枚举分类。

边界上，端点 `0` 和 `u64::MAX` 都必须失败且错误完全相同。若未来改变错误类别、文本或允许某些估算值通过，需要同步评估 CLI/上层是否依赖该提示，并更新独立测试；不能仅让测试通过而把桩误写成真实 Windows rlimit 实现。

## 并发与资源生命周期

本模块是无状态的同步纯逻辑：没有锁、原子变量、线程、future、通道、文件描述符或 Windows handle。两个函数可被并发调用，彼此不共享或修改状态。

它也不拥有资源生命周期：`GetSystemRLimit` 不打开句柄，`VerifyRLimit` 不尝试提升限制，返回错误后没有清理动作。local backend、EngineManager 以及导入 worker 的创建和关闭都由 `local.rs` 等其他模块负责，并且当前会在进入这些生命周期前经过 Unix 校验调用边。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/ingestor/ingestctrl/local_windows.go`，它用 `//go:build windows` 限定平台。对应关系如下：

- Go `type RlimT = uint64` 对应 Rust `pub type RlimT = u64`。
- Go `GetSystemRLimit` 返回 `(math.MaxInt32, nil)`；Rust 返回 `Ok(i32::MAX as u64)`，数值和成功语义一致。
- Go `VerifyRLimit(estimateMaxFiles uint64)` 不使用参数并始终 `errors.New(...)`；Rust同样忽略参数并始终返回同文本的 `Error::InvalidData`。
- Go 文件有 Windows build tag；Rust 文件及 `lib.rs` 的模块声明没有等价 `cfg(windows)`。此外，当前 Rust `NewBackend` 使用 `local_unix` 而非平台分派。这是实际迁移/接线差异，不应在文档中解释成已经完成的平台等价。

仓库搜索未找到专门调用这两个 Go 函数的 Go 测试；Rust 的契约回归由 `local_windows_test.rs::windows_rlimit_matches_go_contract` 提供，覆盖占位值、错误文本和参数无关性。

## 扩展指南

若只是调整 Windows 拒绝策略或提示文本，修改 `VerifyRLimit`，并同步 `local_windows_test.rs` 以及 Go 对照实现/测试意图；需检查上层是否展示或匹配该消息。若改变占位上限，修改 `GetSystemRLimit` 并保留 Rust/Go 数值一致，明确新值仍是占位还是已经来自系统查询。

若要真正接入 Windows 平台，修改点不应局限于本文件：需要在 `lib.rs` 或 `local.rs::NewBackend` 附近增加清晰的平台选择，使 Windows 构建调用本模块、Unix 构建调用 `local_unix`，并确认 Unix FFI 不会进入 Windows 编译/链接路径。应把平台分派测试放在独立测试文件中；真实 Windows 行为还需要 Windows 环境上的构建与运行验证。

若要实现真实 Windows 资源检查，应先定义可验证的 Windows 等价资源和错误契约，再引入最小 OS 依赖、处理系统调用失败，并为零值、极大值、系统调用失败和并发调用增加独立测试。不要把 Unix `RLIMIT_NOFILE` 的含义直接套到 Windows handle 模型，也不要在生产源文件内嵌测试。

## 验证依据

- `pkg/ingestor/ingestctrl/local_windows.rs`：完整读取；确认 `RlimT`、`GetSystemRLimit`、`VerifyRLimit`、统一错误类型以及无条件编译/无状态实现。
- `pkg/ingestor/ingestctrl/lib.rs`：确认公开模块声明、独立测试装配和 `Error::InvalidData` 的 crate 级含义。
- `pkg/ingestor/ingestctrl/local.rs`：确认 `NewBackend` 当前实际调用 `local_unix::VerifyRLimit`，而非 Windows 平台分派。
- `pkg/ingestor/ingestctrl/Cargo.toml`：确认 crate 名称、`lib.rs` 入口、Go 包移植元数据和 Windows 目标依赖边界。
- `pkg/ingestor/ingestctrl/local_windows.go`：确认 Windows build tag、`uint64` 类型、`math.MaxInt32` 返回值及固定错误文本。
- `pkg/ingestor/ingestctrl/local_windows_test.rs`：确认独立测试对成功值、`0`/`u64::MAX` 边界和精确错误文本的断言。
- RustCodeGraph：`status` 显示索引包含本文件；`files --filter pkg/ingestor/ingestctrl/local_windows.rs` 和 `node --file ...` 显示文件有 5 个符号并由 `local_windows_test.rs` 使用；`query GetSystemRLimit`、`query VerifyRLimit` 将 Rust 符号分别定位到第 27、34 行，并同时定位到 Go 对照符号。对 callers/callees 的查询没有返回额外生产调用边，仓库 `rg` 搜索进一步确认当前唯一直接 Rust 使用来自测试，而 `NewBackend` 指向 Unix 实现。
- 未运行 Cargo：任务是纯文档分析，且计划明确排除 Cargo 验证；文档结构检查用于交付验证。
