# `br/pkg/kms/gcp.rs`

## 文件定位

该文件是 `astersql-br-pkg-kms` crate 的 GCP Cloud KMS 解密后端，crate 入口 `br/pkg/kms/lib.rs` 通过 `pub mod gcp` 加载并重导出其公开符号。它位于 BR 主密钥解密链的云厂商适配层：`br/pkg/encryption/master_key/master_key.rs::createCloudBackend` 在厂商为 `gcp` 时调用 `NewGcpKms`，再把结果装箱为统一的 `Provider`，交给 `KmsBackend` 解开数据加密密钥。

文件对照 Go 实现 `br/pkg/kms/gcp.go`，但并非仅有占位逻辑：`GcpSdkClient` 已连接 `google-cloud-kms-v1` 和 `google-cloud-auth`，会读取可选凭证文件并发起真实 Decrypt RPC。`br/pkg/kms/stubs.rs` 只定义配置和可注入客户端 trait；真正的 GCP SDK 适配在本文件中。

## 核心职责

1. 用 `StorageVendorNameGcp = "gcp"` 固定加密元数据中的厂商标识，并通过 `Provider::Name` 暴露给上层校验。
2. 在 `NewGcpKmsWithClient` 中校验 GCP 配置、按 Go `strings.TrimSuffix` 语义只移除一个尾部斜杠，并提取资源名的前四段作为 location。
3. 在 `NewGcpKms` 中创建 Tokio runtime、解析显式凭证 JSON（或使用 Application Default Credentials），构建生产 GCP KMS 客户端。
4. 在 `DecryptDataKeyWithContext` 中为密文附加 CRC32C，调用 KMS Decrypt，并以响应明文 CRC32C 阻止传输损坏的密钥进入后续 AES-GCM 解密。
5. 将取消信号传播到在途 RPC，并在 `Close` 时委托客户端释放资源、记录而不返回关闭错误。

该模块只负责“用 GCP 主密钥解开 data key”，不缓存 data key，也不直接解密备份内容；缓存、重试和内容解密由 `br/pkg/encryption/master_key/kms_backend.rs::KmsBackend::DecryptWithContext` 负责。

## 主要符号

- `StorageVendorNameGcp: &str`：固定为小写 `gcp`，必须与 TiKV 的 `STORAGE_VENDOR_NAME_GCP` 以及上层元数据一致。
- `GcpKms<C: GcpDecryptClient>`：后端状态，持有规范化后的 `MasterKeyKms`、解析出的 `location` 和一个可替换客户端 `C`。泛型边界让生产 SDK 与测试假客户端共享相同逻辑。
- `NewGcpKmsWithClient(config, client)`：依赖注入构造器。缺少 `GcpKms` 配置时报 `GCP config is missing`；KeyId 少于四个 `/` 分段时报 `invalid GCP key id`。
- `GcpSdkClient`：生产适配器，内部持有 `google_cloud_kms_v1::client::KeyManagementService` 与共享的多线程 Tokio `Runtime`。
- `NewGcpKms(&MasterKeyKms)`：生产构造入口。根据凭证 JSON 的 `type` 选择 service account、authorized user、external account 或 impersonated service account builder，然后调用注入构造器完成公共规范化。
- `GcpKms::Name`：返回固定厂商名。
- `GcpKms::DecryptDataKey`：无外部取消上下文的便利入口，创建默认 `Context` 后转调上下文版本。
- `GcpKms::DecryptDataKeyWithContext`：解密主流程；请求失败统一包装为 `gcp kms decrypt request failed: ...`，响应 CRC 不符返回 `response corrupted in-transit`。
- `GcpKms::checkCRC32` / `calculateCRC32C`：可复用的校验与计算薄封装；前者提供包含期望值和实际值的诊断错误。
- `GcpKms::Close`：调用 `GcpDecryptClient::Close`，失败只写 stderr，不改变调用方控制流。
- `impl Provider for GcpKms<C>`：把具体实现接入 `DecryptDataKey`、`Name`、`Close` 三方法的统一厂商接口。
- `crc32c`：手写的反射 Castagnoli CRC32C，使用多项式 `0x82F6_3B78`、全 1 初值和最终取反，对齐 Go `crc32.Checksum(..., crc32.Castagnoli)`。

## 执行流程

配置链从 `br/pkg/task/encryption.rs::parseGcpKmsConfig` 开始：URL 被解析成 `projects/{project}/locations/{location}/keyRings/{ring}/cryptoKeys/{key}`，凭证路径写入 GCP 配置。`createCloudBackend` 将配置转换为本 crate 的 `MasterKeyKms`，调用 `NewGcpKms` 并包装成 `Box<dyn Provider + Send>`。

生产构造分为以下步骤：

1. `NewGcpKms` 先拒绝缺少 GCP 配置的输入，再创建 Tokio runtime。
2. 若 `Credential` 非空，则读取整个 JSON 文件、解析 `type`，构建对应的 `google-cloud-auth` 凭证；为空时保留 SDK 默认凭证发现行为。
3. 在 runtime 上同步等待 `KeyManagementService::builder().build()`。
4. `NewGcpKmsWithClient` 克隆并规范化配置，只剥离一个末尾 `/`，至少要求四段，然后保存前四段 location 和客户端。

一次解密按如下顺序运行：

1. `KmsBackend::DecryptWithContext` 先核对密文元数据中的厂商名和 ciphertext key，并在缓存未命中时调用 `Provider::DecryptDataKey`。
2. `DecryptDataKeyWithContext` 对密文计算 Castagnoli CRC32C，把完整 KeyId、密文和 CRC 交给客户端。
3. `GcpSdkClient::Decrypt` 用 SDK builder 设置 `name`、`ciphertext`、`ciphertext_crc32c`，并以 `tokio::select!` 在 RPC 完成与 `Context` 取消之间竞速。
4. RPC 成功后把 SDK 响应投影为 `GcpDecryptResponse`；上层重新计算明文 CRC，与响应值严格比较。
5. 只有 CRC 相等才返回明文 data key；随后 `KmsBackend` 将其校验为 AES-GCM-256 密钥、缓存对应后端并解密内容。

## 数据与状态

`GcpKms` 在构造后保存一份拥有所有权的配置副本，因此 `NewGcpKms` 调用者之后修改原配置不会改变该实例。构造器会直接修改这份副本中的 `KeyId` 以移除一个尾斜杠。`location` 是 KeyId 前四段拼接出的字符串；当前文件只保存并公开该字段，实际 Decrypt 请求仍使用完整 `config.KeyId`，生产逻辑没有再次消费 `location`。

`GcpSdkClient` 的 SDK client 和 `Arc<Runtime>` 随实例长期存活。每次解密只产生请求、响应和局部 CRC，没有模块级可变状态，也没有在本文件内缓存明文密钥。敏感明文字节以 `Vec<u8>` 返回；本文件没有显式清零，生命周期由上层所有权和缓存决定。

`GcpDecryptResponse::PlaintextCrc32C` 是必需的 `i64` 视图。SDK 响应若没有该可选字段，适配器以 `0` 代替，通常会使后续完整性检查失败，而不是静默跳过校验。

## 依赖与调用关系

上游关系：

- `br/pkg/task/encryption.rs::parseGcpKmsConfig` 生成 GCP KMS 配置。
- `br/pkg/encryption/master_key/master_key.rs::createCloudBackend` 调用 `NewGcpKms`，并通过 `CreateKmsBackendWithProvider` 装入 `KmsBackend`。
- `br/pkg/encryption/master_key/kms_backend.rs::DecryptWithContext` 调用 `Provider::Name` 和 `Provider::DecryptDataKey`；`KmsBackend::Close` 调用 `Provider::Close`。
- `br/pkg/kms/gcp_test.rs` 与 `parity_test.rs` 通过 `NewGcpKmsWithClient` 注入假客户端，验证公共契约且不访问云服务。

下游关系：

- `crate::kms::{Context, Provider}` 提供取消语义和统一后端接口。
- `crate::stubs::{MasterKeyKms, GcpDecryptClient}` 定义配置与 SDK 隔离边界。
- Cargo 依赖 `google-cloud-kms-v1` 发起 RPC，`google-cloud-auth` 构建凭证，`serde_json` 解析凭证文件，`bytes` 复制请求密文，`tokio`/`tokio-util` 提供 runtime 与取消通知。

RustCodeGraph 显示目标文件被 `gcp_test.rs` 和 `parity_test.rs` 直接使用；生产侧通过 crate 根重导出和 `Provider` trait 间接连接，不能只按对 `gcp.rs` 的文本引用判断其运行位置。

## 错误处理与边界

构造阶段所有错误均为 `String`：缺配置、KeyId 分段不足、runtime 创建失败、凭证文件读取/JSON 解析失败、凭证类型缺失或不支持、认证 builder 失败、KMS client 构建失败都会立即终止。除“至少四段”外，`NewGcpKmsWithClient` 不验证 `projects`/`locations` 标签或完整 keyRing/cryptoKey 结构；正常生产路径依赖更早的 URL 解析保证格式。

解密阶段将所有客户端错误加上稳定前缀，便于上层重试时保留上下文；CRC 不符则不返回明文。`KmsBackend` 位于本文件之外，当前会把这些错误纳入最多十次的重试聚合，因此本文件本身不做退避或错误分类。

取消分支返回文本 `context canceled`，随后仍被包装为 `gcp kms decrypt request failed: context canceled`。`tokio::select!` 取消 RPC future 的等待，但本文件不承诺远端已经停止处理请求。

`Close` 的契约是尽力关闭并记录错误，不向调用者传播。需要特别注意：当前生产 `GcpSdkClient::Close` 直接返回成功，是空操作；Go 版本会调用 `KeyManagementClient.Close()`。这是当前实现事实，不能把 Rust 路径描述为已经显式关闭底层 SDK 连接。

## 并发与资源生命周期

`GcpDecryptClient: Send` 以及上层的 `Box<dyn Provider + Send>` 允许后端跨线程转移，但 trait 没有要求 `Sync`；应避免据此声称同一 provider 可被任意并发共享。`Decrypt` 只借用 `&self`，具体客户端必须自行满足其内部并发约束。

每个生产 provider 创建一套多线程 Tokio runtime，并在同步方法中用 `Runtime::block_on` 驱动异步 SDK。调用方应把它视为同步阻塞接口；在已有 Tokio runtime 的异步任务中直接调用存在嵌套 runtime/blocking 的集成风险，应由调用层隔离到合适的阻塞执行环境。

`Arc<Runtime>` 确保 runtime 与 SDK client 至少共同存活到 `GcpSdkClient` 被销毁。显式 `Close` 只需要 `&mut self`，保证调用期间没有其他安全借用；测试客户端可借此释放或记录资源。生产 Close 当前没有主动网络资源清理，最终释放依赖 Rust 字段析构。

## 与 Go 版本的对应关系

一致点包括：厂商名为 `gcp`；缺少 GCP 配置立即失败；只去掉一个尾斜杠；location 取 KeyId 前四段；显式凭证文件与默认凭证二选一；请求携带密文 CRC32C；响应明文 CRC32C 必须匹配；RPC 错误带 `gcp kms decrypt request failed` 上下文；关闭错误只记录不返回。

Rust 用泛型 `GcpDecryptClient` 代替 Go 结构体中固定的 `*kms.KeyManagementClient`，从而可以在独立测试中记录请求、注入错误。Go 使用标准库 CRC 表，Rust 以逐位算法产生相同 Castagnoli 结果。Go 直接接受 `context.Context`，Rust 用包含 `CancellationToken` 的 `Context`，SDK 适配器通过 `tokio::select!` 实现取消传播。

差异和迁移边界包括：Go 构造器假设 KeyId 已在参数解析阶段验证，直接切前四段；Rust 增加了分段数量检查以避免越界。Rust 显式识别四类凭证 JSON，未知类型会在本地拒绝；Go 将文件交给 Google option 层解析。Rust SDK 的可选响应 CRC 缺失时视为 `0`。最重要的是 Rust 生产 Close 当前为空操作，而 Go 会真实关闭 client。

Go 同目录没有独立 `gcp_test.go`；本任务能核对的 Go 行为依据是 `br/pkg/kms/gcp.go` 本身，边界回归主要由 Rust 的 `gcp_test.rs` 和 `parity_test.rs` 固化。

## 扩展指南

- 新增请求字段、重试可观察信息或 SDK 行为时，优先扩展 `GcpDecryptClient` 和 `GcpSdkClient::Decrypt`，同时更新 `RecordingGcpClient`，避免在通用业务路径中写云 SDK 专用分支。
- 修改 KeyId 规范化或 location 规则时，应同时检查 `NewGcpKmsWithClient`、`parseGcpKmsConfig` 和 Go `NewGcpKms`；保留“只去一个尾斜杠”的兼容行为，除非有明确迁移方案。
- 修改 CRC 实现必须以 Go Castagnoli 结果为基准，并同步覆盖空输入、已知向量、请求 CRC 和响应损坏。不能换成常见但不兼容的 IEEE CRC32。
- 若实现生产资源的显式关闭，应在 `GcpSdkClient::Close` 接入 SDK 支持的关闭机制，并保留“记录但不传播”这一 Provider 契约；同步更新 `br/pkg/kms/gcp_test.rs`。
- 若增加凭证类型或 ADC 配置，修改 `NewGcpKms` 的凭证分派，并增加不访问真实云服务的构造边界测试。凭证解析错误不得输出 JSON 内容或密钥材料。
- 回归测试继续放在独立文件 `br/pkg/kms/gcp_test.rs` 或跨语言契约文件 `br/pkg/kms/parity_test.rs`，不要把测试嵌入生产源文件。
- 性能改动要关注每次解密的逐位 CRC 成本和同步 `block_on`；若替换 CRC 实现或 runtime 所有权模型，需保持输出和取消语义不变。

## 验证依据

- RustCodeGraph：`status` 确认索引包含 7032 个 Rust 文件；`files --filter br/pkg/kms` 确认目标、模块入口、Go 对照和独立测试；`node --file br/pkg/kms/gcp.rs` 阅读全部 224 行并列出其两个直接测试使用方；`explore 'br/pkg/kms/gcp.rs GCP KMS symbols callers callees'` 核对 `NewGcpKmsWithClient`、Decrypt/Close、CRC 与 `Provider` 调用关系。
- 生产源码：`br/pkg/kms/gcp.rs`、`br/pkg/kms/kms.rs`、`br/pkg/kms/stubs.rs`、`br/pkg/kms/lib.rs`、`br/pkg/encryption/master_key/master_key.rs`、`br/pkg/encryption/master_key/kms_backend.rs`、`br/pkg/task/encryption.rs`。
- crate 边界：`br/pkg/kms/Cargo.toml`，确认库入口为 `lib.rs`，并声明 Google KMS/Auth、Tokio、bytes 与 serde_json 依赖。
- Go 对照：`br/pkg/kms/gcp.go`，核对构造、KeyId/location、CRC、错误包装和关闭语义；同目录未发现 `gcp_test.go`。
- Rust 测试：`br/pkg/kms/gcp_test.rs::close_error_is_reported_like_go` 验证关闭错误被记录且不导致失败；`br/pkg/kms/parity_test.rs::{gcp_key_id_trims_exactly_one_trailing_slash,gcp_request_crc_response_crc_and_close_match_go,gcp_decrypt_errors_match_go,go_rust_public_contract_matches}` 验证规范化、location、请求与响应 CRC、错误文案、厂商名和关闭委托。
- 本任务是只读行为分析加文档输出，按计划不运行 Cargo；结构验证命令及退出码在任务交付时记录。
