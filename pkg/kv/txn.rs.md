# `pkg/kv/txn.rs`

## 文件定位

`pkg/kv/txn.rs` 属于 `astersql-kv` crate，是 KV 抽象层的“内部事务辅助”实现。它不定义底层事务协议，而是组合本 crate 已有的 `Storage`、`Transaction`、`Context` 和事务选项，提供以下横切能力：在独立事务中执行回调并按策略重试、记录活跃内部事务的原始 startTS、检测长时间内部事务、生成提交重试退避，以及向事务写入请求来源和资源组信息。

模块由 `pkg/kv/lib.rs` 以 `#[path = "txn.rs"] mod txn_impl;` 装入，并通过 `pub use txn_impl::*;` 从 crate 根重新导出，因此外部调用通常写作 `astersql_kv::RunInNewTxn` 或别名 `kv::RunInNewTxn`，而不是引用私有的 `txn_impl` 模块。`pkg/kv/Cargo.toml` 表明该 crate 的库入口是 `lib.rs`；本文件直接使用的外部 crate 是 `fail`、`log` 和 `rand`，它们均为 `astersql-kv` 的普通依赖，不受 `nextgen` feature 条件控制。本文件没有条件编译项。

它位于“上层元数据/会话逻辑—KV 接口—具体存储实现”链路的中间：例如 `pkg/session/runtime/bootstrap_wait.rs`、`pkg/session/starter_bootstrap_file.rs`、`pkg/domain/autoid_store.rs` 会把元数据读写闭包交给 `RunInNewTxn`；本文件再调用动态分派的 `Storage::Begin` 和 `Transaction::{Commit,Rollback,SetOption,...}`。它不负责 SQL 事务状态、两阶段提交实现或具体 TiKV RPC。

## 核心职责

1. `RunInNewTxn` 为内部操作建立全新事务，每次尝试都重新 `Begin`，执行调用方回调，并在成功后 `Commit`。
2. 仅当调用方允许重试且错误满足 `IsTxnRetryableError` 时重试。回调错误会先尽力 `Rollback`；提交错误的可重试路径还会调用 `BackOff`。`Begin` 错误、不可重试错误直接返回。
3. 首次成功 `Begin` 后，把该事务的 startTS 注册到全局集合。`InnerTxnGuard` 在所有退出路径（成功、错误、重试耗尽或栈展开）上移除它，使 `GetMinInnerTxnStartTS` 可以参与 SafeTS/GC 类最小时间戳计算。
4. `PrintLongTimeInternalTxn` 把 TSO 的物理毫秒部分转换为 `SystemTime`；超过五分钟才记 info 日志，并区分 `RunInNewTxn` 与内部 session 两类来源。
5. `setRequestSourceForInnerTxn` 将 `Context::RequestSource()` 中的内部标记、来源类型和可选显式来源类型复制为事务选项；缺失来源或错误地标成外部请求时记录告警。
6. `SetTxnResourceGroup` 写入 `ResourceGroupName`。测试 failpoint `TxnResourceGroupChecker` 生效时，还安装 RPC 拦截器，校验 Prewrite、Commit、PessimisticLock 三类请求携带预期资源组名。

## 主要符号

- `TimeToPrintLongTimeInternalTxn: Duration`：公开的长事务告警阈值，固定为五分钟。
- `globalInnerTxnTsBox: LazyLock<InnerTxnStartTsBox>`：进程内延迟初始化的活跃内部事务 startTS 登记表。
- `InnerTxnStartTsBox`：私有状态容器，内部是 `Mutex<HashSet<u64>>`。`new` 预留 256 个元素；`storeInnerTxnTS`、`deleteInnerTxnTS` 修改集合；`getMinStartTS` 在持锁扫描时记录长事务并计算下限之上的最小值。
- `GetMinInnerTxnStartTS(now, start_ts_lower_limit, current_min_start_ts) -> u64`：公开查询门面。结果初始为调用方给定的当前最小值，只接受严格满足 `inner_ts > start_ts_lower_limit && inner_ts < current_min` 的登记值。
- `GetTimeFromTS(start_ts) -> SystemTime`：私有 TSO 解码，将低 18 位逻辑计数去掉，把高位物理毫秒加到 Unix epoch。
- `PrintLongTimeInternalTxn(now, start_ts, run_by_function)`：公开日志辅助。`start_ts == 0` 立即返回；系统时钟早于 startTS 时用零时长兜底，不误报长事务。
- `InnerTxnGuard`：私有 RAII 清理器，持有可选 startTS；`Drop` 时从全局集合删除。
- `RunInNewTxn<F>(ctx, store, retryable, callback) -> Result<(), Error>`：核心公开 API。回调为 `FnMut(&Context, &mut dyn Transaction) -> Result<(), Error>`，因此同一个闭包可能在多次尝试中被再次调用，闭包捕获的外部状态不会自动回滚。
- `logRetry`：私有重试日志函数，同时记录当前尝试 startTS、首次事务 startTS 和错误。
- `MaxRetryCnt: AtomicU32`：公开、运行时可调整的最大尝试次数，默认 100；循环开始时以 `Relaxed` 读取一次作为范围上界。
- `retryBackOffBase`、`retryBackOffCap`：私有指数退避参数，分别为 1 和 100；代码实际以毫秒构造 `Duration`。
- `BackOff(attempts) -> Duration`：公开的 full-jitter 风格退避。先用饱和运算计算 `min(100, 1 * 2^attempts)`，再随机选择 `[0, upper)` 毫秒、阻塞当前线程并返回实际睡眠时长。
- `setRequestSourceForInnerTxn`：私有请求来源选项接线。
- `SetTxnResourceGroup`：公开资源组接线与 failpoint 校验入口。

## 执行流程

`RunInNewTxn` 的一次完整调用按以下顺序运行：

1. 初始化原始 startTS、RAII guard 和“最后一次可重试错误”。
2. 在 `0..MaxRetryCnt` 中开始尝试。每次先调用 `store.Begin(&[])`；失败会记 error 日志并立即返回，不进入重试判定。
3. 把调用方 `Context` 的请求来源复制进新事务。第一次成功 Begin 时读取 `txn.StartTS()`，把它作为整个调用的 `original_txn_ts` 写入全局登记表，并交给 guard 管理。后续重试事务有各自 startTS，但不会加入全局表。
4. 调用回调。回调失败时总是先调用 `Rollback`；回滚失败只记 warn 日志，不覆盖原错误。如果 `retryable` 为真且原错误可重试，记录重试信息并直接开始下一次尝试；该分支与 Go 版本一致，不执行 `BackOff`。否则返回回调错误。
5. 回调成功后，先检查 `mockCommitErrorInNewTxn` failpoint：`retry_once` 只在首次尝试制造 `ErrTxnRetryable`，`no_retry` 制造普通错误；未注入时才实际调用 `txn.Commit(ctx)`。
6. 提交成功立即返回 `Ok(())`。提交失败且允许重试、错误可重试时，记录错误、执行 `BackOff(attempt)` 后继续；否则直接返回提交错误。
7. 尝试耗尽后返回最后一次可重试错误；如果循环根本没有执行且没有错误（例如 `MaxRetryCnt` 被设为 0），当前实现返回 `Ok(())`。函数退出时 guard 删除首次 startTS。

活跃 startTS 查询流程较短：`GetMinInnerTxnStartTS` 转发到全局 box；`getMinStartTS` 一次持锁遍历集合，对每个登记值调用 `PrintLongTimeInternalTxn`，再应用严格下限和当前最小值两道过滤，最后返回候选最小值。

资源组流程中，`SetTxnResourceGroup` 始终先设置 `ResourceGroupName`；只有 failpoint 返回期望名称时才设置 `RPCInterceptor`。拦截器忽略其他请求类型，对 Prewrite、Commit、PessimisticLock 使用断言验证 `RpcRequest::ResourceGroupName`。

## 数据与状态

- 全局集合只保存每个 `RunInNewTxn` 调用的首次 startTS，而不是每次重试的 startTS。这让监控值稳定代表整段逻辑事务的起点，重试日志则用 `original_txn_ts` 把后续尝试关联回来。
- `HashSet<u64>` 会合并相同 startTS。正常 TSO 应提供唯一时间戳；若不同活跃调用意外得到相同值，一个 guard 的删除会使另一个调用不再可见，这是该数据结构的隐含前提。
- `current_min_start_ts` 既是查询初值也是上界；集合为空、所有值不高于 lower limit、或所有值不小于当前最小时，返回值保持不变。
- `MaxRetryCnt` 表示最大尝试次数而非“首次尝试之外的重试次数”。它是全局原子变量，测试会临时修改；并行修改会影响新调用读到的上界。
- 回调是 `FnMut`，重试会重复执行其事务内逻辑。事务回滚不撤销闭包对进程内变量、日志、网络或其他外部系统产生的副作用，因此调用方必须让回调可安全重放。
- 请求来源值使用 `Box<dyn Any...>` 风格的事务选项传递；字符串在设置前克隆，布尔值按值复制。若存在来源对象但 `RequestSourceType` 为空，按缺失来源处理。

## 依赖与调用关系

向下依赖由 `pkg/kv/txn.rs` 的 `crate::{...}` 导入体现：

- `Storage::Begin` 创建每次尝试的事务；`Transaction` 提供 startTS、提交、回滚和事务选项接口。
- `IsTxnRetryableError` 和 `ErrTxnRetryable` 统一可重试错误判定及 failpoint 注入。
- `RequestSourceInternal`、`RequestSourceType`、`ExplicitRequestSourceType`、`ResourceGroupName`、`RPCInterceptor` 是 `Transaction::SetOption` 的键；`RpcInterceptor`、`RequestKind` 描述测试拦截器。
- 标准库提供全局初始化、互斥、原子和时间；`rand` 生成退避抖动；`fail` 提供两处测试注入；`log` 记录 error/warn/info。

RustCodeGraph 对 `pkg/kv/txn.rs::RunInNewTxn` 的调用边显示，它直接调用本文件的 `storeInnerTxnTS`、`setRequestSourceForInnerTxn`、`logRetry` 和 `BackOff`；索引识别的直接调用者包括 `pkg/ddl/tests/serial/serial_test.rs` 与 `pkg/kv/mpp_2_aster_unit_test.rs`。源码搜索补充了当前生产调用点，例如：

- `pkg/session/runtime/bootstrap_wait.rs`：读取和发布 SYSTEM keyspace bootstrap 版本；
- `pkg/session/starter_bootstrap_file.rs`：读取和完成 starter bootstrap；
- `pkg/domain/autoid_store.rs`：为 auto ID 存储适配事务执行；
- `pkg/session/runtime/session.rs`、`control.rs`、`system_session.rs`、`create_table_resources.rs`：会话与系统元数据操作。

RustCodeGraph 还确认 `GetMinInnerTxnStartTS -> InnerTxnStartTsBox::getMinStartTS`、`getMinStartTS -> PrintLongTimeInternalTxn -> GetTimeFromTS`，以及 `RunInNewTxn -> BackOff`。仓库当前未发现 Rust 生产代码调用 `GetMinInnerTxnStartTS`、`PrintLongTimeInternalTxn` 或 `SetTxnResourceGroup`；前两者的 Go 生产调用位于 `pkg/domain/infosync/info.go`，后者当前主要由 `pkg/session/test/resourcegrouptest/resource_group_test.rs` 验证。因此这些公开 API 已实现，但不能据此声称对应 Rust 生产链路已经接通。

## 错误处理与边界

- Begin 错误不重试；回调错误在返回或重试前尝试 Rollback；Rollback 错误仅记录，保持原回调错误的优先级。
- `retryable == false` 会关闭所有错误重试，即使错误类型为 `ErrTxnRetryable`。`retryable == true` 仍只重试 `IsTxnRetryableError` 认可的错误。
- 回调错误的可重试分支没有退避，提交错误的可重试分支有退避。这是 Rust 与当前 Go 实现共同的控制流，不应为了“统一”而单方面修改。
- `Mutex::lock().expect(...)` 把锁中毒视为不可恢复错误并 panic。`SetTxnResourceGroup` 的校验拦截器也有意通过 `assert_eq!` 让资源组不一致的测试立即失败。
- `GetTimeFromTS` 假定输入采用 TiDB/TiKV TSO 编码。任意 `u64` 都能被位移转换，但不代表它是有效 TSO。
- `SystemTime::duration_since` 出错时用零时长替代，因此未来时间戳或时钟倒退不会产生告警，也不会向调用者返回错误。
- `BackOff` 上界至少为 1，故随机范围始终合法；随机值采用半开区间，达到 cap 时实际睡眠为 0 至 99 毫秒。函数是同步阻塞睡眠，不适合直接放在要求非阻塞的异步执行线程上。
- 当 `MaxRetryCnt == 0` 时，循环不执行且返回 `Ok(())`。默认值和现有测试不覆盖这个边界；若要改变，必须先与 Go 语义及调用方预期对齐并增加独立回归测试。
- failpoint `mockCommitErrorInNewTxn = "no_retry"` 直接返回普通错误；`retry_once` 仅首次注入。未知值不注入错误。

## 并发与资源生命周期

`globalInnerTxnTsBox` 通过 `LazyLock` 只初始化一次，集合的每次访问都受 `Mutex` 保护。插入和删除持锁时间短；查询会在持锁期间遍历全部活跃 startTS，并可能为每项执行时间转换和日志调用，因此活跃内部事务很多或日志后端较慢时，会延迟并发注册/清理。当前容量 256 只是初始预分配，不是数量上限。

`InnerTxnGuard` 把登记生命周期绑定到 `RunInNewTxn` 栈帧：只有首次 Begin 成功后才设置 startTS；后续的成功、普通错误、重试耗尽和 panic 栈展开都会触发 `Drop` 清理（进程 abort 除外）。Begin 在首次成功前失败时没有登记，也无需清理。事务本身则由每轮局部变量拥有；回调错误显式 Rollback，提交错误路径依赖事务实现处理失败后的资源释放，本文件不会再调用 Rollback。

`MaxRetryCnt` 用原子避免数据竞争，但 `Relaxed` 只保证原子性，不建立其他状态同步关系。全局修改它适合受控测试或配置接线，不适合让多个并发请求拥有不同重试预算。`RunInNewTxn` 和 `BackOff` 都是同步 API；退避会占用当前 OS 线程。

资源组拦截器使用 `Arc` 封装闭包并移动捕获期望字符串，生命周期由事务选项持有。请求来源字符串克隆后同样由事务持有，不借用调用方 `Context`。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/kv/txn.go`，测试是 `pkg/kv/txn_test.go`。Rust 保留了 Go 的主要结构和分支：五分钟阈值、互斥集合、严格 lower-limit 过滤、首次 startTS 登记与退出删除、每轮重新 Begin、回调失败先 Rollback、仅提交重试退避、最大尝试数 100、full jitter 上限 100、请求来源选项以及三种资源组 RPC 的校验。

语言层面的对应替换包括：Go 的全局 map+mutex 对应 `LazyLock<InnerTxnStartTsBox>`；defer 删除对应 `InnerTxnGuard::drop`；`uint` 全局计数对应 `AtomicU32`；`oracle.GetTimeFromTS` 在 Rust 中以内联的右移 18 位完成；Go interface 对应 `dyn Storage`/`dyn Transaction`；Go failpoint 注入对应 `fail::eval`。

需要注意的已核实差异：

- Go 注释称退避基数/上限为 microsecond，但实现与 Rust 都乘 `time.Millisecond`/构造毫秒 `Duration`；文档应以实际实现的毫秒为准。
- Go 的请求来源来自 `context.Context` value，Rust 来自强类型 `Context::RequestSource()`；Go 测试模式的 `intest.Assert(true, ...)` 实际不触发，Rust只保留告警，当前可观察行为一致为继续执行。
- Go 资源组 failpoint 拦截真实 tikvrpc 请求并读取嵌套 protobuf context；Rust 拦截 crate 的抽象 `RpcRequest`，比较其已归一化的 `ResourceGroupName`。
- Rust 用饱和整数运算避免极大 attempts 的溢出；Go 用浮点 `math.Pow` 后取最小值。`pkg/kv/txn_test.rs` 和 `pkg/kv/mpp_2_aster_unit_test.rs` 都覆盖了极大 attempts 仍受 100ms 上限约束。
- Rust 测试用线程与 channel 让真实 `RunInNewTxn` 生命周期包住 startTS 查询，除复刻 Go 的多时间戳 lower-limit 场景外，还验证 guard 在调用结束后清理登记。

## 扩展指南

- 增加重试条件、预算或退避策略时，入口应集中在 `RunInNewTxn`、`IsTxnRetryableError` 判定和 `BackOff`，并保持“回调错误是否退避”“最大值是尝试次数还是额外重试次数”等 Go 语义。同步扩展独立文件 `pkg/kv/txn_test.rs`，不要把测试内嵌到 `txn.rs`；必要时同步 `pkg/kv/mpp_2_aster_unit_test.rs` 的 Go 对齐断言。
- 增加事务选项时，应在 `setRequestSourceForInnerTxn` 或新的窄辅助函数中接线，并确认每次重新 Begin 后都会重设；测试需验证首次尝试和重试尝试，避免只覆盖单次事务。
- 扩展资源组校验的请求类型时，修改 `SetTxnResourceGroup` 的 `RequestKind` 匹配，并同步 `pkg/session/test/resourcegrouptest/resource_group_test.rs`。这会触及抽象 RPC 边界，需核对底层请求构造是否确实填充 `RpcRequest::ResourceGroupName`。
- 改变 startTS 登记模型时，必须维护“首次成功 Begin 才登记、所有退出路径清理、下限严格排除”的不变量，并更新 `pkg/kv/txn_test.rs::test_inner_txn_start_ts_box`。若允许重复 startTS，应把 `HashSet` 改成引用计数结构，否则一个 guard 会过早删除共享值。
- 把最小 startTS 或资源组功能接入 Rust 生产链路时，应从 Go 的 `pkg/domain/infosync/info.go` 或相应 session 资源组路径追踪调用时机，而不是仅因公开函数存在就推断已接线。
- 性能风险主要是全局锁扫描、同步退避和高重试次数；兼容风险主要是错误优先级、回调重复执行、TSO 位布局、事务选项值类型及 Go/Rust failpoint 行为。修改这些位置应先补回归测试，再验证 Go 对照。

## 验证依据

- RustCodeGraph 索引状态：仓库索引包含 `pkg/kv/txn.rs`，该文件报告 16 个符号；使用 `node --file pkg/kv/txn.rs --offset 1 --limit 500` 读取了完整 283 行源码。
- RustCodeGraph 精确符号查询：`query RunInNewTxn --kind function --json` 定位 `pkg/kv/txn.rs:141`；`node pkg/kv/txn.rs::RunInNewTxn` 给出其对 `storeInnerTxnTS`、`setRequestSourceForInnerTxn`、`logRetry`、`BackOff` 的调用边及已识别调用者。另对 `GetMinInnerTxnStartTS`、`PrintLongTimeInternalTxn`、`BackOff`、`SetTxnResourceGroup` 执行 `node`，核对各自源码和图中调用关系。宽泛的 `callers RunInNewTxn` 因同名符号过多未在限定时间内返回，因此生产调用覆盖以精确符号 trail 加仓库源码搜索补齐，不把不完整图结果当作完整调用清单。
- crate 与装配证据：`pkg/kv/Cargo.toml`；`pkg/kv/lib.rs` 中 `txn_impl` 的 path 声明、根级再导出和独立 `txn_test` 声明。
- Rust 源码和调用证据：`pkg/kv/txn.rs`、`pkg/session/runtime/bootstrap_wait.rs`、`pkg/session/starter_bootstrap_file.rs`、`pkg/domain/autoid_store.rs`，以及仓库内对五个公开符号的搜索结果。
- Go 对照证据：`pkg/kv/txn.go`、`pkg/domain/infosync/info.go`。
- 测试证据：`pkg/kv/txn_test.rs`、`pkg/kv/txn_test.go`、`pkg/kv/mpp_2_aster_unit_test.rs`、`pkg/session/test/resourcegrouptest/resource_group_test.rs`。本任务只写文档，按计划未运行 Cargo 或代码测试。
- 人工复核结论：本文分别说明了该文件为何存在（内部事务横切能力）、如何运行（Begin/回调/Rollback/Commit/重试及 guard 生命周期）、当前接线边界，以及安全扩展时要修改的符号、独立测试和兼容/性能风险。
