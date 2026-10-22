# `pkg/util/etcd.rs`

## 文件定位

`CancellationContext` 提供 Go `context.Context` 的一个小型子集：可跨克隆共享显式取消标记，并可携带绝对截止时间。

## 核心职责

- `NewSession` 在有限或近似无限的重试预算内调用工厂创建 session；每次尝试前先检查取消、截止期限和上一次失败是否属于终止类错误。
- `CancellationContext` 提供 Go `context.Context` 的一个小型子集：可跨克隆共享显式取消标记，并可携带绝对截止时间。
- `ContextCanceled`、`DeadlineExceeded`、`ClientConnectionClosing` 为 `anyhow::Error` 提供可按具体类型识别的三种终止原因。
- `FormatLeaseID` 将有符号 64 位 lease ID 格式化为至少 16 位、零填充的小写十六进制文本，与 Go 的 `%016x` 目的相同。
- `newSessionRetryInterval`、`logIntervalCnt`、`NewSessionDefaultRetryCnt` 和 `NewSessionRetryUnlimited` 固化重试节奏及公共默认值。

## 主要符号

- `CancellationContext { cancelled: Arc<AtomicBool>, deadline: Option<Instant> }`：可克隆的取消上下文。`new()` 创建无期限上下文；`with_deadline(Instant)` 保存绝对期限；`cancel()` 以 `Release` 写入共享标志；`error()` 先以 `Acquire` 读取取消状态，再判断 `Instant::now() >= deadline`。
- `SessionFactory`：关联类型 `Session` 表示成功产物；`new_session(&mut self, &CancellationContext, ttl: i32)` 是实际建连边界。可变接收者允许工厂记录尝试次数或维护客户端状态。
- `NewSession<F: SessionFactory>(..., retryCnt: i64, ttl: i32) -> Result<Option<F::Session>, anyhow::Error>`：公开重试入口。`Some(session)` 表示创建成功；有尝试且全部失败时返回最后一个错误；零次或负数预算没有最后错误，返回 `Ok(None)`。
- `contextDone(...) -> Option<Error>`：私有终止判定。当前上下文错误优先于历史错误；历史错误只有能向下识别为上述三种具体类型时才终止重试。
- `FormatLeaseID(i64) -> String`：无状态纯格式化函数。
- 四个常量：重试间隔为 200 ms；每 15 次失败记录一次 warn；默认重试 3 次；“无限”预算用 `i64::MAX` 表示，并非真正无界循环。

## 执行流程

`NewSession` 的每轮行为如下：

1. `(0..retryCnt).enumerate()` 产生本轮失败计数；当预算不大于零时循环不进入。
2. 调用 `contextDone(ctx, last_error.as_ref())`。若上下文已取消/过期，或上次错误是取消、超时、连接关闭，立即返回相应类型的新错误，不再调用工厂。
3. 记录开始时间并调用 `factory.new_session(ctx, ttl)`。本文件只透传上下文和 TTL，不解释 TTL，也不直接创建 lease/session。
4. 成功时记录 debug 日志并立即返回 `Ok(Some(session))`。
5. 失败时保存错误；失败序号能被 15 整除时记录 warn，因此第一次失败也会记录。
6. 每次失败后固定阻塞当前线程 200 ms，包括预算中的最后一次失败。若仍有预算，下一轮开始时会先检查刚保存的错误是否属于终止类。
7. 循环耗尽后返回最后错误；仅在一次尝试都未发生时返回 `Ok(None)`。

`FormatLeaseID` 直接使用 `format!("{id:016x}")`。宽度 16 是最小宽度；负数按 Rust 的有符号十六进制格式规则输出，兼容性需要由测试锁定后再依赖。

## 数据与状态

模块没有全局可变状态。`CancellationContext` 的显式取消状态位于 `Arc<AtomicBool>` 中，所以其克隆共享取消结果；`deadline` 随结构克隆而复制，仍表示同一个单调时钟时间点。取消一旦写入就不会复位。

`NewSession` 的局部状态只有 `last_error`、循环计数和每次尝试的 `started` 时间。工厂自身状态由调用方拥有，并通过 `&mut F` 保证同一次同步调用期间的独占访问。成功 session 的所有权移入返回值；失败错误先由 `last_error` 持有，最终被返回或在终止类型判断时被同类型新错误替代。

## 依赖与调用关系

crate 边界由 `pkg/util/Cargo.toml` 定义：本文件直接使用 `anyhow`、`log` 和 `thiserror`，以及标准库的原子变量、`Arc`、线程休眠和时间类型；这些依赖均已在 `astersql-util` 声明。本实现没有依赖 Rust etcd SDK，具体客户端适配必须由 `SessionFactory` 实现提供。

RustCodeGraph 给出的 `NewSession` 下游边是 `contextDone` 与 `SessionFactory::new_session`；调用边未把日志宏、计时和 `thread::sleep` 建模为普通函数边。其上游边仅指向 `pkg/util/etcd_test.rs::terminal_session_error_preserves_its_type` 和 `pkg/util/cpu_posix_1_aster_unit_test.rs::etcd_session_retries_checks_context_and_formats_lease_id`。`FormatLeaseID` 没有生产调用者，测试调用由源码检索确认。

Go 对照的生产调用关系包括：`pkg/owner/manager.go` 创建 owner session 并格式化 lease；`pkg/ddl/schemaver/syncer.go` 使用默认或无限预算；`pkg/ddl/serverstate/syncer.go` 与 `pkg/domain/serverinfo/syncer.go` 创建服务协调 session。它们证明该工具在完整 Go 应用中的架构位置，但不能作为 Rust 已接线的证据。

## 错误处理与边界

- 上下文状态优先：即使已有其他失败，显式取消或期限到达也返回 `ContextCanceled` 或 `DeadlineExceeded`。
- 上次错误仅通过 `anyhow::Error::is::<T>()` 识别三种终止类型；普通临时错误继续消耗预算。包裹错误能否识别取决于 `anyhow` 的类型链。
- 终止类错误发生于某次工厂调用时，函数会先完成该轮的 warn 与 200 ms 休眠，再在下一轮开始时终止；若该轮正好耗尽预算，则循环后直接返回原始 `anyhow::Error`。
- `retryCnt <= 0` 返回 `Ok(None)`，调用方必须处理“没有 session 也没有错误”。`NewSessionRetryUnlimited` 仍会被取消、期限或终止错误打断。
- `ttl` 未在此层校验，非正值或其他约束完全交给工厂；`logPrefix` 只进入日志。
- 休眠不可被上下文唤醒，取消最多要等待当前 200 ms 休眠结束才会被下一轮观察到。

## 并发与资源生命周期

`CancellationContext` 可安全跨线程克隆和取消：`Arc<AtomicBool>` 配合 Release/Acquire 保证取消标志的可见性。deadline 使用 `Instant`，避免墙上时钟调整影响期限比较。

`NewSession` 本身是同步且阻塞的：工厂调用和 `thread::sleep` 都占用当前 OS 线程，没有异步任务、通道或锁。`&mut F` 排除了同一工厂在此次调用期间被其他安全 Rust 代码并发调用，但模块不替调用方协调不同工厂或上下文。成功时 session 生命周期交给调用方；失败时本模块没有可释放的具体客户端资源，清理由工厂负责。取消上下文不会主动关闭已经成功返回的 session。

## 与 Go 版本的对应关系

尚未对齐或未接入的 Go 行为必须明确保留为差异：Rust 没有 `closeClient`/`closeGrpc` failpoint；没有 `metrics.NewSessionHistogram` 观测；日志使用 `log` 而不是 TiDB 的结构化 logger；没有直接调用 etcd SDK；也没有在 Rust owner、DDL 或 domain 路径发现调用。

## 扩展指南

- 接入真实 Rust etcd 客户端时，应新增独立适配器文件实现 `SessionFactory`，保持本文件只负责编排；同时在独立测试文件中覆盖 TTL 透传、资源关闭和真实 SDK 错误到三种终止类型的映射。
- 修改重试策略应集中在 `NewSession` 和四个常量，并同步核对 Go `pkg/util/etcd.go`。尤其要保留“尝试前终止检查、首次失败记录日志、失败后休眠、返回最后错误”的顺序，除非明确变更跨语言契约。
- 若要提升取消响应速度，可把不可中断的 `thread::sleep` 替换为可被取消唤醒的等待机制；这会改变并发与时序语义，需要在 `pkg/util/etcd_test.rs` 增加确定性测试，不能把测试嵌入生产文件。
- 扩展 `CancellationContext` 时要维持克隆间取消共享、不依赖墙上时钟，以及取消优先于 deadline 的现有判定顺序；新增错误类别需同时更新 `contextDone` 与独立测试。
- 若将 Rust API 接入 owner/DDL/domain，需逐个确认调用点的 session 类型、生命周期、指标与 failpoint 要求，不能仅因 Go 调用链存在就假定 Rust 接口可直接替换。
- `FormatLeaseID` 若要覆盖负数、高位值或与 etcdctl 完全一致的位级格式，应先增加与 Go 输出对照的边界测试。
