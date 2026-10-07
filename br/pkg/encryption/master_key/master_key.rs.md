# `br/pkg/encryption/master_key/master_key.rs`

## 文件定位

该文件位于 `astersql-br-pkg-encryption-master-key` library crate 中；crate 的入口 `br/pkg/encryption/master_key/lib.rs` 以 `#[path = "master_key.rs"] pub mod master_key` 装配该模块，并通过 `pub use master_key::*` 重导出其公开符号。`br/pkg/encryption/master_key/Cargo.toml` 的 `[package.metadata.porting]` 将本 crate 对应到 Go 包 `br/pkg/encryption/master_key`，因此本文件是同路径 `master_key.go` 的 Rust 工厂与统一后端门面。

它不实现具体密码算法：文件密钥的读取和 AES-GCM 解密由 `file_backend.rs` 承担，云密钥的数据密钥解封与缓存由 `kms_backend.rs` 承担。本文件负责根据 protobuf 风格的 `MasterKeyBackend` 配置选择实现，并用 `AnyBackend` 把两种具体类型统一为一个可返回的 Rust 类型。

当前生产侧直接入口是 `multi_master_key_backend.rs::NewMultiMasterKeyBackend`：它逐个调用 `CreateBackend(Some(mk))` 建立候选后端，之后按顺序尝试解密。crate 外也可以通过 `lib.rs` 的重导出直接使用这里的公开 API；现有 RustCodeGraph/文本引用未显示其他 Rust 生产调用者。

## 核心职责

1. 用 `Backend` trait 规定统一的解密与关闭能力，使多主密钥逻辑无需知道具体实现。
2. 用 `AnyBackend::{File, Kms}` 保存异构后端，并把 `Decrypt`、`Close` 分派到真实实现。
3. 用 `CreateBackend` 验证主密钥配置并选择文件或 KMS 后端；明确拒绝缺失、未设置和明文配置。
4. 用 `createCloudBackend` 将本 crate 的 `MasterKeyKms` 转成 `astersql-br-pkg-kms::MasterKeyKms`，再按厂商选择 AWS/GCP provider；Azure 当前明确返回未实现错误。
5. 用 `CreateKmsBackendWithProvider` 把 provider 注入 `KmsBackend`，形成云后端构造的单一收口点。

该文件只负责装配和静态分派，不读取文件内容、不调用远程 KMS 解密、不缓存数据密钥，也不执行加密。

## 主要符号

- `StorageVendorNameAWS`、`StorageVendorNameAzure`、`StorageVendorNameGCP`：云厂商配置的精确字符串标识，分别为 `"aws"`、`"azure"`、`"gcp"`；`createCloudBackend` 对 `Vendor` 做区分大小写的精确匹配。
- `trait Backend`：公开抽象，`Decrypt(&self, &EncryptedContent) -> Result<Vec<u8>, String>` 返回明文或字符串错误；`Close(&mut self)` 提供显式资源回收入口。trait 本身不要求 `Send`/`Sync`，也不携带请求上下文。
- `enum AnyBackend`：当前仅有 `File(FileBackend)` 与 `Kms(KmsBackend)` 两个变体。使用枚举而非 `Box<dyn Backend>`，让工厂返回单一具体类型并进行穷尽分派。
- `impl Backend for AnyBackend`：`Decrypt` 按变体只调用一次相应后端；`Close` 需要可变借用并按变体转发。这里不改写结果、不吞掉错误。
- `CreateBackend(Option<&MasterKey>)`：顶层工厂。`None`、`Unset`、`Plaintext` 均失败；`File` 调 `createFileBackend`；`Kms` 调 `createCloudBackend`。
- `createCloudBackend(&MasterKeyKms)`：复制 `KeyId`、`Region`、`Endpoint`、`AwsKms`、`GcpKms` 到 KMS crate 配置，并调用 `NewAwsKms` 或 `NewGcpKms`。Azure 和未知厂商不会创建 provider。
- `CreateKmsBackendWithProvider(Box<dyn Provider + Send>)`：调用 `NewKmsBackend`，成功后包装为 `AnyBackend::Kms`。`Send` 约束保证被注入 provider 可跨线程转移，但该签名没有声明 `Sync`。

## 执行流程

配置到后端的主流程如下：

1. `NewMultiMasterKeyBackend` 或 crate 使用者把一个 `MasterKey` 引用传给 `CreateBackend`。
2. `CreateBackend` 先解构 `Option`。无配置立即返回 `master key config is nil`。
3. 工厂匹配 `config.Backend`：
   - `Unset` 返回 `unknown master key backend type`；
   - `Plaintext` 返回 `should not create plaintext master key`，因为调用链约定明文类型不应建立解密后端；
   - `File` 把 `Path` 交给 `createFileBackend`，成功后构成 `AnyBackend::File`；
   - `Kms` 进入 `createCloudBackend`。
4. 云路径先克隆五个配置字段，避免 provider 借用调用方的 protobuf 配置。随后根据 `Vendor`：AWS/GCP 分别调用生产 KMS 构造器；Azure直接失败；未知值返回包含原厂商字符串的错误。
5. AWS/GCP provider 经 `CreateKmsBackendWithProvider` 传给 `NewKmsBackend`，最后得到 `AnyBackend::Kms`。
6. 使用阶段对 `AnyBackend` 调 `Decrypt`，枚举分派到 `FileBackend::Decrypt` 或 `KmsBackend::Decrypt`。多主密钥上游会在失败时继续尝试下一个后端，在首个成功结果处停止。
7. 所有者结束使用时以可变引用调用 `Close`；文件后端当前为空操作，KMS 后端会继续调用 provider 的 `Close`。

## 数据与状态

本文件自身没有全局可变状态，也不保存缓存。三个厂商名是只读 `&'static str` 常量。

`AnyBackend` 拥有一个具体后端：文件变体间接持有内存 AES-GCM 后端；KMS 变体持有 provider 以及由 `kms_backend.rs` 管理的 `Mutex<Option<CachedKeys>>`。因此状态生命周期与 `AnyBackend` 一致，但状态的创建、更新及密码材料处理均发生在下游文件中。

`createCloudBackend` 会克隆字符串和可选厂商配置，构造完成后后端不借用传入的 `MasterKeyKms`。`CreateBackend` 的 `MasterKey` 参数只是构造期间的共享借用。解密输入也只被共享借用，明文以新 `Vec<u8>` 返回。

`AnyBackend` 没有 `Clone`/`Copy` 实现；provider 通过 `Box<dyn Provider + Send>` 被独占转移到 KMS 后端。调用者不能从统一枚举直接访问 KMS 缓存或文件后端的内部密钥。

## 依赖与调用关系

- 上游生产调用：`multi_master_key_backend.rs::NewMultiMasterKeyBackend -> CreateBackend`。同文件的 `MultiMasterKeyBackend::Decrypt/Close` 随后通过 `Backend` trait 调统一门面。
- 测试调用：`parity_test.rs::go_rust_public_contract_matches` 直接覆盖 `CreateBackend`、`createCloudBackend`、`AnyBackend::Decrypt/Close`；`cloud_backends_use_production_kms_constructors` 覆盖 AWS/GCP 构造。
- 文件后端下游：`CreateBackend -> file_backend.rs::createFileBackend`；运行期 `AnyBackend::Decrypt/Close -> FileBackend::Decrypt/Close`。
- 云后端下游：`CreateBackend -> createCloudBackend -> astersql_br_pkg_kms::{NewAwsKms|NewGcpKms} -> CreateKmsBackendWithProvider -> kms_backend.rs::NewKmsBackend`；运行期再分派至 `KmsBackend::Decrypt/Close`。
- 数据类型依赖：`pb.rs::{MasterKey, MasterKeyBackend, MasterKeyKms, EncryptedContent}` 提供配置 oneof、KMS 配置和密文载体。
- crate 边界：`Cargo.toml` 通过路径依赖 `../../kms` 引入 `astersql-br-pkg-kms`；`aes-gcm`、`rand` 属于同 crate 其他实现文件的直接依赖，本文件没有直接引用它们。

RustCodeGraph 对 `createCloudBackend` 给出的局部调用边是 `CreateBackend -> createCloudBackend -> CreateKmsBackendWithProvider`，并显示后者内部调用 `NewKmsBackend`。对全仓 Rust 文本引用的复核补充确认了 `multi_master_key_backend.rs` 的生产工厂调用。

## 错误处理与边界

所有错误统一为 `String`，本文件不定义结构化错误类型。匹配失败立即返回，成功路径不会降级到另一种后端；只有上层 `MultiMasterKeyBackend` 才会尝试多个已构造后端。

主要边界及可观察错误为：

- `config == None`：`master key config is nil`。
- `MasterKeyBackend::Unset`：`unknown master key backend type`。
- `MasterKeyBackend::Plaintext`：`should not create plaintext master key`。
- 文件构造失败：前缀被包装为 `master key config is nil: ...`。这个措辞与 Go `errors.Annotate` 保持一致，但语义上容易被误读为配置对象为空，修改时应把兼容性风险纳入测试。
- AWS/GCP provider 构造失败：分别增加 `new AWS KMS: `、`new GCP KMS: ` 前缀。
- Azure：固定返回 `not implemented Azure KMS`。
- 未知厂商：返回 `vendor not found: {other}`，并暴露原始厂商值以便诊断。
- `CreateKmsBackendWithProvider` 原样传播 `NewKmsBackend` 的字符串错误，不额外增加上下文。

此层不验证空 `KeyId`、区域、endpoint 或凭据，也不执行密文元数据校验；这些责任留给 provider 构造器及 `KmsBackend::Decrypt`。厂商名大小写、首尾空白均不会被规范化。

## 并发与资源生命周期

工厂函数只创建局部值，没有锁和异步任务。`CreateBackend` 返回的 `AnyBackend` 由调用者独占；统一 `Backend` trait 未声明 `Send + Sync`，因此不能只凭该 trait 推断后端可以安全共享。

KMS provider 的注入类型要求 `Provider + Send`。真正的并发保护位于 `kms_backend.rs`：`KmsBackend` 用 `Mutex<Option<CachedKeys>>` 串行保护缓存刷新与使用。本文件的 `Decrypt(&self, ...)` 允许共享借用，但能否跨线程共享仍由完整类型及 provider 的 auto trait 决定。

资源关闭不是 `Drop` 自动触发，而是显式 `Close(&mut self)`。`AnyBackend::Close` 对文件后端为空操作，对 KMS 后端继续关闭 provider。上游 `MultiMasterKeyBackend::Close` 会遍历所有后端调用它；若调用者漏调 `Close`，本文件没有补偿机制。当前 `Close` 无返回值，关闭失败也没有可传播通道。

## 与 Go 版本的对应关系

Rust 的三个厂商常量、`Backend` 抽象、`CreateBackend` 分支、云 provider 选择和主要错误文本，均直接对应 `br/pkg/encryption/master_key/master_key.go`。

实现形态存在以下已验证差异：

- Go `Backend` 是动态接口，工厂返回 `Backend`；Rust 用 `AnyBackend` 枚举承载当前两种实现，同时仍提供 `Backend` trait。新增后端时必须同时扩展枚举及两处分派。
- Go `Decrypt(ctx context.Context, ...)` 把取消上下文贯穿统一接口；Rust `Backend::Decrypt` 不接收上下文，`KmsBackend::Decrypt` 使用默认上下文，只有具体方法 `DecryptWithContext` 能显式接收 KMS crate 的 `Context`。因此经 `AnyBackend`/`MultiMasterKeyBackend` 的公共路径不能传入调用方取消信号。
- Go protobuf oneof 未设置时落入 switch 默认分支；Rust 用显式 `MasterKeyBackend::Unset` 表达并返回同类错误。
- Go 返回接口中的指针后端；Rust 枚举按值拥有 `FileBackend`/`KmsBackend`，并通过借用调用方法。
- Go `createCloudBackend` 会记录 region、endpoint、key ID、vendor；Rust 对应函数当前不产生日志。
- Go 将同一个 protobuf KMS 配置直接传给 provider 构造器；Rust 显式克隆字段，转换成 KMS crate 的配置类型。
- Go 的 provider 接口类型没有在此签名展示并发限定；Rust 注入点显式要求 `Send`。

`parity_test.rs` 固定了两端共同契约：缺失/明文/Unset 配置失败，文件后端可解密，Azure 仍未实现，AWS/GCP 使用生产构造器且可以在离线测试配置下完成构造。

## 扩展指南

- 新增云厂商：先在 KMS crate 提供真实 `Provider` 构造器，再新增厂商常量和 `createCloudBackend` 分支；同步 `pb.rs` 配置映射、`parity_test.rs` 的成功/错误场景及 Go 同路径实现。注意凭据、endpoint、区域和错误前缀的兼容性，不能只返回占位 provider。
- 新增非 KMS 后端：扩展 `MasterKeyBackend`、`AnyBackend`、`CreateBackend`，并同步 `AnyBackend` 的 `Decrypt` 与 `Close` 两个穷尽分派；同时更新 `multi_master_key_backend` 的独立测试，验证失败回退与资源关闭。
- 改进取消传播：需要同时调整 `Backend::Decrypt`、`AnyBackend`、`MultiMasterKeyBackend` 及所有具体实现/测试，不能只改 KMS 分支。目标应与 Go 的 `context.Context` 可观察取消行为对齐。
- 改动错误：错误字符串被 `parity_test.rs` 和上层聚合错误消费；若引入结构化错误，应保留足够上下文并验证对外显示文本及多后端聚合行为。
- 改动关闭语义：由于当前不是 RAII，若改为 `Drop` 或可失败关闭，需明确是否仍保留幂等显式 `Close`，并增加 provider 关闭次数、提前返回和部分构造失败的独立测试。
- 性能关注：本文件的配置克隆仅发生在构造期；不要把 provider 创建或配置克隆移入逐次 `Decrypt`。真正的 KMS 网络重试和缓存性能应在 `kms_backend.rs` 及其独立测试中验证。

Rust 单元测试应继续放在独立文件中。与本文件最直接的是 `br/pkg/encryption/master_key/parity_test.rs`；多后端抽象与回退行为位于 `multi_master_key_backend_test.rs`，具体文件/KMS 行为分别位于 `file_backend_test.rs`、`kms_backend_test.rs`。

## 验证依据

- RustCodeGraph `status`：索引包含目标目录；`files --filter br/pkg/encryption/master_key` 列出 `master_key.rs`、Go 对照、模块入口及独立测试。
- RustCodeGraph `node --file br/pkg/encryption/master_key/master_key.rs`：核对 3 个常量、`Backend`、`AnyBackend`、3 个工厂函数及全部分支。
- RustCodeGraph `explore "MasterKeyBackend EncryptedContent create_backend br/pkg/encryption/master_key/master_key.rs"`：核对 `CreateBackend -> createCloudBackend -> CreateKmsBackendWithProvider` 以及文件/KMS 的解密和关闭分派。
- `br/pkg/encryption/master_key/Cargo.toml`、`lib.rs`：核对 library crate、Go 包映射、KMS 路径依赖、模块装配和公开重导出。
- `br/pkg/encryption/master_key/master_key.go`：核对工厂分支、厂商选择、错误上下文、日志与 context 差异。
- `br/pkg/encryption/master_key/multi_master_key_backend.rs`、`multi_master_key_backend.go`：核对生产上游、逐后端构造、解密回退和关闭生命周期。
- `br/pkg/encryption/master_key/parity_test.rs`：核对缺失、Plaintext、Unset、File、Azure、AWS、GCP 的真实 Rust 测试断言；该测试由 `lib.rs` 以 `#[cfg(test)]` 独立装配。
- `br/pkg/encryption/master_key/multi_master_key_backend_test.go`：核对 Go trait 替身对首个成功、后续成功、全部失败和空集合的行为约束。
- 未运行 Cargo 或代码测试：任务是纯文档分析，计划明确禁止 Cargo；事实验证使用索引、源码、Cargo 声明、Go 对照与现有独立测试完成。
