# `br/pkg/encryption/master_key/lib.rs`

## 文件定位

[`lib.rs`](lib.rs) 是 Cargo 包 `astersql-br-pkg-encryption-master-key` 的 crate 根。其边界由同目录 [`Cargo.toml`](Cargo.toml) 的 `[lib] path = "lib.rs"` 明确指定；该 manifest 还把本包标记为 Go 包 `br/pkg/encryption/master_key` 的 library 移植，并直接依赖 `astersql-br-pkg-kms`、`aes-gcm` 与 `rand`。

本文件是公开门面而非算法实现：它用 `#[path = "..."]` 装配 7 个生产模块，在测试构建中再装配 7 个独立测试模块，最后以 `pub use ...::*` 将生产模块的公开项提升到 crate 根。上游 [`br/pkg/encryption/manager.rs`](../manager.rs) 因而可以从 `astersql_br_pkg_encryption_master_key` 根路径直接导入 `EncryptedContent`、`MasterKey`、`MultiMasterKeyBackend` 和 `NewMultiMasterKeyBackend`，无需知道各符号所在子模块。

## 核心职责

1. 声明 `common`、`file_backend`、`kms_backend`、`master_key`、`mem_backend`、`multi_master_key_backend`、`pb` 七个公开生产模块。
2. 仅在 `cfg(test)` 下声明 `common_test`、`file_backend_test`、`kms_backend_test`、`mem_backend_test`、`multi_master_key_backend_test`、`parity_test`，保持生产源与测试源分离；`pb_test` 也由本文件独立装配。
3. 将七个生产模块的公开项通配再导出，形成 crate 的扁平兼容 API。当前调用者可以写 `crate::Backend` 或从外部 crate 根导入类型，而不必写 `crate::master_key::Backend`。
4. 在 crate 级允许迁移代码中尚存的 `dead_code`、Go 风格命名及未使用项。该属性只抑制 lint，不改变加密、错误传播或资源释放语义。

本文件不读取密钥、不执行 AES-GCM、不访问云 KMS，也不持有缓存或锁；这些行为分别位于下述子模块中。

## 主要符号

`lib.rs` 自身没有定义常量、类型、trait、函数、`impl` 或条件编译 feature。它公开汇总的主要符号按所有者如下：

| 所有模块 | 主要公开符号 | 对外语义 |
| --- | --- | --- |
| [`common.rs`](common.rs) | `IV`、`IvType`、`NewIVGcm`、`NewIVFromSlice`、`MetadataKey*` | 定义 12 字节 GCM / 16 字节 CTR IV 及加密元数据键。 |
| [`file_backend.rs`](file_backend.rs) | `FileBackend`、`createFileBackend`、`AesGcmKeyLen` | 从“64 个十六进制字符 + 换行”的文件加载 32 字节 AES-256 密钥，并委托内存后端加解密。 |
| [`kms_backend.rs`](kms_backend.rs) | `KmsBackend`、`CachedKeys`、`NewKmsBackend` | 校验 KMS 厂商/密文数据密钥元数据，调用 KMS 解开数据密钥，并缓存对应内存后端。 |
| [`master_key.rs`](master_key.rs) | `Backend`、`AnyBackend`、`CreateBackend`、`createCloudBackend`、`CreateKmsBackendWithProvider` | 定义统一解密/关闭契约，按 File、AWS KMS、GCP KMS 配置构造后端；拒绝 Unset、Plaintext 和未实现的 Azure。 |
| [`mem_backend.rs`](mem_backend.rs) | `MemAesGcmBackend`、`NewMemAesGcmBackend` | 使用 32 字节明文密钥执行 AES-256-GCM，并拆分/校验 IV、tag 和 method 元数据。 |
| [`multi_master_key_backend.rs`](multi_master_key_backend.rs) | `MultiMasterKeyBackend`、`NewMultiMasterKeyBackend` | 顺序尝试多个后端，首个成功结果即返回，全部失败时聚合错误，并统一关闭后端。 |
| [`pb.rs`](pb.rs) | `EncryptedContent`、`MasterKey`、`MasterKeyBackend`、`MasterKeyFile`、`MasterKeyKms` | 提供本 crate 消费的 protobuf 轻量镜像；它不实现 protobuf 线格式编解码。 |

由于使用通配再导出，新增任何 `pub` 子模块符号都可能自动成为 crate 根 API；这是扩展时必须显式评估的兼容面。

## 执行流程

crate 根没有可执行控制流。它所建立的典型运行链是：

1. 上游 [`manager.rs`](../manager.rs) 的 `NewManager` 通过根级再导出的 `NewMultiMasterKeyBackend` 接收一组 `MasterKey` 配置。
2. `NewMultiMasterKeyBackend` 逐项调用 `CreateBackend`；文件配置进入 `createFileBackend`，KMS 配置进入 `createCloudBackend`，后者再建立 AWS/GCP provider 和 `KmsBackend`。
3. `Manager::Decrypt` 在 master-key 模式下把首个 `EncryptedContent` 交给 `MultiMasterKeyBackend::Decrypt`；多后端按配置顺序尝试 `Backend::Decrypt`。
4. 文件后端直接用已加载的 `MemAesGcmBackend` 解密；KMS 后端先核对 vendor 和 ciphertext-key 元数据，必要时远程解开数据密钥、建立并缓存内存后端，再解开内容。
5. `Manager::Close` 调用 `MultiMasterKeyBackend::Close`，继而逐个派发到 `AnyBackend::Close`；文件后端无资源动作，KMS 后端关闭 provider。

编译期流程则是：普通构建只包含七个生产模块；测试构建额外包含七个独立测试源（六个 `*_test.rs` 加 `pb_test.rs`）。这些测试可通过 `use crate::{...}` 直接验证根级再导出契约。

## 数据与状态

本文件不声明静态变量或运行时状态。跨模块流动的核心数据由再导出 API 表达：`MasterKey`/`MasterKeyBackend` 描述后端选择，`EncryptedContent` 携带密文和 `HashMap<String, Vec<u8>>` 元数据，`IV` 区分 GCM 与 CTR 长度，`AnyBackend` 封装文件或 KMS 实现，`MultiMasterKeyBackend` 保存有序后端列表。

关键状态位于实现模块：`FileBackend` 持有加载后的 `MemAesGcmBackend`；`MemAesGcmBackend` 持有 KMS 包校验过的明文 AES-256 key；`KmsBackend` 在 `Mutex<Option<CachedKeys>>` 中保存最近一个密文数据密钥及其解密后端；`MultiMasterKeyBackend` 的向量顺序决定回退顺序。crate 根只让这些状态类型可被外部命名，不复制或转换数据。

## 依赖与调用关系

- 上游生产调用：[`br/pkg/encryption/manager.rs`](../manager.rs) 通过 Cargo 依赖声明和 crate 根再导出使用 `EncryptedContent`、`MasterKey`、`MultiMasterKeyBackend`、`NewMultiMasterKeyBackend`。这是当前代码搜索得到的直接跨 crate 生产入口。
- 内部装配：`master_key.rs` 依赖文件/KMS/pb 模块；`multi_master_key_backend.rs` 依赖 `Backend`、`AnyBackend`、`CreateBackend`；文件与 KMS 后端都依赖 `MemAesGcmBackend`；内存后端与 KMS 后端消费 `common`/`pb` 定义。
- 外部依赖：`rand` 为 `NewIVGcm` 提供 `OsRng`；`aes-gcm` 实现 AES-256-GCM；`astersql-br-pkg-kms` 提供 provider、密钥类型以及 AWS/GCP 构造器。依赖均由 [`Cargo.toml`](Cargo.toml) 声明。
- 测试调用：同目录测试大量使用 `use crate::{...}`，直接依赖 `lib.rs` 的扁平再导出。例如 [`parity_test.rs`](parity_test.rs) 同时导入 `Backend`、`CreateBackend`、IV/元数据常量和各后端构造函数，构成门面契约的集中验证。

RustCodeGraph 的文件索引确认 `lib.rs` 共 65 行、由一个文件节点表示，并识别出上述 25 个同目录 Go/Rust 文件；按符号执行的 `callers/callees` 查询在本次环境中 30 秒内未返回，因此精确调用边以 `rg` 的导入/调用结果和源码读取交叉验证。

## 错误处理与边界

crate 根不捕获或改写错误；公开函数的 `Result<_, String>` 直接来自所属子模块。通过门面可观察到的主要边界包括：缺失/Unset/Plaintext 主密钥配置被拒绝，文件必须严格为 32 字节密钥的 hex 表示并以换行结尾，IV 只接受 12 或 16 字节，AES-GCM method/IV/tag 缺失或认证失败会报错，KMS vendor/ciphertext-key 不匹配会报错，Azure KMS 当前明确返回未实现错误，多后端全部失败时合并各后端错误。

`NewMultiMasterKeyBackend` 保留 Go 当前条件 `nil && len == 0` 的可观察效果：Rust 的 `None` 被拒绝，而 `Some(&[])` 可构造、但首次 `Decrypt` 会返回“应至少包含一个后端”的内部错误。该边界由 [`parity_test.rs`](parity_test.rs) 明确固定，不能在只调整门面时“顺手修正”。

通配再导出也构成 API 边界：子模块中原本只为局部使用但标为 `pub` 的项会暴露到根；同名公开项可能导致编译冲突。新增或重命名公开项时必须检查根级调用者与测试。

## 并发与资源生命周期

`lib.rs` 不创建线程、任务、通道、锁、文件句柄或网络连接。并发保证来自 `KmsBackend` 的 `Mutex<Option<CachedKeys>>`：解密路径在检查/更新缓存以及 KMS 解密期间持锁，使共享后端的缓存状态串行化；provider 被约束为 `Box<dyn Provider + Send>`。`with_retry` 使用阻塞线程休眠并检查 KMS `Context` 取消，不是异步任务。

资源关闭由显式 `Close` 链负责，而不是由 crate 根负责：`Manager::Close` → `MultiMasterKeyBackend::Close` → `AnyBackend::Close` → 具体后端。`FileBackend::Close` 是空操作；`KmsBackend::Close` 调用 provider 的 `Close`。因此新增后端时必须同时接入 `AnyBackend` 的 `Decrypt` 和 `Close` 分派，避免只完成构造却遗漏释放。

密钥材料生命周期也值得注意：文件密钥被读入 `MemAesGcmBackend`，KMS 解开的数据密钥被缓存；当前类型没有在 drop 时显式清零内存。本文件只是公开这些类型，不应被描述为提供内存擦除保证。

## 与 Go 版本的对应关系

Go 同路径没有单独的 `lib.go` 门面；包内的 [`common.go`](common.go)、[`file_backend.go`](file_backend.go)、[`kms_backend.go`](kms_backend.go)、[`master_key.go`](master_key.go)、[`mem_backend.go`](mem_backend.go)、[`multi_master_key_backend.go`](multi_master_key_backend.go) 天然共享 `package encryption` 命名空间。Rust 的 `lib.rs` 通过显式 `mod` 加 `pub use ...::*` 模拟这一扁平包级可见性，`pb.rs` 则投影 Go 使用的 `encryptionpb` 字段。

主要语义对应为：Go `Backend` interface ↔ Rust `Backend` trait；Go 返回接口值 ↔ Rust `AnyBackend` enum；Go `context.Context` 参数在普通文件/内存解密 API 中未完整保留，而 KMS 路径通过 KMS crate 的 `Context` 提供取消；Go `sync.Mutex` 缓存 ↔ Rust `Mutex<Option<CachedKeys>>`；Go `Close()` ↔ Rust 可变引用上的显式 `Close`。

相关 Go 测试为 `file_backend_test.go`、`kms_backend_test.go`、`mem_backend_test.go`、`multi_master_key_backend_test.go`；Rust 对应测试全部独立位于 `common_test.rs`、`file_backend_test.rs`、`kms_backend_test.rs`、`mem_backend_test.rs`、`multi_master_key_backend_test.rs`、`pb_test.rs`、`parity_test.rs`。Rust `parity_test.rs` 还覆盖 Go/Rust 公共契约以及 AWS/GCP 生产构造器，补足 Go 测试未集中表达的门面验证。

## 扩展指南

- 新增后端：在独立生产模块实现行为和独立 `*_test.rs`；把模块声明加入 `lib.rs`，按需要再导出；同步扩展 `MasterKeyBackend`、`AnyBackend`、`CreateBackend`、`Backend::Decrypt/Close` 分派和 Go 对照测试。若需要外部 crate，更新本 crate `Cargo.toml`。
- 新增公共类型或函数：优先从所有者模块导出；确认通配再导出是否会意外扩大 API、产生重名，使用 `rg` 检查 crate 根和模块路径两种调用方式。
- 修改密文格式：同步检查 `MetadataKey*`、`EncryptedContent`、文件/KMS/内存后端和 Go `encryptionpb` 兼容性；认证 tag、IV 长度、method/vendor 字符串属于持久兼容协议，不能只改门面。
- 修改并发或重试：落点应是 `kms_backend.rs` 及其独立测试，而不是 `lib.rs`；需验证锁持有范围、取消传播、重试次数/退避和 provider 关闭。
- 测试保持独立文件：不要把单元测试内嵌到生产 `.rs`。新增测试模块时使用 `#[cfg(test)] #[path = "..."] mod ...;`，并同步 Go 对照或 `parity_test.rs` 中相应契约。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 7032 个 Rust 文件；`files --filter br/pkg/encryption/master_key` 枚举 25 个直接相关文件；`node --file br/pkg/encryption/master_key/lib.rs --offset 1 --limit 260` 确认目标文件全貌、7 个生产模块、7 个测试模块和 7 组通配再导出。`query CreateBackend --kind function --json` 将 Rust 工厂定位到 `master_key.rs:72`；精确 `callers/callees` 查询超时，未把缺失输出当作事实。
- Rust 源码：读取 `lib.rs`、七个生产子模块、直接上游 `br/pkg/encryption/manager.rs`；使用 `rg` 核对公开符号、测试函数以及跨 crate 导入。
- Cargo：读取 `br/pkg/encryption/master_key/Cargo.toml` 和依赖方 `br/pkg/encryption/Cargo.toml`，确认 crate 名、lib 路径、Go 包元数据及直接依赖关系。
- Go 对照：读取同目录 `common.go`、`file_backend.go`、`kms_backend.go`、`master_key.go`、`mem_backend.go`、`multi_master_key_backend.go`；用 `rg` 核对四个 `*_test.go` 的测试入口。
- Rust 测试：读取 `parity_test.rs`，并用 `rg` 核对 `common_test.rs`、`file_backend_test.rs`、`kms_backend_test.rs`、`mem_backend_test.rs`、`multi_master_key_backend_test.rs`、`pb_test.rs` 的独立测试入口及覆盖边界。
- 按任务约束未运行 Cargo；本任务只新增文档。交付前另运行任务给定的 11 章节结构检查，并人工复核本文没有把 crate 门面描述为算法实现。
