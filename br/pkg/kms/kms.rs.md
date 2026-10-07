# `br/pkg/kms/kms.rs`

## 文件定位

`br/pkg/kms/kms.rs` 是 `astersql-br-pkg-kms` crate 的公共抽象层：它不创建云客户端，也不执行厂商特有的解密协议，而是定义取消上下文 `Context` 和统一的密钥管理服务接口 `Provider`。crate 入口 `br/pkg/kms/lib.rs` 以 `pub mod kms` 装载本文件，并通过 `pub use kms::*` 将两个符号提升到 crate 根；上层因此可以写 `astersql_br_pkg_kms::{Context, Provider}`，无需依赖内部模块路径。

该 crate 的边界由 `br/pkg/kms/Cargo.toml` 确认：包名为 `astersql-br-pkg-kms`，库入口是 `lib.rs`，`tokio-util = "0.7"` 提供本文件唯一直接依赖的 `CancellationToken`。同一 manifest 中的 AWS、GCP、Tokio 和 TLS 依赖由相邻厂商实现使用，不代表本文件自身执行网络 I/O。

## 核心职责

本文件承担两项职责：

1. 用 `Context` 把一次 KMS 解密操作的取消信号从上层传到厂商客户端。`Context::cancel` 发出信号，`Context::is_cancelled` 供同步逻辑轮询，crate 内部的 `Context::token` 供异步 SDK 适配器等待取消。
2. 用 `Provider` 固定所有 KMS 后端必须提供的三项行为：带上下文解密 data key、返回厂商标识、显式关闭资源。`br/pkg/kms/aws.rs` 与 `br/pkg/kms/gcp.rs` 分别实现这一契约，上层 `br/pkg/encryption/master_key/kms_backend.rs` 通过 trait 对象屏蔽厂商差异。

它刻意不负责 data key 的格式校验、重试、缓存、内容解密或厂商错误分类：这些职责分别位于 `br/pkg/kms/common.rs`、`br/pkg/encryption/master_key/kms_backend.rs` 以及 AWS/GCP 实现中。

## 主要符号

- `pub struct Context { cancellation: CancellationToken }`：一次或一组共享取消状态的轻量句柄。字段私有，外部只能通过方法操作状态。
- `Context::new() -> Self`：构造未取消的上下文，语义等同于 `Default::default()`。
- `Context::cancel(&self)`：触发底层 token；方法只需共享引用，调用后所有由该 token 派生或克隆的观察者都能看到取消。
- `Context::is_cancelled(&self) -> bool`：非阻塞读取取消状态。`br/pkg/encryption/master_key/kms_backend.rs::with_retry` 用它在一次 KMS 失败后决定是否停止重试。
- `Context::token(&self) -> &CancellationToken`：crate 内可见的底层访问口。`br/pkg/kms/aws.rs::AwsSdkClient::Decrypt` 和 `br/pkg/kms/gcp.rs::GcpSdkClient::Decrypt` 在 `tokio::select!` 中等待 `token().cancelled()`，外部 crate 不能绕过封装直接操作 token。
- `pub trait Provider`：公共动态分派边界。`DecryptDataKey(&self, &Context, &[u8]) -> Result<Vec<u8>, String>` 接收密文 data key 并返回明文；`Name(&self) -> &str` 提供稳定厂商名；`Close(&mut self)` 释放实现持有的资源。

本文件没有模块级常量、条件编译项或内部辅助函数。命名沿用 Go API 的首字母大写形式，crate 根在 `br/pkg/kms/lib.rs` 中显式允许 `non_snake_case`。

## 执行流程

完整主链可由以下调用关系复核：

1. `br/pkg/encryption/master_key/master_key.rs::createCloudBackend` 根据配置的 vendor 调用 `NewAwsKms` 或 `NewGcpKms`，并把实现装入 `Box<dyn Provider + Send>`。
2. `CreateKmsBackendWithProvider` 将该 trait 对象交给 `br/pkg/encryption/master_key/kms_backend.rs::NewKmsBackend`。
3. `KmsBackend::DecryptWithContext` 先调用 `Provider::Name` 校验加密内容中的厂商元数据，再提取密文 data key。
4. 缓存未命中时，`KmsBackend::with_retry` 调用 `Provider::DecryptDataKey(ctx, ciphertext)`；每次失败后检查 `ctx.is_cancelled()`，取消后不再继续退避重试。
5. AWS/GCP 的 `Provider` 实现把调用委托给各自的 `DecryptDataKeyWithContext`，继而把同一个 `Context` 传给 SDK 客户端。生产适配器同时等待 SDK 请求和 `ctx.token().cancelled()`，先完成者决定结果。
6. 上层把返回的明文 data key 构造成 AES-GCM 后端并缓存；关闭 `AnyBackend` 时，调用链经 `KmsBackend::Close` 到达 `Provider::Close`。

因此，本文件本身没有“执行一次解密”的函数体；它规定了执行链各层之间必须保持的参数、返回值和生命周期协议。

## 数据与状态

`Context` 的唯一状态是 `tokio_util::sync::CancellationToken`。`#[derive(Clone, Default)]` 的关键语义是：`Default` 创建未取消 token，而 `Clone` 共享同一取消状态，不是复制一个独立布尔值。`br/pkg/encryption/master_key/kms_backend_test.rs::test_kms_backend_decrypt_propagates_context_cancellation` 将克隆后的上下文交给假 provider，由假 provider 取消克隆，原上下文随即使重试循环停止，验证了共享传播语义。

取消是单向状态转换：本文件没有 reset、deadline、timeout 或取消原因字段。`Context` 也不保存密钥、客户端、运行时或错误；这些数据属于具体 provider 和上层 backend。

`Provider::DecryptDataKey` 以借用切片接收密文、以拥有的 `Vec<u8>` 返回明文，避免在接口层规定密钥存储方式。`Name` 返回借用字符串，要求实现返回值至少与 `self` 同寿命；现有 AWS/GCP 实现返回静态常量。trait 自身没有声明 `Send`、`Sync` 或 `async`，需要跨线程移动的约束由消费者 `Box<dyn Provider + Send>` 局部添加。

## 依赖与调用关系

直接下游只有 `tokio_util::sync::CancellationToken`。`Context::token` 的实际消费者是：

- `br/pkg/kms/aws.rs::AwsSdkClient::Decrypt`：在 AWS SDK `send()` 与取消 future 之间选择；取消映射成 code 为 `KMS error`、message 为 `context canceled` 的 `AwsDecryptError`。
- `br/pkg/kms/gcp.rs::GcpSdkClient::Decrypt`：在 GCP SDK `send()` 与取消 future 之间选择；取消返回字符串错误 `context canceled`。

`Provider` 的实现方是 `br/pkg/kms/aws.rs::AwsKms<C>` 与 `br/pkg/kms/gcp.rs::GcpKms<C>`。前者的 `Close` 为空操作；后者委托固有方法关闭客户端，并在关闭失败时记录错误。其主要上游是 `br/pkg/encryption/master_key/kms_backend.rs::KmsBackend`，该类型保存 `Box<dyn Provider + Send>`，使用 `Name`、`DecryptDataKey`、`Close` 三个方法。`br/pkg/kms/lib.rs` 是公开重导出入口，`br/pkg/encryption/master_key/Cargo.toml` 以路径依赖接入此 crate。

RustCodeGraph 对 `DecryptDataKey` 给出的关键边包括 `KmsBackend::DecryptWithContext → Provider::DecryptDataKey`、AWS/GCP 的 trait 实现到各自 `DecryptDataKeyWithContext` 的委托，以及 `kms_test.rs::provider_propagates_cancellation_context_to_client → Provider::DecryptDataKey`。

## 错误处理与边界

`Context` 的三个公开操作均不返回错误；`CancellationToken::cancel` 可重复调用，`is_cancelled` 只报告状态。真正的取消结果由消费 token 的具体 provider 转换为其错误表示。

`Provider::DecryptDataKey` 统一使用 `Result<Vec<u8>, String>`，因此接口层不保留结构化 SDK 错误类型。错误分类、上下文包装和可重试性均由实现或调用者决定：AWS 在 `classifyDecryptError` 中归一化服务错误，GCP 包装请求错误并检查 CRC，上层 `KmsBackend` 再增加 `decrypt encrypted key failed` 上下文并实施最多十次的退避策略。

接口没有规定空密文、明文长度、厂商名格式或关闭失败的统一行为；不能仅根据此 trait 宣称这些条件已被验证。现有实现的具体边界必须到 `aws.rs`、`gcp.rs`、`common.rs` 及对应测试中核对。`Close` 无返回值意味着清理失败无法沿 trait 返回，上层也必须显式调用它；本文件没有 `Drop` 兜底。

## 并发与资源生命周期

`CancellationToken` 可安全地在克隆的 `Context` 之间共享取消信号，且 `cancel(&self)` 不要求独占访问。它是本文件唯一的并发原语；这里没有锁、通道、后台任务或线程创建。AWS/GCP 生产客户端各自持有 Tokio runtime，并在同步接口内部 `block_on` 异步请求，这一桥接发生在厂商文件而非本文件。

trait 的方法接收方式表达生命周期约束：解密与读取名称只借用 `&self`，允许 provider 状态在不取得可变引用时处理请求；关闭要求 `&mut self`，由所有者在生命周期末尾发起。现有上层以 `Box<dyn Provider + Send>` 独占 provider，并在 `KmsBackend::Close` 中显式转发关闭。接口没有声明 `Sync`，因此不能据此假定一个 provider 可被多个线程同时共享调用。

敏感数据方面，本文件只短暂借用密文并拥有返回的明文缓冲区；它未实现零化。明文的缓存和后续销毁行为属于 `KmsBackend`/`MemAesGcmBackend`，扩展时不能把 `Context` 误作密钥生命周期容器。

## 与 Go 版本的对应关系

Go 对照文件 `br/pkg/kms/kms.go` 只定义 `Provider`：`DecryptDataKey(context.Context, []byte) ([]byte, error)`、`Name() string`、`Close()`。Rust `Provider` 保留相同的三方法职责，但有以下明确差异：

- Go 直接接受标准库 `context.Context`；Rust 增设本地 `Context`，当前只封装取消 token，不支持 Go context 的 deadline、value 和带 cause 取消。
- Go 用 `error` 保留动态错误链；Rust 当前压缩为 `String`，结构化错误和 cause 不跨 trait 边界。
- Go `Name` 返回拥有的字符串值；Rust 返回借用 `&str`，现有实现使用常量，避免分配。
- Go 接口不在声明处体现可变关闭；Rust `Close(&mut self)` 要求所有者提供可变访问。
- Rust trait 本身不强制 `Send`；上层构造 `KmsBackend` 时额外要求 `Box<dyn Provider + Send>`。

Rust 的 `Context` 是为了在同步 trait 与异步云 SDK 之间复现 Go 取消传播的局部适配，不应被描述为完整的 `context.Context` 等价物。`br/pkg/kms/kms_test.rs` 验证取消能到达注入的 AWS 客户端；`br/pkg/encryption/master_key/kms_backend_test.rs` 验证克隆共享状态并能停止重试。

## 扩展指南

新增 KMS 厂商时，应在独立源文件中实现 `Provider`，并同步处理以下接线点：

1. `DecryptDataKey` 必须把传入的同一个 `Context` 继续交给实际 I/O 层；异步客户端应同时等待请求完成与 `Context::token().cancelled()`，不要在入口处换成新的默认上下文。
2. `Name` 必须返回与加密元数据及 TiKV 侧约定一致的稳定值，否则 `KmsBackend::DecryptWithContext` 会在发起 KMS 请求前拒绝内容。
3. `Close` 必须释放客户端或运行时持有的资源；若底层关闭可能失败，需要明确采用记录、聚合或接口演进策略，因为当前 trait 不能返回关闭错误。
4. 在 `br/pkg/encryption/master_key/master_key.rs::createCloudBackend` 增加配置到 provider 的最小接线，并继续以 `Box<dyn Provider + Send>` 交给 `CreateKmsBackendWithProvider`。
5. 测试逻辑保持在独立文件。至少扩展 `br/pkg/kms/kms_test.rs` 验证取消传播，并在厂商专属 `*_test.rs` 或 `br/pkg/kms/parity_test.rs` 验证名称、请求参数、错误映射和关闭行为；不要把测试嵌入 `kms.rs`。

若要扩展 `Context` 支持 deadline 或取消原因，需同时修改两个 SDK 适配器、`KmsBackend::with_retry` 的停止判定和独立回归测试，并评估与 Go `context.Context` 的可观察语义差异。若把错误从 `String` 改为结构化类型，则是跨 crate 的接口变更，必须同步所有实现、trait 对象调用者和测试假实现。

## 验证依据

- 目标源码：`br/pkg/kms/kms.rs`，确认 `Context` 的四个方法和 `Provider` 的三个方法，无条件编译项或其他状态。
- crate 边界：`br/pkg/kms/lib.rs`（模块声明与公开重导出）、`br/pkg/kms/Cargo.toml`（库入口、`tokio-util` 及云 SDK 依赖）、`br/pkg/encryption/master_key/Cargo.toml`（上层路径依赖）。
- Go 对照：`br/pkg/kms/kms.go`，确认三方法公共契约及 Rust 局部适配差异。
- 实现方：`br/pkg/kms/aws.rs::{AwsSdkClient::Decrypt, impl Provider for AwsKms<C>}` 与 `br/pkg/kms/gcp.rs::{GcpSdkClient::Decrypt, impl Provider for GcpKms<C>}`，确认 token 消费、解密委托、名称和关闭行为。
- 上游链路：`br/pkg/encryption/master_key/master_key.rs::{createCloudBackend, CreateKmsBackendWithProvider}` 与 `br/pkg/encryption/master_key/kms_backend.rs::{KmsBackend::DecryptWithContext, KmsBackend::Close, with_retry}`。
- 独立测试：`br/pkg/kms/kms_test.rs::provider_propagates_cancellation_context_to_client`；`br/pkg/encryption/master_key/kms_backend_test.rs::test_kms_backend_decrypt_propagates_context_cancellation`；`br/pkg/kms/parity_test.rs` 中的 Provider 多态、AWS/GCP 请求与关闭契约测试。
- RustCodeGraph：运行了 `status`、`files --filter br/pkg/kms`、针对 `Provider`/`Context`/`DecryptDataKey` 的 `query`、`node` 与 `explore`；索引显示 7032 个 Rust 文件，并给出 `KmsBackend::DecryptWithContext → Provider::DecryptDataKey`、AWS/GCP 实现委托及取消测试调用边。
- 本任务为纯文档分析，按计划不运行 Cargo；结构检查要求文档存在且固定二级标题恰好为 11 个。
