# `br/pkg/kms/stubs.rs`

## 文件定位

`stubs.rs` 位于 `astersql-br-pkg-kms` crate 内，由 [`lib.rs`](lib.rs) 以 `pub mod stubs` 加载，并通过 `pub use stubs::*` 提升到 crate 根。它处在 BR 主密钥配置与云厂商 SDK 之间：一端提供 `encryptionpb.MasterKeyKms` 所需字段的轻量 Rust 投影，另一端用可注入 trait 固定 AWS/GCP 解密客户端的最小契约。

“stubs”表示这里不承载云 SDK 的网络、认证和请求构造细节，并不表示整个生产路径是假实现。生产适配器 `AwsSdkClient`、`GcpSdkClient` 分别在 [`aws.rs`](aws.rs) 与 [`gcp.rs`](gcp.rs) 实现本文件的 trait；测试也通过同一边界注入内存假客户端。crate 的边界由 [`Cargo.toml`](Cargo.toml) 确认：库入口是 `lib.rs`，Go 对照包是 `br/pkg/kms`，AWS 与 Google Cloud KMS SDK 均为直接依赖。

## 核心职责

本文件有三项职责：

1. 以 `AwsKmsConfig`、`GcpKmsConfig` 和 `MasterKeyKms` 表达当前 Rust KMS 后端实际消费的配置字段。
2. 以 `AwsDecryptClient` 和 `GcpDecryptClient` 隔离厂商 SDK，使后端业务逻辑能够依赖稳定、可测试的同步接口。
3. 以 `AwsDecryptError`、`GcpDecryptResponse` 保存后端做错误分类和 CRC32C 完整性校验所需的最小返回信息。

它不选择厂商、不创建 provider、不执行重试，也不校验完整 protobuf 兼容性。厂商构造和请求语义在 `aws.rs`/`gcp.rs`，统一 `Provider` 契约在 [`kms.rs`](kms.rs)，更上层的数据密钥缓存、重试与内容解密在 `br/pkg/encryption/master_key/kms_backend.rs`。

## 主要符号

- `AwsKmsConfig { AccessKey, SecretAccessKey }`：AWS 静态凭证字段镜像；两个字段均为公开 `String`，`Default` 得到空字符串。是否使用静态凭证由 `NewAwsKms` 判断：仅两项同时非空时启用，否则走 SDK 默认凭证链。
- `GcpKmsConfig { Credential }`：GCP 凭证文件路径镜像；空值让生产构造路径采用默认凭证发现。
- `MasterKeyKms { KeyId, Region, Endpoint, AwsKms, GcpKms }`：两类厂商共享的配置聚合体。厂商子配置用 `Option` 表达缺失；所有字符串及选项的默认值均为空。
- `AwsDecryptClient: Send`：要求实现 `Decrypt(&self, ctx, ciphertext, key_id) -> Result<Vec<u8>, AwsDecryptError>`。成功值是明文数据密钥，失败值保留厂商错误码与消息。
- `AwsDecryptError { code, message }`：AWS 错误的可克隆载体；其 `Display` 精确输出 `"{code}: {message}"`，供 `classifyDecryptError` 添加语义前缀。
- `GcpDecryptClient: Send`：要求实现 `Decrypt(&self, ctx, name, ciphertext, ciphertext_crc32c)` 和 `Close(&mut self)`。请求同时传递资源名、密文与密文 CRC32C；关闭允许返回字符串错误。
- `GcpDecryptResponse { Plaintext, PlaintextCrc32C }`：GCP 解密响应投影，供调用方验证服务端返回的明文 CRC32C。

这些符号全部为公开 API；文件内唯一有函数体的逻辑是 `AwsDecryptError` 的 `Display` 实现，其余行为由 trait 实现者和调用方承担。命名保留 Go/protobuf 风格的 PascalCase；`lib.rs` 在 crate 级允许相应 Rust lint，以便迁移代码直接对照。

## 执行流程

AWS 路径如下：

1. 上层把 `MasterKeyKms` 交给 `NewAwsKms`/`NewAwsKmsWithClient`，后者保存 `KeyId`、`Region`、`Endpoint` 并持有一个 `AwsDecryptClient`。
2. `AwsKms::DecryptDataKeyWithContext` 调用本 trait 的 `Decrypt(ctx, dataKey, currentKeyID)`。
3. 生产 `AwsSdkClient::Decrypt` 把参数转换为 AWS SDK 请求，并监听 `Context` 取消；测试实现可直接记录参数或返回预设结果。
4. 成功的 `Vec<u8>` 原样向上返回；`AwsDecryptError` 则交给 `classifyDecryptError`，按 `code` 分类为错误主密钥、API 超时、内部错误或一般 KMS 错误。

GCP 路径如下：

1. `NewGcpKmsWithClient` 先要求 `MasterKeyKms.GcpKms` 存在、规范化 `KeyId`，然后保存实现了 `GcpDecryptClient` 的客户端。
2. `GcpKms::DecryptDataKeyWithContext` 对密文计算 Castagnoli CRC32C，并把 `Context`、完整密钥资源名、密文和 CRC 传给 trait 的 `Decrypt`。
3. 实现返回 `GcpDecryptResponse`；调用方重新计算 `Plaintext` 的 CRC 并与 `PlaintextCrc32C` 比较，只在匹配时返回明文。
4. provider 关闭时，`GcpKms::Close` 委托 trait 的 `Close(&mut self)`；关闭错误被报告到标准错误输出而不向调用者返回。

本文件自身没有入口函数或控制流调度；上述流程由 RustCodeGraph 证实的调用边 `AwsKms::DecryptDataKeyWithContext -> AwsDecryptClient::Decrypt` 和 `GcpKms::DecryptDataKeyWithContext -> GcpDecryptClient::Decrypt` 连接起来。

## 数据与状态

配置类型均拥有自己的 `String`/`Option` 数据并派生 `Clone + Debug + Default`，没有借用外部配置的生命周期。`MasterKeyKms` 的默认值不是“有效配置”，只代表所有字段缺省；例如 GCP 构造器会明确拒绝 `GcpKms == None`，AWS 注入式构造器则允许空配置并把空 key id 传到客户端边界。

`AwsDecryptError` 拥有错误码和消息，因此错误分类不依赖 SDK 错误对象的生命周期。`GcpDecryptResponse` 同样拥有明文字节及校验值；默认响应是空明文和零 CRC，但调用方仍会执行 CRC 比较，不能把派生 `Default` 当作真实服务端成功响应。

与 `encryptionpb.MasterKeyKms` 相比，本地 `MasterKeyKms` 只投影当前 KMS crate 使用的 `key_id`、`region`、`endpoint`、AWS 和 GCP 子配置；protobuf 的 `vendor` 与 `azure_kms` 不在此类型中。`br/pkg/encryption/master_key/pb.rs` 另有包含 `Vendor` 的上层镜像，并在装配 AWS/GCP provider 时转换为本类型。因此本类型不是 protobuf 编解码结构，也不承诺字段全集或 wire compatibility。

## 依赖与调用关系

直接类型依赖很小：trait 参数只依赖 `crate::kms::Context`，错误展示只依赖 `std::fmt`，其余字段均为标准库拥有类型。云 SDK crate 虽列在同一 `Cargo.toml` 中，但仅由 `aws.rs`/`gcp.rs` 的具体适配器使用，本文件没有直接导入 SDK 类型。

主要上游与下游关系为：

- `lib.rs` 公布本文件所有公开符号。
- `aws.rs` 消费 `MasterKeyKms`、`AwsDecryptClient`、`AwsDecryptError`，并让 `AwsSdkClient` 实现该 trait。
- `gcp.rs` 消费 `MasterKeyKms`、`GcpDecryptClient`、`GcpDecryptResponse`，并让 `GcpSdkClient` 实现该 trait。
- `br/pkg/encryption/master_key/pb.rs` 在其 KMS 配置镜像中复用 `AwsKmsConfig` 与 `GcpKmsConfig`。
- `br/pkg/encryption/master_key/master_key.rs` 将上层配置转换为本文件的 `MasterKeyKms`，再调用 `NewAwsKms` 或 `NewGcpKms`；生成的 `Provider` 由 `kms_backend.rs` 调用以解密数据密钥。
- `parity_test.rs`、`kms_test.rs`、`gcp_test.rs` 为 trait 提供假实现，分别验证请求参数、错误、取消、CRC 和关闭行为。

RustCodeGraph 对精确符号的调用边显示：AWS trait 方法由 `aws.rs:134` 的 `DecryptDataKeyWithContext` 调用；GCP trait 方法由 `gcp.rs:154` 的同名函数调用。常见的 `Decrypt`/`Close` 名称在全仓库有许多同名符号，维护时应使用文件或 trait 限定，避免把无关调用计入本模块。

## 错误处理与边界

`AwsDecryptClient::Decrypt` 使用结构化 `AwsDecryptError`，但结构只保留字符串 `code`/`message`，不保留原 SDK 错误链、重试元数据或 HTTP 状态。生产适配器负责把 SDK 错误压缩到该结构；`classifyDecryptError` 依赖稳定错误码，因此新增映射时必须同时核对 AWS SDK 和 Go `classifyDecryptError`。`Display` 不转义或隐藏消息，敏感信息是否可输出由构造错误的适配器负责。

`GcpDecryptClient::Decrypt` 与 `Close` 都以 `String` 表示错误，调用边不会获得类型化 source。GCP 响应的 `PlaintextCrc32C` 是强制字段而非 `Option`；缺失信息若被适配为默认零值，除非空明文的实际 CRC 也为零，否则会在上层触发 `response corrupted in-transit`。请求 CRC 的正确计算和响应 CRC 的验证均不在本文件，分别由 `gcp.rs` 完成。

配置结构不执行语义校验：空 key id、region、endpoint、凭证路径以及仅填写一半的 AWS 静态凭证都可以被构造。真实接受规则由厂商构造器承担；尤其不能仅凭 `Default` 或公开字段认定配置可用于生产。这里也没有 Azure 类型或加密 API，新增厂商/加密能力不能通过扩展现有解密 trait 隐式获得。

## 并发与资源生命周期

两个客户端 trait 都要求 `Send`，使具体客户端可以随拥有它的 provider 在线程之间转移；它们没有要求 `Sync`，因此本契约本身不保证同一实例可被多线程共享。`Decrypt` 接收 `&self`，实现者若记录请求或维护连接状态，需要自行使用线程安全的内部可变性；现有测试用 `Arc<Mutex<...>>` 或 `AtomicBool` 观察行为。

`Context` 以共享引用传递，生产 AWS/GCP 适配器可把取消信号传播到异步 SDK 调用；本文件既不创建后台任务也不拥有运行时。配置和响应类型没有锁、通道、事务或全局状态。

AWS trait 没有 `Close`，与 Go/AWS SDK 无需显式关闭的当前语义一致。GCP trait 的 `Close(&mut self)` 明确要求独占访问，资源释放由 `GcpKms::Close` 主动委托；没有 `Drop` 保底，也不会自动重试关闭。测试 `gcp_test.rs::close_error_is_reported_like_go` 证明关闭失败会被记录但不改变关闭调用的成功返回形态。

## 与 Go 版本的对应关系

Go 端没有 `stubs.go` 一一对应文件；本文件是 Rust 为依赖注入和避免让业务层直接依赖 SDK 类型而新增的边界。字段语义来自 `pkg/util/resourcegrouptag/proto/kvproto/encryptionpb.proto`：`AwsKms` 的 `access_key`/`secret_access_key`、`GcpKms` 的 `credential`，以及 `MasterKeyKms` 的 key id、region、endpoint 与厂商子消息。

行为对应关系由相邻 Go 文件确定：

- [`aws.go`](aws.go) 直接持有 `*kms.Client`；Rust 把其 `Decrypt` 能力抽成 `AwsDecryptClient`，并以 `AwsDecryptError` 支撑相同错误码分类。
- [`gcp.go`](gcp.go) 直接持有 `*kms.KeyManagementClient`；Rust 把请求参数、响应明文/CRC 和 `Close` 抽成 `GcpDecryptClient`/`GcpDecryptResponse`。
- [`kms.go`](kms.go) 的 `Provider` 才是面向上层的统一接口；本文件的两个 client trait 是 provider 内部的 SDK 接缝，不应与 `Provider` 混为一谈。

Rust 比 Go/protobuf 更窄：本地 `MasterKeyKms` 缺少 `vendor`、Azure 配置及序列化能力；Rust 也将厂商 SDK 错误降格为本地结构或字符串。相应收益是单元测试无需真实云凭证或网络即可验证 Go 可观察语义。`parity_test.rs` 明确覆盖 AWS 请求/错误分类、GCP 请求 CRC/响应 CRC/关闭和公开契约，且所有云调用均为注入假实现。

## 扩展指南

修改本文件前先判断变化属于“稳定接缝”还是“厂商实现”：

- 新增 AWS/GCP 请求或响应字段时，先扩展对应 trait 签名或响应结构，再同步 `aws.rs`/`gcp.rs` 的生产 SDK 实现及所有假客户端实现。签名变化是 crate 公共 API 破坏性变化，应避免仅为某个 SDK 私有细节扩大通用边界。
- 新增错误分类时，优先保持 `AwsDecryptError` 的厂商无关载体，更新 `aws.rs::classifyDecryptError`，并在独立的 `aws_test.rs` 或 `parity_test.rs` 增加回归场景。
- 新增 GCP 完整性字段时，在 `gcp.rs::DecryptDataKeyWithContext` 实现校验，不要把业务判定塞入数据结构；同步 `RecordingGcpClient` 的请求记录和损坏响应测试。
- 新增厂商时，通常应新增独立配置、client trait 和后端模块，并在上层厂商选择处接线；不要将 Azure 等不同生命周期强行并入 AWS/GCP trait。
- 若目标是完整 protobuf 互操作，应使用生成类型或显式转换层，而不是不断把本结构扩成未经验证的 protobuf 替代品。

测试逻辑应继续留在独立文件，不能内嵌到 `stubs.rs`。最近的验证位置是 `br/pkg/kms/parity_test.rs`（跨 Go/Rust 契约）、`kms_test.rs`（取消上下文传播）、`gcp_test.rs`（关闭错误），以及 `aws_test.rs`（AWS 分类）。涉及上层消费时还应同步 `br/pkg/encryption/master_key/kms_backend_test.rs`。兼容性风险主要是公开 trait 实现集被破坏、错误文本改变和配置投影漂移；性能风险集中在新增的复制或锁，而当前结构本身只持有拥有值、没有热路径计算。

## 验证依据

本说明基于以下直接证据：

- RustCodeGraph `status`：索引包含 7,032 个 Rust 文件；`files --filter br/pkg/kms` 列出本 crate 的源文件与独立测试。
- RustCodeGraph `node --file br/pkg/kms/stubs.rs`：确认文件共 73 行，以及三个配置结构、两个 trait、两个传输结构和 `Display` 实现的完整定义。
- RustCodeGraph 精确符号查询：`aws.rs::DecryptDataKeyWithContext` 调用 `stubs.rs::AwsDecryptClient::Decrypt`；`gcp.rs::DecryptDataKeyWithContext` 调用 GCP `Decrypt` 并验证响应 CRC；`NewAwsKmsWithClient`、`NewGcpKmsWithClient` 分别消费 `MasterKeyKms`；`gcp.rs::Close` 委托 client `Close`。
- [`Cargo.toml`](Cargo.toml) 与 [`lib.rs`](lib.rs)：确认 crate 名、入口、公开重导出、云 SDK 直接依赖及 Go 包映射。
- Go/协议对照：[`aws.go`](aws.go)、[`gcp.go`](gcp.go)、[`kms.go`](kms.go) 和 `pkg/util/resourcegrouptag/proto/kvproto/encryptionpb.proto`。
- Rust 消费方：`br/pkg/encryption/master_key/pb.rs`、`master_key.rs`、`kms_backend.rs`；其中 `kms_backend.rs::DecryptWithContext` 通过 `Provider::DecryptDataKey` 解密并缓存数据密钥。
- 独立测试：[`parity_test.rs`](parity_test.rs)、[`kms_test.rs`](kms_test.rs)、[`gcp_test.rs`](gcp_test.rs)、[`aws_test.rs`](aws_test.rs) 以及上层 `br/pkg/encryption/master_key/kms_backend_test.rs`。

本任务是纯文档分析，按计划不运行 Cargo。结构验收通过固定标题检查完成；代码运行行为的结论限于上述源码、调用图、Go 对照与已有测试证据，不声称本轮重新执行了这些测试。
