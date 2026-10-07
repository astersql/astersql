# `br/pkg/kms/aws.rs`

## 文件定位

`br/pkg/kms/aws.rs` 是 `astersql-br-pkg-kms` crate 的 AWS KMS 后端实现。crate 入口 `br/pkg/kms/lib.rs` 以 `pub mod aws` 装载本文件并通过 `pub use aws::*` 暴露其公共符号；`br/pkg/kms/Cargo.toml` 将该 crate 定义为库，并声明 `aws-config`、`aws-sdk-kms`、`aws-smithy-http-client`、`hyper-rustls`、`tokio` 等生产依赖。

在完整解密链中，`br/pkg/encryption/master_key/master_key.rs::createCloudBackend` 遇到 vendor `"aws"` 时调用 `NewAwsKms`，将结果作为 `Box<dyn Provider + Send>` 交给 `CreateKmsBackendWithProvider`。随后 `br/pkg/encryption/master_key/kms_backend.rs::KmsBackend::DecryptWithContext` 校验密文元数据中的厂商名、提取密文 data key，并经 `Provider::DecryptDataKey` 进入本文件。因此，本文件负责“用 AWS CMK 解开 data key”，不负责使用该 data key 解密业务内容；后者由上层 `MemAesGcmBackend` 完成。

## 核心职责

- `EncryptionVendorNameAwsKms` 固定返回元数据协议使用的 `"AWS"`。它与上层用于选择实现的配置 vendor `"aws"` 大小写不同：前者参与加密内容元数据校验，后者见 `master_key.rs::StorageVendorNameAWS`。
- `NewAwsKms` 从 `MasterKeyKms` 构造 AWS SDK 配置、TLS HTTP 客户端和 KMS client；支持 region、可选自定义 endpoint，以及 access key 与 secret key 同时存在时的静态凭证覆盖。
- `AwsSdkClient::Decrypt` 把同步的本地 client trait 适配到异步 AWS SDK `Decrypt` 请求，并在请求完成与 `Context` 取消之间竞速。
- `AwsKms::DecryptDataKeyWithContext` 将密文 data key 和 `currentKeyID` 传给可注入的 `AwsDecryptClient`，成功时透传明文，失败时交给 `classifyDecryptError` 统一成与 Go 版本一致的错误前缀。
- `Provider for AwsKms<C>` 把实现接入上层统一 KMS 抽象；`Close` 保持无操作，因为 AWS client 没有需要调用方显式关闭的资源。

## 主要符号

- `pub const EncryptionVendorNameAwsKms: &str = "AWS"`：KMS 元数据厂商标识，注释明确要求与 TiKV 的 `ENCRYPTION_VENDOR_NAME_AWS_KMS` 完全一致。
- `pub struct AwsKms<C: AwsDecryptClient>`：通用后端状态，持有注入客户端 `client`、当前 CMK 标识 `currentKeyID`、`region` 和 `endpoint`。泛型边界使测试可注入确定性的假客户端。
- `pub fn NewAwsKmsWithClient<C>(...) -> Result<AwsKms<C>, String>`：轻量构造器，复制 `KeyId`、`Region`、`Endpoint` 并接收现成 client。它不联网、不校验凭证，也不会因 access/secret 仅提供一项而报错。
- `pub struct AwsSdkClient`：生产适配器，私有持有 `aws_sdk_kms::Client` 和共享的 `Arc<tokio::runtime::Runtime>`；外部只能通过 `AwsDecryptClient` 使用它。
- `impl AwsDecryptClient for AwsSdkClient::Decrypt`：建立包含 `ciphertext_blob` 与 `key_id` 的 AWS 请求，返回 `plaintext` 字节；AWS SDK 错误被压缩为 `AwsDecryptError { code, message }`。
- `pub fn NewAwsKms(...) -> Result<AwsKms<AwsSdkClient>, String>`：生产构造器，创建多线程 Tokio runtime、rustls connector、AWS 默认配置链和 KMS client。
- `AwsKms::{Name, DecryptDataKey, DecryptDataKeyWithContext, Close}`：固有方法。无 context 的 `DecryptDataKey` 使用默认未取消 context；带 context 版本是实际委托点。
- `impl Provider for AwsKms<C>`：上层多态入口，`DecryptDataKey` 显式转发到固有的 `DecryptDataKeyWithContext`，避免丢失取消信号。
- `pub fn classifyDecryptError(&AwsDecryptError) -> String`：按 AWS 错误码映射稳定的用户可见错误前缀。

本文件没有条件编译项、宏定义或模块级可变状态。公开 API 主要是常量、两个结构体、两个构造器、`AwsKms` 方法和错误分类函数；SDK client 的字段保持私有。

## 执行流程

1. `master_key.rs::createCloudBackend` 把 protobuf 风格配置复制成该 crate 的 `MasterKeyKms`，并为 AWS 分支调用 `NewAwsKms`。
2. `NewAwsKms` 创建 Tokio runtime。创建失败立即返回带 `failed to load AWS config` 前缀的字符串错误。
3. 构造器使用编译内置的 webpki 根证书创建同时允许 HTTPS/HTTP、HTTP/1 和 HTTP/2 的 connector，再交给 AWS Smithy HTTP client。允许 HTTP 是为了保持自定义 Go endpoint 的兼容性，正常 HTTPS 仍执行证书校验。
4. AWS 配置 loader 固定使用 `masterKeyConfig.Region`。只有 `AwsKms` 子配置存在且 access key、secret key 都非空时，才安装静态凭证；否则保留 AWS 默认凭证链。
5. loader 在所持 runtime 上同步等待配置加载；非空 `Endpoint` 被写入 KMS service config。随后通过 `NewAwsKmsWithClient` 保存 client 与配置字段。
6. 上层解密时，`KmsBackend::DecryptWithContext` 先校验 metadata vendor 必须等于 `AwsKms::Name()` 返回的 `"AWS"`，然后把 metadata 中的密文 key 传给 trait 方法。
7. `Provider::DecryptDataKey` 转发至 `AwsKms::DecryptDataKeyWithContext`。后者调用 `client.Decrypt(ctx, dataKey, currentKeyID)`。
8. 生产 client 使用 `tokio::select!`：AWS `send()` 先完成则处理服务响应；取消 token 先完成则生成 `code = "KMS error"`、`message = "context canceled"` 的本地错误。
9. 成功响应取 `output.plaintext`；响应未带 plaintext 时按当前实现返回空向量。失败响应的 service code 与完整 SDK 错误文本进入 `AwsDecryptError`，再由 `classifyDecryptError` 生成字符串错误。
10. 上层 `KmsBackend` 对失败执行自己的重试策略；成功得到的明文必须能构造成 AES-GCM-256 key，之后才缓存并用于内容解密。

## 数据与状态

`AwsKms<C>` 的四个字段在构造后不再修改。`currentKeyID` 是每次 AWS `Decrypt` 请求的 `KeyId`；`region` 与 `endpoint` 同时保存在对象中，便于保持 Go 结构对齐，但实际 SDK 配置在构造阶段已固化到 `AwsSdkClient.client`。`NewAwsKmsWithClient` 直接克隆配置字符串，因此后续修改原始 `MasterKeyKms` 不会影响现有 provider。

`AwsSdkClient` 通过 `Arc<Runtime>` 保持 runtime 活到 client 结束，并允许结构在需要时共享 runtime 所有权。每次调用只创建一次请求和局部结果，不保存明文、密文或错误状态。本文件也没有缓存；密文 data key 与派生解密后端的缓存位于 `KmsBackend.cached: Mutex<Option<CachedKeys>>`。

配置结构与 client trait 来自 `br/pkg/kms/stubs.rs`。这里的 `MasterKeyKms` 是本地 protobuf 边界替身，`AwsDecryptClient: Send` 是可注入接口；但 `AwsSdkClient` 已连接真实 AWS SDK，所以不能把整个 AWS 后端描述为模拟实现。

## 依赖与调用关系

上游生产调用链为：

`CreateBackend` → `createCloudBackend` → `NewAwsKms` → `CreateKmsBackendWithProvider` → `KmsBackend::DecryptWithContext` → `Provider::DecryptDataKey` → `AwsKms::DecryptDataKeyWithContext` → `AwsDecryptClient::Decrypt`。

直接内部依赖包括：

- `crate::kms::{Context, Provider}`：提供取消上下文和统一 provider trait。
- `crate::stubs::{AwsDecryptClient, AwsDecryptError, MasterKeyKms}`：提供配置、注入边界及可分类错误载体。
- `aws_config`、`aws_credential_types`、`aws_types`：默认配置链、静态凭证和 region。
- `aws_sdk_kms`：KMS client、Decrypt builder、密文字节 Blob 与 service error metadata。
- `aws_smithy_http_client`、`hyper_rustls`：使用内置根证书的 HTTP(S) transport。
- `tokio`、`tokio-util`：同步边界内部的异步 runtime 与取消 token。

RustCodeGraph 将 `br/pkg/kms/aws.rs` 的直接使用文件识别为 `br/pkg/kms/stubs.rs`、`br/pkg/kms/kms_test.rs` 和 `br/pkg/kms/parity_test.rs`，并显示 `NewAwsKms` 的实际生产上游位于 `br/pkg/encryption/master_key/master_key.rs`。Cargo 层面，`br/pkg/encryption/master_key/Cargo.toml` 以路径依赖 `../../kms` 引用本 crate。

## 错误处理与边界

- runtime 创建失败：`NewAwsKms` 返回 `failed to load AWS config: ...`。AWS loader 的异步加载在当前 API 中不返回 `Result`，因此本函数没有单独的 loader 错误分支。
- 静态凭证：只有 access key 和 secret key 均非空才覆盖默认链；缺一项不是本层配置错误，而是回落默认凭证发现。这与 `aws.go::NewAwsKms` 的宽松策略一致。
- endpoint：空字符串不覆盖 SDK endpoint；非空值原样传入，构造器不在本地验证 URL 或 TLS 安全性。
- 取消：`AwsSdkClient::Decrypt` 观察 `Context::token()`；取消被表达为普通 `"KMS error: context canceled"`，上层可收到该错误。`kms_test.rs::provider_propagates_cancellation_context_to_client` 固定了 trait 层的传播行为。
- AWS service code：`NotFoundException` 和 `InvalidKeyUsageException` 映射为 `wrong master key`；`DependencyTimeoutException` 映射为 `API timeout`；`KMSInternalException` 映射为 `API internal error`；其余映射为 `KMS error`。
- transport/dispatch 类错误：适配器用 `"KMS error"` 哨兵 code。分类函数对此只拼接 `err.message`，避免生成重复的 `KMS error: KMS error: ...`；`aws_test.rs::non_service_sdk_error_is_annotated_once_like_go` 专门验证该边界。
- 成功但缺少 plaintext：当前使用 `unwrap_or_default()` 返回空字节，不在本层报协议错误。上层通常会在 `NewPlainKey(...AesGcm256)` 的长度检查处拒绝它；若单独调用本 provider，则会直接观察到空结果。
- `classifyDecryptError` 只根据字符串 code 分类，不保留结构化 SDK error、error source 或重试标记。上层重试逻辑目前对所有 provider 字符串错误采用相同尝试策略。

## 并发与资源生命周期

`AwsDecryptClient` 要求 `Send`，而生产入口又将 provider 放入 `Box<dyn Provider + Send>`。本文件没有内部锁或可变共享状态；`DecryptDataKeyWithContext` 只借用 `&self`，并发能力最终取决于具体 client 是否满足调用方的共享方式。当前 `Provider` trait 本身未声明 `Sync`，因此不要仅凭 AWS SDK client 通常可共享就假定 trait object 能跨线程并发借用。

`AwsSdkClient` 在 `NewAwsKms` 时创建并拥有一个 Tokio runtime。同步 `Decrypt` 使用该 runtime 的 `block_on` 驱动单次异步请求；调用结束后请求局部值释放，而 runtime 随 `AwsSdkClient` 生命周期保留。取消 token 由调用方的 `Context` 持有，`tokio::select!` 在取消分支胜出时丢弃请求 future。

`AwsKms::Close` 和 `Provider::Close` 都是空实现，与 `aws.go::Close` 的“不需要手工关闭”约定一致。没有后台任务、通道、事务或显式 socket 关闭协议；client 与 runtime 依靠 Rust drop 回收。修改生命周期代码时应特别验证在 Tokio runtime 内再次调用同步 `block_on` 的使用约束，以及并发调用同一 provider 的目标是否需要提升 trait 边界。

## 与 Go 版本的对应关系

`br/pkg/kms/aws.go` 是直接语义基准。两端都保存 client、key id、region、endpoint；都把 `"AWS"` 作为厂商名；都只在 access key 与 secret key 同时存在时使用静态凭证；都向 Decrypt 请求传入密文字节和 key id；都无需显式 Close；错误码到前缀的四类映射一致。

主要实现差异如下：

- Go 的 `*kms.Client` 直接接受 `context.Context`；Rust 用 `AwsDecryptClient` 隔离 SDK，并以本地 `Context`/`CancellationToken` 传递取消，再由 `AwsSdkClient` 用 `tokio::select!` 适配。
- Go 通过 `config.LoadDefaultConfig` 返回配置加载错误；Rust AWS SDK 的 loader 在 `block_on(loader.load())` 处直接生成配置，当前显式可失败步骤是 runtime 创建。
- Go 使用 SDK 默认 HTTP transport；Rust 显式构造 webpki roots 的 rustls transport，并为兼容自定义测试/服务 endpoint 同时允许 HTTP。
- Go `classifyDecryptError` 同时通过 Smithy `APIError` code 和具体 error 类型做 `errors.As`；Rust 适配层已把 SDK 错误归一为 code/message，所以分类函数只匹配 code。
- Go 成功时直接返回 `result.Plaintext`；Rust 对缺失的可选 plaintext 使用空向量默认值。

同目录没有 `aws_test.go`，因此没有可读取的 Go AWS 单测。Rust 侧以 `aws_test.rs`、`kms_test.rs` 和 `parity_test.rs` 固化当前 Go 对齐语义；其中 parity 测试验证配置字段复制、请求中的 ciphertext/key id、明文透传、所有错误类别、厂商名与 `Provider` 多态调用。

## 扩展指南

- 新增 AWS 请求参数（例如 encryption context 或 grant token）时，先扩展 `AwsDecryptClient::Decrypt` 的边界，再同步修改 `AwsSdkClient::Decrypt`、所有假 client 和 `parity_test.rs` 的请求记录断言；同时核对 `aws.go::DecryptDataKey`，避免 Rust 独有语义漂移。
- 调整认证规则时修改 `NewAwsKms`，并保留“凭证必须成对才覆盖默认链”的兼容行为，除非 Go 版本也改变。测试应放在独立的 `aws_test.rs` 或 `parity_test.rs`，不要内嵌进生产文件。
- 新增错误类别时修改 `classifyDecryptError`，至少覆盖 service code、未知 code 与非 service/transport 错误，注意避免重复前缀。若上层需要结构化重试判断，应先重新设计当前 `Result<_, String>` 边界，而不是解析展示字符串。
- 改变 endpoint/TLS 行为时重点审查 `NewAwsKms` 的 connector 和 `endpoint_url`，兼顾 AWS HTTPS、私有 KMS 兼容 endpoint 与明文 HTTP 的安全风险。
- 改变取消或异步模型时同时检查 `Context`、`AwsSdkClient` 所有 runtime、`KmsBackend::with_retry` 的取消检查和 sleep 行为；不要只修改 `tokio::select!`。
- 若增加需释放的 client 资源，必须同时实现固有 `Close` 与 trait `Provider::Close`，并补充上层 `KmsBackend::Close` 链路测试。目前空 Close 是明确契约，不代表未来资源可被忽略。
- 性能上，runtime 和 SDK client 应继续按 provider 生命周期复用；不要在每次 `DecryptDataKey` 中重新加载配置或建立 client。

## 验证依据

- RustCodeGraph 索引状态：项目索引包含 7,032 个 Rust 文件；读取 `br/pkg/kms/aws.rs` 全部 175 行，识别 19 个符号，并核对 `NewAwsKmsWithClient`、`NewAwsKms`、`DecryptDataKeyWithContext`、`classifyDecryptError` 等入口及调用关系。
- 生产源码：`br/pkg/kms/aws.rs`；统一 trait：`br/pkg/kms/kms.rs`；配置与 client 边界：`br/pkg/kms/stubs.rs`；crate 入口：`br/pkg/kms/lib.rs`。
- crate/依赖证据：`br/pkg/kms/Cargo.toml`；上游路径依赖：`br/pkg/encryption/master_key/Cargo.toml`。
- 应用调用链：`br/pkg/encryption/master_key/master_key.rs::createCloudBackend` 与 `CreateKmsBackendWithProvider`；消费和重试：`br/pkg/encryption/master_key/kms_backend.rs::KmsBackend::DecryptWithContext`、`with_retry`。
- Go 对照：`br/pkg/kms/aws.go`；上层 Go 接线：`br/pkg/encryption/master_key/master_key.go::createCloudBackend`。检索确认同目录不存在 `aws_test.go`。
- Rust 独立测试：`br/pkg/kms/aws_test.rs::non_service_sdk_error_is_annotated_once_like_go`、`br/pkg/kms/kms_test.rs::provider_propagates_cancellation_context_to_client`、`br/pkg/kms/parity_test.rs::aws_constructor_request_and_all_error_classes_match_go` 与 `go_rust_public_contract_matches`。
- 本任务是纯文档分析，按计划不运行 Cargo；交付前使用任务指定命令验证文档存在且恰有 11 个固定二级章节，并人工复核未把测试替身误写为生产实现、未声称存在 Go AWS 单测。
