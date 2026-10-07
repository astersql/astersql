# `pkg/ingestor/errdef/errors.rs` 逻辑说明

## 文件定位

`errors.rs` 是独立 crate `astersql-ingestor-errdef` 的实现文件，由 `pkg/ingestor/errdef/lib.rs` 声明为 `errors` 模块并整体再导出。该 crate 的 `Cargo.toml` 没有运行时依赖或 feature，`package.metadata.porting.go-package` 指向 Go 对照包 `pkg/ingestor/errdef`。它位于 ingest 基础设施的错误契约边界：将 TiKV Region/ingest 错误、next-generation write/ingest HTTP 状态错误，以及 global-sort 归并文件数上限错误表示为可跨包装层识别的稳定类别。

包级说明 `pkg/ingestor/doc.go` 将 ingestor 定位为直接向存储层导入 SST，并负责 KV 排序、Region split/scatter 和 TiKV import mode 准备；本文件不执行这些操作，只为这些路径提供错误身份和文本格式。直接依赖该 crate 的 manifest 包括 `pkg/ingestor/ingestcli/Cargo.toml`、`pkg/ingestor/globalsort/Cargo.toml`、`pkg/ingestor/ingestctrl/Cargo.toml`、`pkg/ddl/Cargo.toml` 与 `pkg/dxf/framework/scheduler/Cargo.toml`。

## 核心职责

1. `NormalizedError` 保存消息模板、稳定 RFC code，以及按参数生成后的可选消息；它模拟 Go `github.com/pingcap/errors.Normalize` 在本仓库所需的最小错误契约。
2. 九个 ingest/TiKV 静态错误原型（`ErrNoLeader` 至 `ErrKVRaftProposalDropped`）集中定义消息和 RFC code，供 `pkg/ingestor/ingestcli/ingest_err.rs::NewIngestAPIError` 把 TiKV `errorpb.Error` 分类。
3. `IsKVDiskFullError` 沿 Rust `std::error::Error::source()` 链查找磁盘满 RFC code，使包装后的磁盘满错误仍可被识别。
4. `HTTPStatusError` 保留 nextgen write/ingest 请求非 200 时的状态码和消息，并提供与 Go 一致的错误文本。
5. `ErrTooManyDataFiles`、`TooManyDataFiles` 和 `IsTooManyDataFilesError` 为 global-sort 无法在文件数限制内安排归并计划的永久错误提供生成、包装和分类能力。

本文件不负责重试策略本身。重试与否由消费者根据这些身份决定，例如 `pkg/ddl/backfilling_dist_scheduler.rs::is_retryable_error` 将 `IsTooManyDataFilesError` 命中的错误视为不可重试。

## 主要符号

- `NormalizedError { message, rfc_code, rendered_message }`：公开 `message` 和 `rfc_code`，内部 `rendered_message` 区分静态原型与生成实例。`message` 使用 `Cow<'static, str>`，所以静态原型借用编译期字符串，动态生成时可持有 `String`。
- `NormalizedError::new`：用消息与 RFC code 建立未展开实例。
- `Code`：始终返回 `0`，对应未配置 MySQL 数值错误码的 Go 零值。
- `RFCCode` 与 `ID`：均返回 `rfc_code`；仓库消费者据此稳定分类，而不是依赖显示文本。
- `MessageTemplate`：返回 `message`。`GetMsg` 优先返回 `rendered_message`，否则返回模板；`GetSelfMsg` 复用 `GetMsg`。
- `Error`：生成 `[RFC code]message`；`Display` 只输出 `GetMsg()`，两者用途不同。
- `GenWithStack`：以调用方已构造的完整消息创建同 RFC code 的新实例。当前 Rust 实现会使新实例的 `MessageTemplate()` 也变成该完整消息，证据见 `errors_test.rs::normalized_error_generation_and_identity_match_go`。
- `GenWithStackByArgs`：克隆原型，只把模板中第一个 `%d` 替换为单个参数的显示值，原始 `message` 保持不变。
- `Is`、`PartialEq`、`Eq`：只比较 RFC code；消息不同但 code 相同仍属于同一错误类别。
- `ErrNoLeader`、`ErrKVEpochNotMatch`、`ErrKVNotLeader`、`ErrKVServerIsBusy`、`ErrKVRegionNotFound`、`ErrKVReadIndexNotReady`、`ErrKVDiskFull`、`ErrKVIngestFailed`、`ErrKVRaftProposalDropped`：与 `pkg/ingestor/errdef/errors.go` 同名、同消息、同 RFC code 的静态错误原型。
- `IsKVDiskFullError`：按 `Ingest:StoreDiskFull` 检查当前错误及其 `source()` 链。
- `HTTPStatusError { StatusCode, Message }`：实现 `Display`、`std::error::Error` 和 Go 风格 `Error()`，固定格式为 `request failed with status code <code>: <message>`。
- `ErrTooManyDataFiles`：RFC code 为 `GlobalSort:TooManyDataFiles`，消息模板含三个 `%d`。
- `TooManyDataFiles`：三参数专用生成器，直接格式化文件总数、并发度和目标文件上限，避免单参数 `GenWithStackByArgs` 无法展开三个占位符的问题。
- `IsTooManyDataFilesError`：沿 `source()` 链按 RFC code 判断，拒绝仅在普通文本中出现相同 code 的错误。

文件没有 trait、宏或条件编译项。所有生产符号均为公开 API；唯一的内部状态字段是 `NormalizedError::rendered_message`。

## 执行流程

TiKV ingest 分类主流程如下：

1. `pkg/ingestor/ingestcli/ingest_err.rs::NewIngestAPIError` 按固定优先级检查 `ErrorPb`：not-leader、epoch-not-match、raft proposal dropped、server busy、region not found、read-index not ready、disk full，未命中则归为 ingest failed。
2. 它从本文件克隆对应的静态 `NormalizedError`，连同 TiKV 响应详情构造 `CategorizedError`；分类比较继续使用 `RFCCode()`。
3. 上层 `IngestAPIError` 通过自身错误链向外传播。需要判断磁盘满时，`IsKVDiskFullError` 从外层错误开始逐层调用 `source()`，遇到 `NormalizedError` 后比较其 RFC code。

HTTP 失败流程如下：

1. `pkg/ingestor/ingestcli/client.rs::send_write_request` 在 `/write_sst` 响应不是 200 时创建 `HTTPStatusError`；`WriteClientImpl::ingest` 在非 200 且 protobuf 错误体解码失败时也创建它。
2. ingestcli 的本地 `Error::HttpStatus` 包装该值，并在 `source()` 中暴露它；日志或重试分类可读取稳定的状态码和消息文本。

global-sort 文件数限制流程如下：

1. `pkg/ingestor/globalsort/util.rs::DivideMergeSortDataFiles` 计算归并后的目标文件数；超过 `adjusted_overlap_threshold` 时调用 `TooManyDataFiles(file_count, merge_concurrency, target_limit)`。
2. 生成的 `NormalizedError` 被包装为 `pkg/ingestor/globalsort/lib.rs::Error::TooManyDataFiles`。该包装的 `Display` 使用带 RFC 前缀的 `NormalizedError::Error()`，`source()` 返回内层规范化错误。
3. `pkg/ddl/backfilling_dist_scheduler.rs::is_retryable_error` 调用 `IsTooManyDataFilesError` 穿透包装链；命中后返回不可重试。错误被 DXF 持久化为文本时，`is_retryable_scheduler_message` 退化为检查同一 RFC 前缀。

## 数据与状态

静态错误原型全部是不可变 `static NormalizedError`：`message` 为 `Cow::Borrowed`，`rfc_code` 为 `&'static str`，`rendered_message` 为 `None`。生成错误不会修改原型：`GenWithStackByArgs` 和 `TooManyDataFiles` 都先克隆，再把新实例的 `rendered_message` 设为拥有所有权的 `String`；因此相同原型可重复生成不同详情而互不影响。

错误类别的不变量是 RFC code，而不是消息。`PartialEq`/`Eq` 与 `Is` 均忽略模板及展开消息；例如消息为 `different detail`、code 为 `Ingest:StoreDiskFull` 的实例等于 `ErrKVDiskFull`。相反，`IsTooManyDataFilesError` 不会把只含字符串 `GlobalSort:TooManyDataFiles` 的普通 `io::Error` 误判为同类错误，见 `errors_test.rs::too_many_data_files_keeps_identity_through_wrappers`。

`HTTPStatusError` 是值对象，只有 `i32` 状态码与拥有所有权的 `String` 消息；本文件不校验状态码范围，也不在类型内部记录是否可重试。

## 依赖与调用关系

下游依赖仅来自 Rust 标准库：`Cow` 管理静态/动态消息，`Display` 负责文本表现，`std::error::Error` 提供类型擦除与 `source()` 链。本 crate 的 `Cargo.toml` 没有第三方依赖。

主要上游调用边为：

- `pkg/ingestor/ingestcli/ingest_err.rs::NewIngestAPIError` → 各 ingest/TiKV `Err*` 原型和 `NormalizedError::RFCCode`。
- `pkg/ingestor/ingestcli/client.rs::send_write_request`、`WriteClientImpl::ingest` → `HTTPStatusError`；ingestcli `Error::source` 再将其暴露给上层。
- `pkg/ingestor/globalsort/util.rs::DivideMergeSortDataFiles` → `TooManyDataFiles` → `ErrTooManyDataFiles` 克隆与消息格式化。
- `pkg/ingestor/globalsort/lib.rs::Error::{Display,source}` → `NormalizedError::{Error, std::error::Error}`，形成可显示且可遍历的包装链。
- `pkg/ddl/backfilling_dist_scheduler.rs::is_retryable_error` → `IsTooManyDataFilesError`，决定永久规划错误不重试。

RustCodeGraph 的文件节点还将 `errors.rs` 标记为被 `br/pkg/restore/snap_client/import.rs`、`br/pkg/restore/split/client.rs`、`pkg/ddl/backfilling_dist_scheduler.rs`、对应测试等文件使用；其中 BR 文件的同名错误主要来自 `astersql-br-pkg-errors`，不能仅因名字相同就认定为本 crate 的直接调用。本文的直接调用边以 crate 路径和限定 `rg` 结果为准。

## 错误处理与边界

- `GenWithStackByArgs` 只替换第一个 `%d`。它适合 `ErrNoLeader` 的单参数模板，不适合 `ErrTooManyDataFiles` 的三参数模板；后者必须使用 `TooManyDataFiles`。
- 如果模板没有 `%d`，`replacen` 不报错，只得到与原模板相同的展开文本。本文件不验证参数个数或格式类型。
- `IsKVDiskFullError` 和 `IsTooManyDataFilesError` 只识别错误链中可 downcast 为本 crate `NormalizedError` 的节点，并比较 RFC code；相同文本、不同类型不会命中。
- 两个检查函数在 `source()` 返回 `None` 时返回 `false`，不会构造新错误，也不会解析显示字符串。
- `NormalizedError::Error()` 带 `[RFC]` 前缀，而其 `Display` 不带前缀。包装层若需要持久化可分类文本，必须显式选择前者；`globalsort::Error::Display` 正是这样做的。
- `HTTPStatusError` 只表达“状态码 + 消息”。是否重试由上层策略根据 `StatusCode` 判断；本类型本身不把 4xx、5xx 编码成不同枚举。
- `Code()` 恒为零；新增需要 MySQL 数值码的错误时，不能假设现有类型已经支持该语义。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道、网络连接、文件句柄或事务，也没有 `Drop` 清理逻辑。静态错误原型在进程生命周期内只读存在；动态错误通过普通所有权和克隆传递，`Cow`/`String` 在实例离开作用域时由 Rust 自动释放。

包装链生命周期由调用者拥有：两个 `Is*Error` 函数只在调用期间借用 `&(dyn Error + 'static)` 并同步遍历 `source()`，不会保存引用或改变链中对象。`'static` 约束针对可 downcast 的错误对象类型，不表示传入引用本身必须永久存活。

由于没有内部可变状态，同一静态原型的分类和克隆不需要本文件内的同步措施；具体错误是否跨线程传递仍取决于外层 API 对 `Send`/`Sync` 的约束，本文件没有显式声明额外并发保证。

## 与 Go 版本的对应关系

`pkg/ingestor/errdef/errors.go` 是直接语义对照：九个 ingest/TiKV 错误和 `ErrTooManyDataFiles` 的消息及 RFC code 一致，`HTTPStatusError.Error()` 的文本格式一致。Rust 用 `NormalizedError` 显式承载 Go `errors.Normalize` 在当前消费者中需要的 `Code`、`RFCCode`、`ID`、消息生成、类别相等和标准错误接口。

两端的包装检测机制不同。Go `IsKVDiskFullError` 依次使用标准库 `errors.Is`、`errors.As` 和 PingCAP `errors.Cause`；Rust 没有这些包装类型，改为遍历标准 `source()` 链并 downcast `NormalizedError`。这要求 Rust 包装类型正确实现 `source()`，例如 `globalsort::Error::TooManyDataFiles` 已这样做。

Go 的 `GenWithStackByArgs` 是可变参数，能直接展开 `ErrTooManyDataFiles` 的三个 `%d`；Rust `GenWithStackByArgs` 仅接受一个 `Display` 参数，因此增加了专用 `TooManyDataFiles(file_count, concurrency, limit)`。Go 侧 `pkg/ingestor/globalsort/util.go::DivideMergeSortDataFiles` 直接调用三参数 `GenWithStackByArgs`，Rust 对应函数调用该专用构造器。

Go 当前没有独立的 `IsTooManyDataFilesError`；`pkg/ddl/backfilling_dist_scheduler.go` 使用 `goerrors.Is(err, errdef.ErrTooManyDataFiles)`。Rust 增加该辅助函数，是为了通过标准 `source()` 链实现相同分类意图。Go 的 `HTTPStatusError` 使用指针接收者，Rust 为值类型实现 `Error`；可观察的格式和字段语义保持一致。

## 扩展指南

新增 ingest/TiKV 错误类别时，应同时：

1. 在本文件按 Go 对照定义静态 `NormalizedError`，保持消息和 RFC code 精确一致；若属于 Go 增量，同步核对 `pkg/ingestor/errdef/errors.go`。
2. 在 `pkg/ingestor/ingestcli/ingest_err.rs::NewIngestAPIError` 的正确优先级位置接入分类，避免多字段 errorpb 改变既有选择顺序。
3. 扩展独立测试 `pkg/ingestor/errdef/errors_test.rs` 与 `migration_aster_unit_test.rs`；涉及 errorpb 映射时同步 `pkg/ingestor/ingestcli/ingest_err_test.rs`。
4. 若错误需穿透包装层，保证每一层实现 `std::error::Error::source()`，并添加“直接、同 code 不同消息、嵌套包装、普通文本伪装”四类测试。

新增多参数模板时，不应继续复用当前单参数 `GenWithStackByArgs`；可新增语义明确的专用构造器，或在不破坏现有调用者的前提下设计类型安全的参数格式化 API。任何修改 `Display`、`Error()` 或 RFC code 的变更都可能影响日志、DXF 持久化文本和重试分类，属于兼容性风险。新增字段会增加每个错误实例的内存占用；当前路径错误量通常远小于数据量，但仍不应在静态原型或生成器中携带大响应体。

修改 global-sort 上限错误时，还需同步 `pkg/ingestor/globalsort/util_test.rs` 和 `pkg/ddl/backfilling_dist_scheduler_test.rs`，前者验证生成条件与身份，后者验证包装后及持久化文本的不可重试语义。修改 HTTP 状态错误时，应同步 ingestcli 客户端测试和上层 HTTP 重试策略测试，而不是把策略塞入这个数据类型。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点；`files --filter pkg/ingestor/errdef` 确认本模块五个已索引源/测试文件。
- RustCodeGraph `node --file pkg/ingestor/errdef/errors.rs`：读取完整 263 行并确认 32 个符号、公开 API、字段和控制流；精确 `query` 定位了 `errors.rs::NormalizedError`、`IsKVDiskFullError`、`TooManyDataFiles` 和 `IsTooManyDataFilesError`。
- RustCodeGraph 的 `callers/callees` 精确查询在本次会话中长时间无输出后被终止，未把空输出解释为“无调用边”；调用关系改由文件节点的 `used by` 提示与限定 Rust 源码搜索交叉验证。
- crate/模块证据：`pkg/ingestor/errdef/Cargo.toml`、`pkg/ingestor/errdef/lib.rs`、根 `Cargo.toml` 及直接消费者的 Cargo manifests。
- Rust 调用证据：`pkg/ingestor/ingestcli/ingest_err.rs`、`pkg/ingestor/ingestcli/client.rs`、`pkg/ingestor/globalsort/util.rs`、`pkg/ingestor/globalsort/lib.rs`、`pkg/ddl/backfilling_dist_scheduler.rs`。
- Go 对照证据：`pkg/ingestor/errdef/errors.go`、`pkg/ingestor/globalsort/util.go`、`pkg/ddl/backfilling_dist_scheduler.go`、`pkg/ingestor/ingestcli/ingest_err.go`、`pkg/ingestor/ingestcli/client.go`；包级边界来自 `pkg/ingestor/doc.go`。
- 测试证据：`pkg/ingestor/errdef/errors_test.rs` 验证完整错误契约及 TooManyDataFiles 包装链；`pkg/ingestor/errdef/migration_aster_unit_test.rs` 验证 Go 消息/RFC 对齐、磁盘满链和 HTTP 文本；`pkg/ingestor/globalsort/util_test.rs`、`pkg/ddl/backfilling_dist_scheduler_test.rs` 与 `pkg/ingestor/ingestcli/ingest_err_test.rs` 验证直接消费者行为。Go 侧相关测试为 `pkg/ingestor/globalsort/util_test.go`、`pkg/ddl/backfilling_test.go`、`pkg/ingestor/ingestcli/ingest_err_test.go` 以及 HTTP 重试分类的 `pkg/lightning/common/retry_test.go`。
- 本任务是纯文档分析，按计划不运行 Cargo；最终只执行固定章节结构验证，并人工复核没有把字符串同名、BR 独立错误 crate 或预期架构误写成本文件的真实调用关系。
