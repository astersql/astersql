# `br/pkg/kms/lib.rs`

## 文件定位

[`lib.rs`](./lib.rs) 是 Cargo 包 `astersql-br-pkg-kms` 的 crate 根。`br/pkg/kms/Cargo.toml` 通过 `[lib] path = "lib.rs"` 指向它，仓库根 `Cargo.toml` 又把 `br/pkg/kms` 列为 workspace member。该文件不执行 KMS 请求，也不保存运行时状态；它负责声明同目录的五个生产模块、挂接四个独立测试模块，并把生产模块的公开项统一重导出到 crate 根。

直接生产调用方是 `br/pkg/encryption/master_key` crate：其 `Cargo.toml` 以路径依赖引入本 crate，`master_key.rs` 从 crate 根导入 `MasterKeyKms`、`NewAwsKms`、`NewGcpKms` 和 `Provider`，再把厂商实现装配进上层 `KmsBackend`。因此本文件位于“主密钥后端装配”与“具体云 KMS/密钥类型实现”之间，是 API 门面而非业务实现。

## 核心职责

1. 用 `#[path = "..."] pub mod ...` 固定模块边界：`stubs` 提供配置结构和可注入客户端 trait，`common` 提供密文/明文密钥类型，`kms` 定义统一 Provider 接口，`aws`、`gcp` 提供厂商后端。
2. 用 `pub use aws::*`、`pub use common::*`、`pub use gcp::*`、`pub use kms::*`、`pub use stubs::*` 建立扁平公共 API，使上游无需依赖子模块路径。
3. 仅在 `cfg(test)` 下引入 `parity_test.rs`、`aws_test.rs`、`gcp_test.rs`、`kms_test.rs`，保持生产源码与测试逻辑分文件。
4. 在 crate 级允许迁移期常见的命名和未使用告警。这里的 Go 风格公开名称（例如 `NewAwsKms`、`DecryptDataKey`）是兼容面的一部分；`#![allow(...)]` 会降低编译器对死代码和未使用项的提示强度，不能视为实现完整性的证明。

## 主要符号

本文件没有常量、结构体、trait、函数或 `impl`，公开面全部来自模块声明与 glob 重导出：

- `pub mod stubs` / `pub use stubs::*`：公开 `MasterKeyKms`、`AwsKmsConfig`、`GcpKmsConfig`、`AwsDecryptClient`、`GcpDecryptClient` 及请求/响应错误载体。配置类型是本 crate 的本地边界类型，不是直接重导出的 protobuf 类型。
- `pub mod common` / `pub use common::*`：公开 `EncryptedKey`、`PlainKey`、开放整数包装 `CryptographyType`，以及 `NewEncryptedKey`、`NewPlainKey`。构造函数承载空密文和 AES-GCM-256 精确 32 字节校验。
- `pub mod kms` / `pub use kms::*`：公开带 `CancellationToken` 的 `Context` 和 `Provider` trait；trait 约束解密、厂商名查询和资源关闭三项能力。
- `pub mod aws` / `pub use aws::*`：公开 `AwsKms<C>`、`AwsSdkClient`、`NewAwsKms`、注入式 `NewAwsKmsWithClient`、`classifyDecryptError` 与厂商名 `EncryptionVendorNameAwsKms`。
- `pub mod gcp` / `pub use gcp::*`：公开 `GcpKms<C>`、`GcpSdkClient`、`NewGcpKms`、注入式 `NewGcpKmsWithClient`、`crc32c` 与 `StorageVendorNameGcp`。
- 四个 `mod *_test` 都是私有且仅测试构建可见，不扩大生产 API。

## 执行流程

`lib.rs` 自身的“执行”发生在编译期：编译器依次纳入 `stubs.rs`、`common.rs`、`kms.rs`、`aws.rs`、`gcp.rs`，测试构建再纳入四个独立测试文件，最后由五条 `pub use` 形成 crate 根命名空间。运行时主链由被重导出的实现承担：

1. `br/pkg/encryption/master_key/master_key.rs::createCloudBackend` 把上层配置复制为本 crate 的 `MasterKeyKms`，按 `aws` 或 `gcp` 调用 `NewAwsKms` / `NewGcpKms`，并装箱为 `Box<dyn Provider + Send>`。
2. `NewAwsKms` 建立 Tokio runtime、AWS 配置和 SDK client；仅在 access key 与 secret key 同时非空时采用静态凭证，并应用可选 endpoint。`NewGcpKms` 建立 runtime 和 GCP client；显式凭证路径存在时读取 JSON，并按 credential `type` 构造凭证。
3. `br/pkg/encryption/master_key/kms_backend.rs::DecryptWithContext` 先校验内容元数据中的 KMS vendor 和密文 data key；缓存未命中时，通过 `Provider::DecryptDataKey` 重试解密，再用 `NewPlainKey(..., CryptographyTypeAesGcm256)` 强制校验 32 字节明文密钥，建立内存 AES-GCM backend 并缓存。
4. AWS 实现把密文和 key ID 交给 SDK 并分类错误；GCP 实现给请求附带密文 CRC32C，并在返回明文前校验响应 CRC32C。上层关闭 backend 时，`Provider::Close` 传递到厂商实现。

## 数据与状态

门面本身没有全局变量、可变静态数据或实例状态。公开数据的所有权在子模块中：`EncryptedKey(Vec<u8>)` 和 `PlainKey { tag, key }` 拥有密钥字节；`MasterKeyKms` 拥有 key ID、region、endpoint 和可选厂商配置；`AwsKms<C>`、`GcpKms<C>` 拥有各自 client 与配置快照。生产 SDK 适配器以 `Arc<tokio::runtime::Runtime>` 保持同步 Provider API 背后的异步 runtime 存活。

`Context` 可克隆，克隆共享 Tokio `CancellationToken` 的取消状态。KMS crate 不缓存解密结果；缓存属于上游 `KmsBackend.cached: Mutex<Option<CachedKeys>>`，以密文 key 的字节相等性判定命中。`GcpKms::location` 由规范化后的 key ID 前四段生成；当前解密请求使用完整 `config.KeyId`，`location` 是保留的资源路径状态。

## 依赖与调用关系

- 上游：`br/pkg/encryption/master_key/master_key.rs::createCloudBackend` 调用生产构造器并依赖 `Provider`；`kms_backend.rs::DecryptWithContext` 调用 `Provider::{Name, DecryptDataKey, Close}` 以及 `NewEncryptedKey`、`NewPlainKey`。
- crate 内部：`aws.rs`、`gcp.rs` 同时依赖 `kms::{Context, Provider}` 与 `stubs` 的配置/客户端边界；`gcp.rs` 还公开本地 `crc32c`；`common.rs` 不依赖云 SDK。
- 外部依赖：`br/pkg/kms/Cargo.toml` 声明 AWS config/credential/KMS/HTTP client、Google auth/KMS、`hyper-rustls`、`serde_json`、`bytes`、`tokio` 和 `tokio-util`。Cargo 文件没有 feature 表，因此这些依赖不是由本 crate 自定义 feature 条件选择的。
- 图证据：RustCodeGraph 将 `NewAwsKmsWithClient` 的调用方识别为 `NewAwsKms` 及 KMS 测试；将 `NewPlainKey` 的生产调用方识别为 `kms_backend.rs::DecryptWithContext`。对 `lib.rs` 的图索引只记录 crate 文件级关系，glob re-export 不产生运行时调用边。

## 错误处理与边界

门面不捕获或改写错误；错误语义来自被重导出的构造器和 Provider 实现。`NewEncryptedKey` 拒绝空输入，`NewPlainKey` 对 AES-GCM-256 要求恰好 32 字节；未知 `CryptographyType` 与 Go 的 default 分支一致，目标长度为 0，因而不施加长度限制。

AWS 配置加载/runtime 创建失败使用 `failed to load AWS config` 前缀；服务错误按错误码映射为 `wrong master key`、`API timeout`、`API internal error` 或通用 `KMS error`。GCP 缺少配置、非法短 key ID、凭证文件读取/解析、未知凭证类型和 client 创建均显式返回错误；解密传输错误添加 `gcp kms decrypt request failed`，响应 CRC 不匹配返回 `response corrupted in-transit`。GCP 构造只移除一个尾斜杠，与 Go `strings.TrimSuffix` 一致。

需要特别区分两类边界：`stubs.rs` 的配置结构和客户端 trait 是隔离 protobuf/SDK并支持依赖注入的本地适配层，但 `NewAwsKms` 与 `NewGcpKms` 已接入真实 SDK；不能把整个 crate 描述为“仅桩实现”。另一方面，Azure 不由本 crate 提供，上游 `createCloudBackend` 明确返回 `not implemented Azure KMS`。

## 并发与资源生命周期

`lib.rs` 不启动线程、任务或网络连接。生产 AWS/GCP 构造器各自创建 Tokio multi-thread runtime，并把它与 SDK client 一起保存在适配器中；同步 `DecryptDataKey` 使用 `runtime.block_on` 驱动异步请求，并用 `tokio::select!` 在请求与 `Context` 取消之间竞速。`Context` 的克隆共享取消信号，因此上层可终止进行中的云请求。

`AwsKms::Close` 是空操作，与 Go AWS client 无需显式关闭的约定一致。`GcpKms::Close` 委托 `GcpDecryptClient::Close`；失败只写入 stderr，不返回给调用者。上层 `KmsBackend::Close` 负责触发 Provider 关闭。测试中的 `Arc<Mutex<...>>` 和 `AtomicBool` 只用于记录请求与关闭行为，不是生产 KMS 状态；生产侧的解密 key 缓存由上游 `Mutex` 串行保护。

## 与 Go 版本的对应关系

Rust crate 把 Go `br/pkg/kms` 的四个生产文件映射为更细的五个模块：`common.go` 对应 `common.rs`，`kms.go` 对应 `kms.rs`，`aws.go` 对应 `aws.rs`，`gcp.go` 对应 `gcp.rs`，额外的 `stubs.rs` 承担 Go 中由 `encryptionpb` 和云 SDK 类型直接提供的边界。`lib.rs` 则对应 Go package 自动聚合文件的 Rust 显式 crate 门面。

关键可观察语义保持一致：Provider 都暴露解密、厂商名和关闭；AWS 厂商名为大写 `AWS`，GCP 厂商名为小写 `gcp`；AWS 仅在静态凭证成对出现时覆盖默认凭证链；GCP key ID 只去一个尾斜杠并做 Castagnoli CRC32C 完整性检查；密文不能为空，AES-GCM-256 明文 key 必须为 32 字节。Rust 的额外显式行为是用 `CancellationToken` 表达 Go `context.Context` 的取消传播，并用泛型客户端 trait 支持无网络测试。

已确认的差异/保护增强是：Rust `NewGcpKmsWithClient` 在切取前四段前显式拒绝不足四段的 key ID，避免 Go 代码在先验“参数解析阶段已校验”失效时发生切片越界；这属于当前代码事实，调用方不能依赖非法短路径继续执行。

## 扩展指南

- 新增公共 KMS 类型或厂商模块时，应先决定是否真的需要 crate 根扁平导出；新增 glob 可能造成同名冲突，应优先核对现有五个模块的公开符号。
- 新增 Provider 必须实现带 `Context` 的解密、稳定的 `Name` 和幂等/安全的 `Close`，并在 `master_key.rs::createCloudBackend` 做最小装配；厂商名必须与加密元数据及 TiKV 常量完全一致。
- 调整密钥类型或校验时，修改 `common.rs`，同步独立的 `parity_test.rs`，并检查上游 `kms_backend.rs` 的缓存与 32 字节 AES key 假设；不要把测试嵌回 `lib.rs`。
- 调整 AWS/GCP 请求时，优先保留 `*DecryptClient` 注入边界，在 `aws_test.rs`、`gcp_test.rs`、`kms_test.rs` 或 `parity_test.rs` 增加独立回归用例，覆盖取消、错误分类、CRC、关闭失败及配置边界。
- 新增 Cargo 依赖或 feature 时同步 `br/pkg/kms/Cargo.toml`，并评估 SDK/runtime 体积、同步 `block_on` 的性能影响、凭证兼容性和 TLS/endpoint 行为；本门面文件不应承载厂商业务逻辑。
- 若改变公开项名称或重导出路径，要同步检查 `br/pkg/encryption/master_key` 的导入和测试，因为 crate 根路径正是当前兼容契约。

## 验证依据

- RustCodeGraph 索引状态：11,467 个文件、7,032 个 Rust 文件；读取并核对 `br/pkg/kms/lib.rs` 及 `common.rs`、`kms.rs`、`aws.rs`、`gcp.rs`、`stubs.rs` 的符号与文件关系。
- RustCodeGraph 调用证据：`NewAwsKmsWithClient` 由生产 `NewAwsKms` 和注入测试调用；`NewPlainKey` 的生产调用方包括 `br/pkg/encryption/master_key/kms_backend.rs::DecryptWithContext`；另核对 `master_key.rs::createCloudBackend` 的 AWS/GCP Provider 装配链。
- Cargo 证据：仓库根 `Cargo.toml`、`br/pkg/kms/Cargo.toml`、`br/pkg/encryption/master_key/Cargo.toml`。
- Go 对照：`br/pkg/kms/kms.go`、`common.go`、`aws.go`、`gcp.go`。
- 独立 Rust 测试：`br/pkg/kms/parity_test.rs` 覆盖公共契约、请求字段、错误分支和 CRC；`aws_test.rs` 覆盖非服务错误只包装一次；`gcp_test.rs` 覆盖关闭错误可观测；`kms_test.rs` 覆盖取消上下文透传。
- 人工复核结论：该文件存在的理由是显式定义并扁平导出 KMS crate 边界；运行逻辑在子模块和上游 master-key backend；安全扩展需同时维护重导出契约、厂商实现、独立测试和上游装配。
