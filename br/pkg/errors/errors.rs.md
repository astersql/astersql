# `br/pkg/errors/errors.rs`

## 文件定位

本文件是 `astersql-br-pkg-errors` 库的实现主体，crate 边界由 [`Cargo.toml`](./Cargo.toml) 定义，包名为 `astersql-br-pkg-errors`，并通过 `package.metadata.porting.go-package = "br/pkg/errors"` 明确对应 Go 包。[`lib.rs`](./lib.rs) 以 `mod errors; pub use errors::*;` 将这里的公开项全部提升到 crate 根，因此调用方通常写 `astersql_br_pkg_errors::ErrInvalidArgument`、`Is` 或 `IsContextCanceled`，不会直接引用私有模块路径。

它位于 BR（Backup & Restore）公共错误层：一方面集中声明跨备份、恢复、PD、流备份、PiTR、外部存储、EBS 和 TiKV 路径共享的稳定 RFC 错误码；另一方面提供按 RFC ID 识别规范化错误和识别 context 取消/超时的辅助函数。它不是错误日志、重试调度或具体业务错误的处理器；这些策略由上游调用方决定。

## 核心职责

1. `Canceled` 与 `DeadlineExceeded` 提供 Rust 侧等价于 Go `context.Canceled`、`context.DeadlineExceeded` 的哨兵类型，固定显示文本分别为 `context canceled` 和 `context deadline exceeded`（`errors.rs:18-42`）。
2. `Is` 在 `SharedError` 因果链中查找 `astersql_errors::Error`，按 `ID()` 而非对象地址或最终消息匹配指定规范化错误（`errors.rs:44-52`）。这使 `Annotate`、`Trace` 等包装不会破坏错误分类。
3. `IsContextCanceled` 同时检查 `Cause` 结果与原始错误的 `std::error::Error::source` 链，识别直接或间接包装的取消/超时哨兵（`errors.rs:54-96`）。
4. `br_err!` 将消息模板和 RFC CodeText 组合为 `LazyLock<astersql_errors::Error>`；本文件共调用该宏 69 次，形成 BR 的规范化错误目录（`errors.rs:98-516`）。

## 主要符号

| 符号 | 可见性 | 语义 |
| --- | --- | --- |
| `Canceled` | `pub` | 零字段、可复制的取消哨兵；实现 `Display`、`StdError`。 |
| `DeadlineExceeded` | `pub` | 零字段、可复制的截止时间超时哨兵；实现 `Display`、`StdError`。 |
| `Is(err, is)` | `pub` | `err: Option<&SharedError>` 允许空错误；通过 `Find` 遍历链并比较规范化错误的 RFC ID。 |
| `chain_is_canceled` | 私有 | 迭代 `StdError::source()`；既检查当前动态错误类型，也检查嵌套的 `SharedError` 本体。 |
| `shared_is_canceled` | 私有 | 先 downcast `SharedError` 的负载，再交给通用 source 链遍历。 |
| `IsContextCanceled(err)` | `pub` | 空值为假；先检查 `Cause`，再回查原始链，覆盖 Trace 和 `%w`/`source` 两类包装。 |
| `br_err!` | 模块内宏 | 为每个名称生成公开 `LazyLock<errors::Error>`，首次访问时调用 `Normalize(message, RFCCodeText(code))`。 |

69 个静态错误按源码分为：Common 11 个、PD 8 个、Backup 6 个、Restore 16 个、Stream 2 个、Restore/PiTR 边界 1 个、PiTR 5 个、ExternalStorage 3 个、EBS 3 个、KV 14 个。名称、消息模板和 RFC ID 以 [`errors.go`](./errors.go) 为兼容基准；不能从 Rust 标识符机械推导 RFC ID，例如 `ErrPDUnknownScatterResult` 保留历史 RFC 拼写 `ErrPDUknownScatterResult`，`ErrPDSplitFailed` 也复用该 ID，`ErrKVNotTiKV` 对应 `ErrNotTiKVStorage`。

## 执行流程

规范化错误的使用流程是：调用方首次解引用某个 `Err*` 静态量时，`LazyLock` 执行 `errors::Normalize`，保存消息模板与 RFC CodeText；调用方可克隆该 `errors::Error` 进入 `SharedError`，再经 `Annotate`、`Trace` 等增加上下文；分类时 `Is` 用 `Find` 遍历包装链，对每一层尝试 downcast 为 `errors::Error`，只要 `ID()` 相同即返回真，否则遍历结束返回假。

取消判定流程是：`IsContextCanceled(None)` 立即返回假；非空时调用 `Cause(Some(err))`，若没有结果则克隆原错误作为候选；`shared_is_canceled` 先检查候选负载是否是两个哨兵，再由 `chain_is_canceled` 沿 `source()` 逐层检查。若 Cause 路径未命中，再对原始 `SharedError` 重复检查，从而兼容 `pingcap/errors` 风格 Cause 与标准库 source/Unwrap 风格包装。任一层命中即短路为真。

静态错误本身不主动执行恢复或重试。RustCodeGraph 的调用边显示，`br/pkg/utils/backoff.rs` 的 `is_tikv_retry_err`、`is_tikv_non_retry_err`、`is_pd_retry_err`、`is_disk_check_retry_err` 调用 `Is` 来分类重试；同文件的 `NextBackoff` 及非重试判定、`br/pkg/conn/conn.rs` 的 `status_code` 与 `with_aggressive_retry` 调用 `IsContextCanceled`，把取消错误转换为停止重试或取消状态。

## 数据与状态

`Canceled` 和 `DeadlineExceeded` 都是无字段值类型，没有可变状态。每个 `Err*` 是进程级 `LazyLock<errors::Error>`：初始化前只保存构造闭包，首次访问完成一次规范化构造，之后共享同一只读错误定义。业务错误实例通常由静态定义克隆后放入 `SharedError`，包装上下文不会修改静态模板。

错误身份由 RFC ID 决定，不由展示字符串决定。这个不变量允许消息被 annotate，同时也意味着两个不同 Rust 名称若共享 RFC ID（如 `ErrPDUnknownScatterResult` 与 `ErrPDSplitFailed`）会被 `Is` 视为同一分类。消息模板中的 `%s` 仍按 Go 版本保留；本文件只登记模板，不负责参数格式化。

## 依赖与调用关系

下游依赖只有 `astersql-errors`（[`Cargo.toml`](./Cargo.toml)），本文件使用其 `SharedError` 作为动态错误容器，使用 `Find` 遍历错误链、`Cause` 获取根因，并用 `Normalize` 与 `RFCCodeText` 构造规范化错误。标准库提供 `StdError`、`fmt` 和线程安全的一次性惰性初始化 `LazyLock`。

模块出口是 [`lib.rs`](./lib.rs)，它公开再导出全部符号，并在 `cfg(test)` 下把 [`parity_test.rs`](./parity_test.rs) 与 [`errors_test.rs`](./errors_test.rs) 作为独立测试模块挂载。RustCodeGraph 将目标文件标记为被 7 个文件直接使用；关键生产调用者包括 [`br/pkg/utils/backoff.rs`](../utils/backoff.rs) 和 [`br/pkg/conn/conn.rs`](../conn/conn.rs)。例如 backoff 用 RFC 分类决定 TiKV/PD/磁盘错误能否重试，conn 在重试和状态映射前优先识别取消，避免把用户取消当普通连接失败继续循环。

Go 侧 [`errors.go`](./errors.go) 被更广泛的现有 BR Go 代码使用；Rust crate 是迁移后的并行公共边界，不改变 Go 包行为。

## 错误处理与边界

- `Is(None, ...)` 和 `IsContextCanceled(None)` 均安全返回假；没有 panic 或错误返回。
- `Is` 只识别能 downcast 为 `astersql_errors::Error` 的链节点，并严格比较 RFC ID；相同文本但非规范化类型不会误判，相同 ID 即使消息被包装仍命中。
- 取消判定只接受 `Canceled` 或 `DeadlineExceeded` 的动态类型，不靠字符串匹配，因而普通错误 `New("connection closed")` 为假。
- `chain_is_canceled` 的循环每次沿 `source()` 前进，直到命中或链尾；本文件不构造环。若外部自定义 `Error::source` 非法形成环，函数没有额外环检测。
- 历史拼写、共享 ID、消息语法都是兼容协议。修改它们可能改变重试分类、日志/遥测聚合或跨 Go/Rust 的错误识别，不能以“纠错”名义直接清理。
- 该文件定义分类，不承诺每个错误的重试性；例如 KV 段注释记录了部分既有约束，真正策略仍以 backoff 等调用者为准。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道、网络连接、文件或事务。`LazyLock` 负责静态错误的并发安全初始化；初始化完成后数据只读，调用者只借用或克隆。`Is` 和取消判定只读取传入错误链，不持有跨调用生命周期的引用，也不修改链。

`IsContextCanceled` 在 Cause 结果缺失时会克隆 `SharedError`，但克隆只延长共享错误所有权，不产生外部资源；局部值在函数返回时释放。错误链遍历是同步、线性的，时间复杂度随包装深度增长，额外空间为常数（不计 `SharedError` 克隆内部的共享所有权成本）。

## 与 Go 版本的对应关系

[`errors.go`](./errors.go) 的 `Is` 使用 `errors.Find` 并比较 `(*errors.Error).ID()`；Rust `Is` 使用同名语义的 `Find` 与 `downcast_ref`，保持“按 RFC ID 穿透包装”的契约。Go `IsContextCanceled` 先执行 `errors.Cause`，然后比较 `context` 两个哨兵并调用标准库 `errors.Is` 处理 `%w`；Rust 通过 `Cause`、两个本地哨兵与 `StdError::source` 链复现这两条路径，并额外回查原始链以避免 Cause 表示丢失标准包装。

Rust 的 `LazyLock` 对应 Go 包级 `var` 初始化：外部观察到的消息模板与 RFC ID 相同，但初始化时机从包加载改为首次访问。Rust 文件的 69 个 `br_err!` 声明与 Go 的 69 个 `errors.Normalize` 声明逐项对应；[`parity_test.rs`](./parity_test.rs) 的 `all_normalized_errors_match_go` 穷举核对全部模板和 ID。

[`errors_test.go`](./errors_test.go) 的 `TestIsContextCanceled` 覆盖 nil、普通错误、直接取消/超时、Trace 和 `url.Error` 包装；Rust [`errors_test.rs`](./errors_test.rs) 用实现 `source()` 的 `UrlError` 复现相同矩阵。Go `TestEqual` 与 Rust `test_equal` 都验证 `ErrPDBatchScanRegion` 经 annotate 后仍按身份相等；Rust parity 测试还直接核对 `Is` 的正反例与公共错误快照。

## 扩展指南

新增 BR 规范化错误时，应先确认 Go `br/pkg/errors/errors.go` 的对应增量或明确的 Rust 独立需求，再在正确分类段增加一个 `br_err!(名称, 消息模板, RFC ID)`。RFC ID 是兼容身份：必须检查是否应新建、历史上是否共享，以及上下游是否依赖既有拼写。随后在 [`parity_test.rs`](./parity_test.rs) 的 `all_normalized_errors_match_go` 中加入精确模板/ID；若新增的是 Go 对齐项，也应同步 Go 对照或至少记录差异来源。

若扩展错误链判定，优先修改 `Is`、`chain_is_canceled`、`shared_is_canceled` 或 `IsContextCanceled` 中最小的语义层，并在独立的 [`errors_test.rs`](./errors_test.rs) 或 [`parity_test.rs`](./parity_test.rs) 增加直接、Trace、标准 `source` 包装、空值和无关错误的回归用例；测试逻辑不要嵌入 `errors.rs`。还需检查 [`br/pkg/utils/backoff.rs`](../utils/backoff.rs) 与 [`br/pkg/conn/conn.rs`](../conn/conn.rs) 是否需要接入新分类。

兼容风险主要是 RFC ID/模板漂移和取消误判；性能风险主要来自让链遍历重复扫描或引入昂贵格式化。新增静态量通常只增加一次惰性初始化成本。不要在这里加入业务重试循环或资源管理，因为该 crate 的职责是稳定错误目录和纯判定辅助。

## 验证依据

- RustCodeGraph `status`：索引包含 7,032 个 Rust 文件，目标目录的 `errors.rs`、`lib.rs`、`errors_test.rs`、`parity_test.rs` 及 Go 对照均在索引内。
- RustCodeGraph `node --file br/pkg/errors/errors.rs`：核对完整 516 行实现、公开/私有符号、69 个 `br_err!` 调用及各消息/RFC ID；`files --filter br/pkg/errors` 核对模块文件集合。
- RustCodeGraph `explore`、`callers`、`callees`：确认 `Is -> Find/ID` 的实现语义，确认 `Is` 的 backoff 分类调用者，以及 `IsContextCanceled -> shared_is_canceled -> chain_is_canceled` 和 conn/backoff 的五个生产调用点。
- 读取 [`Cargo.toml`](./Cargo.toml) 与 [`lib.rs`](./lib.rs)：核对 crate 名、唯一外部 crate 依赖、Go 包映射、公开再导出和独立测试模块。
- 读取 [`errors.go`](./errors.go)、[`errors_test.go`](./errors_test.go)、[`errors_test.rs`](./errors_test.rs)、[`parity_test.rs`](./parity_test.rs)：核对 Go/Rust 函数语义、取消边界、包装行为、Equal/Is 身份和 69 项穷举快照。
- 本任务是只读逻辑分析加文档，不运行 Cargo；交付验证使用任务指定的 11 章节结构命令，并人工检查文档未把错误定义误写成业务重试实现。
