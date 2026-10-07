# `pkg/ingestor/ingestcli/mock/client_mock.rs`

## 文件定位

本文件属于独立 crate `astersql-ingestor-ingestcli-mock`。同目录 `Cargo.toml` 只直接依赖父目录的 `astersql-ingestor-ingestcli`，`lib.rs` 将 `client_mock` 设为私有模块后通过 `pub use client_mock::*` 公开其中的 mock 类型，并以独立的 `client_mock_test.rs` 承载单元测试。根 `Cargo.toml` 还以 `facade_ingestor_ingestcli_mock` 登记该 crate，`pkg/ingestor/ingestctrl/Cargo.toml` 将它列为开发依赖。

它位于 ingest 测试边界：生产接口 `pkg/ingestor/ingestcli/interface.rs` 定义“写 KV 生成 SST，再把 SST 导入目标 Region”的 `Client` 与 `WriteClient`；本文件实现这两个 trait 的可编程替身，供测试在不启动真实 HTTP/TiKV 的情况下控制返回值、观察调用并验证期望。它不实现网络协议、Region 路由或 SST 写入。

## 核心职责

1. 提供 `MockClient`，模拟 `Client::write_client` 与 `Client::ingest`。
2. 提供 `MockWriteClient`，模拟 `WriteClient::write`、`recv` 与 `close`。
3. 通过两个 recorder 的 `EXPECT()` 风格 API 排队一次性 matcher/responder，并在调用时消费匹配期望。
4. 保存每次调用的输入摘要，允许测试通过 `calls()` 检查调用历史。
5. 对未匹配调用、未消费期望和未满足的关闭次数进行累计，由 `verify()` 统一返回 `VerificationError`。

与通用 mocking 框架不同，这些行为全部由本文件的 `VecDeque`、闭包和互斥状态手写完成；`ISGOMOCK()` 仅保留 GoMock 兼容形状，不承担注册或校验。

## 主要符号

- `IngestMatcher` / `WriteMatcher`：接收请求引用并返回 `bool` 的 `Send` 闭包，用于选择匹配期望。
- `IngestResponder` / `WriteResponder`：取得请求所有权的一次性 `FnOnce` 闭包，返回对应操作的 `Result`。
- `WriteClientResponder`：一次性构造 `Box<dyn ingestcli::WriteClient>`，可同时注入 `ingestcli::Error`。
- `RecvResponder`：一次性返回 `WriteResponse` 或错误；`recv` 没有输入匹配器。
- `IngestExpectation`、`WriteClientExpectation`、`WriteExpectation`：把匹配条件与一次性响应器组合成待消费记录。`WriteClientExpectation::commit_ts` 为 `None` 时接受任意时间戳。
- `ClientCall`：公开记录 `Ingest` 或 `WriteClient` 调用；两种记录都保存调用时的 `context.is_cancelled()`，并分别保存请求副本或 `commit_ts`。
- `WriteClientCall`：公开记录 `Write(request)`、`Recv`、`Close`。
- `VerificationError { messages }`：聚合多条验证消息；`Display` 用 `"; "` 连接消息，并实现标准 `Error`。
- `ClientState` / `WriteClientState`：私有状态容器，分别持有期望队列、调用历史与失败记录；后者另有 `expected_close` 计数。
- `MockClient` / `MockWriteClient`：公开替身，内部均为 `Arc<Mutex<State>>`，实现生产 trait，并可安全克隆共享同一状态。
- `MockClientMockRecorder` / `MockWriteClientMockRecorder`：公开 recorder，同样共享状态；其 `Ingest`、`WriteClient`、`Write`、`Recv`、`Close` 方法返回 `&Self` 以支持链式排队。
- `NewMockClient()` / `NewMockWriteClient()`：创建空状态替身。命名刻意保留 Go 版本风格。
- 两个 `verify()`：检查运行期失败记录和所有仍未消费的期望，但不会清空状态。

## 执行流程

`MockClient` 的典型流程如下：

1. 测试调用 `NewMockClient()`，再由 `EXPECT()` 取得共享 recorder。
2. `MockClientMockRecorder::Ingest` 把 matcher/responder 追加到 `ingest` 队尾；`WriteClient` 把可选 `commit_ts` 与工厂 responder 追加到另一队列。
3. `Client::ingest` 先在锁内记录 `ClientCall::Ingest`，其中请求被克隆、取消状态被即时采样；然后从队首向后寻找第一个 matcher 返回 `true` 的元素，并按位置移除。
4. `Client::write_client` 同样先记录调用，再寻找第一个 `commit_ts` 未限定或等于实参的期望。
5. 找到后先释放互斥锁，再调用对应的 `FnOnce` responder；这样用户闭包执行期间不持有内部锁。找不到时记录失败，并返回 `Error::InvalidHttpResponse`。

`MockWriteClient` 的流程相似，但各操作有不同消费规则：

1. `Write` 期望按 matcher 搜索第一个匹配项，因此后排但匹配的期望可以先被消费，未匹配的早期项仍留在队列中。
2. `Recv` 没有参数，严格使用 `pop_front()` 按 FIFO 消费 responder。
3. `Close` 不调用 responder，只消耗 `expected_close` 计数；无剩余计数时把意外关闭写入失败记录。
4. 测试最后调用 `verify()`；只有失败记录为空、各队列为空且关闭计数为零时才返回 `Ok(())`。

## 数据与状态

所有可变数据都在堆上的共享状态中。克隆 mock 或 recorder 只克隆 `Arc`，不会复制期望队列，因此任一克隆上的调用都影响同一组期望、历史和验证结果。`calls()` 会在锁内克隆历史后返回快照，调用方不能借此修改内部状态。

期望队列使用 `VecDeque`。`Ingest`、`WriteClient` 与 `Write` 虽按队尾加入，却通过 `iter().position(...)` 搜索第一个匹配项，而非无条件消费队首；这既保留相同匹配条件的先入优先，也允许不同条件的后续期望先执行。`Recv` 则只按队首消费。所有 responder 都是 `FnOnce`，与每条期望最多被消费一次的不变量一致。

请求类型来自 `ingestcli`：`WriteRequest` 含 KV 对，`WriteResponse` 可含生成的 SST 元数据，`IngestRequest` 组合目标 `RegionInfo` 与写入结果。mock 不解释这些字段，只有 matcher、调用历史或 responder 会观察它们。

## 依赖与调用关系

- crate 边界：`pkg/ingestor/ingestcli/mock/Cargo.toml` 仅连接 `astersql-ingestor-ingestcli`，说明该 mock 不依赖真实实现的额外传输或运行时组件。
- 接口下游：`MockClient` 实现 `pkg/ingestor/ingestcli/interface.rs::Client`，`MockWriteClient` 实现同文件的 `WriteClient`；错误与请求/响应类型也全部从该 crate 导入。
- 标准库下游：`VecDeque` 管理待消费期望，`Arc<Mutex<_>>` 管理共享状态，`std::fmt` 与 `std::error::Error` 提供聚合错误展示。
- 直接已验证调用者：RustCodeGraph 将 `NewMockClient`、`NewMockWriteClient`、`EXPECT`、trait 方法和 `verify` 连接到 `pkg/ingestor/ingestcli/mock/client_mock_test.rs` 的三个测试。
- 上层测试接线：`pkg/ingestor/ingestctrl/Cargo.toml` 把此 crate 声明为开发依赖；当前源码检索未发现其中直接构造 Rust mock 的活动代码，`job_worker_test.rs` 仅以注释保留 Go 测试中 `NewMockClient`/`NewMockWriteClient` 的对应点。因此不能把该 crate 描述成当前生产主链的一部分。
- Go 上游用法：`pkg/ingestor/ingestctrl/job_worker_test.go` 使用生成的 Go mock 验证 job worker 写入/导入流程，为 Rust mock 后续接入同类测试提供语义来源。

## 错误处理与边界

未找到期望的 `write_client`、`ingest`、`write`、`recv` 会做两件事：把固定文本加入 `failures`，并立即返回 `ingestcli::Error::InvalidHttpResponse`。这样被测代码能走正常错误分支，事后 `verify()` 仍能证明发生过意外调用。`close` 的 trait 签名没有错误返回，因此意外关闭只能记录到 `failures`。

匹配成功后，responder 返回的成功值或 `ingestcli::Error` 原样传播；responder 自身返回错误不会自动写入 `failures`，因为这是测试刻意配置的业务结果，而不是 mock 协议违约。反过来，即使调用已经返回错误，期望也已被移除，符合“一次调用消费一次期望”。

`verify()` 同时报告全部已记录失败和各类剩余期望，不会遇到第一项就提前返回；也不会自动在 `Drop` 时运行。测试必须显式调用它。所有锁使用 `expect("... lock poisoned")`，若持锁线程 panic 导致中毒，后续 API 会 panic，而不是转换为 `ingestcli::Error`。

matcher 与 responder 都可能由测试代码 panic；本文件不捕获 panic。matcher 在持锁期间执行，因此 matcher 不应重入同一 mock，否则存在自锁风险。responder 在锁外执行，可以安全调用同一 mock 的其他方法，但仍应避免制造不符合测试意图的递归调用。

## 并发与资源生命周期

`Client` 要求 `Send + Sync`，`WriteClient` 要求 `Send`。本文件通过 `Arc<Mutex<_>>` 保护状态，并要求存入的 matcher/responder 为 `Send`，使替身满足这些 trait 边界。多线程调用会被互斥锁串行化；调用历史的顺序是取得锁的顺序，不承诺与线程启动顺序一致。

锁只覆盖记录调用、查找/移除期望和更新计数；一次性 responder 在锁外执行，避免长时间占锁并允许 responder 返回拥有所有权的 `Box<dyn WriteClient>` 或 `WriteResponse`。`MockWriteClient` 的 trait 方法仍接收 `&mut self`，但克隆出来的实例共享状态，因此调用方应把它看作一个逻辑 mock，而不是彼此独立的流。

本文件不创建线程、异步任务、通道、文件或网络资源。返回的装箱 `WriteClient` 所有权移交调用者，关闭责任遵循生产接口约定；mock 自身销毁不会替调用者调用 `close`。期望闭包捕获的资源一直存活到期望被消费或最后一个共享状态句柄销毁。

## 与 Go 版本的对应关系

Go 对照为 `pkg/ingestor/ingestcli/mock/client_mock.go`，它是 MockGen 生成代码。两边都公开 `MockClient`、`MockWriteClient`、`NewMock*`、`EXPECT`、`ISGOMOCK`，并覆盖 `Ingest`、`WriteClient`、`Write`、`Recv`、`Close`，但实现模型并不相同：

- Go 构造函数接收 `*gomock.Controller`；Rust 构造函数无参数，状态和验证逻辑由替身自身维护。
- Go `EXPECT()` 返回持有 mock 指针的生成 recorder，调用期望由 `RecordCallWithMethodType` 注册；Rust recorder 仅共享 `Arc<Mutex<State>>`，直接把闭包记录压入队列。
- GoMock 支持其框架定义的 matcher、次数和调用顺序能力；Rust 当前只支持请求闭包、可选 `commit_ts`、一次性 responder、`Recv` FIFO 和 `Close` 次数累计，不能等同于完整 GoMock 功能集。
- Go 的请求/响应使用指针；Rust trait 使用拥有所有权的请求和返回值，并在历史记录需要时显式克隆。
- Go 通过 controller 在测试结束时处理未满足期望；Rust 必须显式调用 `verify()`。
- Rust 额外公开 `calls()`、`ClientCall`、`WriteClientCall` 与聚合 `VerificationError`，用于弥补没有外部 controller 的观察和校验能力。

`client_mock_test.rs` 证明 Rust 实现刻意允许后排匹配项先执行而不消耗前排不匹配项；这是当前 Rust 事实。若要声称与 GoMock 的更细粒度选择/顺序规则完全一致，还需要额外对照测试，不能仅依据同名 API 推断。

## 扩展指南

当 `ingestcli::Client` 或 `WriteClient` 增加必需方法时，应同时补齐：响应器/匹配器类型、私有 expectation、状态字段、recorder 方法、trait 实现、调用历史枚举、`verify()` 剩余期望报告，以及独立的 `client_mock_test.rs` 覆盖。不要把测试模块嵌回本生产文件。

若要增加调用次数、严格顺序或更丰富 matcher，应先明确与 GoMock 的目标语义。当前“搜索第一个匹配项”的规则是重要兼容行为；改成简单 `pop_front()` 会破坏已验证的后排匹配用例。新增无返回值方法时，需要像 `Close` 一样设计无法即时返回错误时的失败记录机制。

新增共享状态访问时，应继续保持“锁内只修改状态，锁外执行 responder”的结构；尤其不要在持锁时运行可能重入 mock 的 responder。matcher 目前在锁内执行，若未来允许复杂或重入 matcher，应先把期望安全移出或设计两阶段匹配，避免死锁，同时处理并发调用间的竞态语义。

兼容风险集中在公开 Go 风格名称、trait 签名、错误类型和验证消息；正确性风险集中在期望被错误消费或遗漏验证；性能通常不是测试替身的瓶颈，但 `position + remove` 对队列是线性搜索，期望数量很大时需评估。变更后至少同步 `pkg/ingestor/ingestcli/mock/client_mock_test.rs`，并根据接入场景核对 `pkg/ingestor/ingestctrl/job_worker_test.go` 的原始测试意图。

## 验证依据

- RustCodeGraph 状态：索引包含 7,032 个 Rust 文件、307,296 个节点和 1,848,419 条边。
- RustCodeGraph 目标源码：`node --file pkg/ingestor/ingestcli/mock/client_mock.rs --offset 1 --limit 500` 返回完整 431 行，并标识独立测试及 ingestctrl/simplesst 测试文件为关联文件。
- RustCodeGraph 调用证据：`explore "client_mock.rs in pkg/ingestor/ingestcli/mock: symbols and callers/callees"` 显示 `NewMockClient`、`NewMockWriteClient`、`calls`、`verify` 及各 trait 方法与 `client_mock_test.rs` 的调用边。
- RustCodeGraph 接口证据：读取 `pkg/ingestor/ingestcli/interface.rs` 全部 174 行，确认 `RequestContext`、`Client`、`WriteClient` 以及请求/响应所有权契约；读取 `pkg/ingestor/ingestcli/lib.rs` 确认公开导出边界。
- 读取的仓库契约与接线：`pkg/ingestor/doc.go`、`pkg/ingestor/ingestcli/mock/lib.rs`、`pkg/ingestor/ingestcli/mock/Cargo.toml`、`pkg/ingestor/ingestcli/Cargo.toml`、`pkg/ingestor/ingestctrl/Cargo.toml` 和根 `Cargo.toml`。
- 读取的 Go 对照与用法：`pkg/ingestor/ingestcli/mock/client_mock.go`、`pkg/ingestor/ingestctrl/job_worker_test.go`。
- 读取的独立 Rust 测试：`pkg/ingestor/ingestcli/mock/client_mock_test.rs`；三个测试分别覆盖后排匹配、调用历史/完整消费和未匹配错误聚合。
- 本任务为纯文档分析，按计划不运行 Cargo；交付验证仅包含固定章节结构检查、内容自审和差异范围检查。
