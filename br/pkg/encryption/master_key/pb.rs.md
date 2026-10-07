# `br/pkg/encryption/master_key/pb.rs`

## 文件定位

[`pb.rs`](./pb.rs) 属于 `astersql-br-pkg-encryption-master-key` library crate。该 crate 由 [`Cargo.toml`](./Cargo.toml) 定义，入口 [`lib.rs`](./lib.rs) 以 `pub mod pb` 装配本模块，再以 `pub use pb::*` 将公开类型重导出到 crate 根。文件只有数据类型，没有加解密、文件访问、KMS 请求或 protobuf 编解码函数。

本文件是 Go `encryptionpb` 消息在 Rust 主密钥子系统中的轻量字段投影，而不是由 `.proto` 生成的完整绑定。权威消息形状位于 [`pkg/util/resourcegrouptag/proto/kvproto/encryptionpb.proto`](../../../../pkg/util/resourcegrouptag/proto/kvproto/encryptionpb.proto)；当前投影只保留 `br/pkg/encryption/master_key` 已实际消费的密文、元数据和后端配置字段。

## 核心职责

本文件承担两个边界职责：一是用 `EncryptedContent` 统一文件、内存 AES-GCM、KMS 和多主密钥后端之间传递的密文字节与元数据；二是用 `MasterKey`、`MasterKeyBackend`、`MasterKeyFile`、`MasterKeyKms` 表达创建主密钥后端所需的配置选择。

它刻意不负责 protobuf 线格式，也没有完整镜像 `encryptionpb.proto`。例如 Rust `EncryptedContent` 没有 proto 中的 `master_key`、`iv`、`ciphertext_key` 字段，Rust `MasterKeyKms` 没有 Azure 具体配置；相邻后端目前把 IV、算法、KMS vendor 和被包装的数据密钥放进 `Metadata`。因此这些结构只适合当前 crate 内部数据流，不能直接当作完整的跨语言序列化类型。

## 主要符号

- `EncryptedContent { Content: Vec<u8>, Metadata: HashMap<String, Vec<u8>> }`：拥有密文字节及字符串到字节串的元数据。`mem_backend.rs::EncryptContent` 创建它；文件后端、KMS 后端和多主密钥后端借用它进行解密。
- `MasterKeyFile { Path: String }`：文件主密钥的最小配置，`master_key.rs::CreateBackend` 将 `Path` 传给 `createFileBackend`。
- `MasterKeyKms { KeyId, Region, Endpoint, Vendor, AwsKms, GcpKms }`：云 KMS 后端配置。两个可选厂商配置来自依赖 crate `astersql-br-pkg-kms`；`master_key.rs::createCloudBackend` 将字段复制到该依赖的 `MasterKeyKms` 配置后选择 AWS、GCP 或错误分支。
- `MasterKeyBackend::{Unset, Plaintext, File, Kms}`：Rust 对 Go `MasterKey.backend` oneof 的显式枚举表示。`Unset` 是 `#[default]`，表示 oneof 未设置；`File` 和 `Kms` 直接携带对应配置。
- `MasterKey { Backend: MasterKeyBackend }`：后端选择的顶层包装。`CreateBackend` 消费单个配置，`NewMultiMasterKeyBackend` 消费配置切片。

五个公开类型都实现 `Clone`、`Debug`、`Default`；除 `MasterKeyBackend::Unset` 的显式默认分支外，结构默认值由字段默认值组合而成，即空字节、空 map、空字符串和 `None`。它们没有实现 protobuf 编解码 trait，也没有实现 `PartialEq`。

## 执行流程

配置链路从 `encryption/manager.rs::MasterKeyConfig.MasterKeys` 或其他构造者产生 `Vec<MasterKey>` 开始。`NewMultiMasterKeyBackend` 逐个把 `MasterKey` 交给 `CreateBackend`；后者匹配 `Backend`：`Unset` 和 `Plaintext` 返回错误，`File` 用 `Path` 创建文件后端，`Kms` 把 `MasterKeyKms` 交给云后端工厂。AWS/GCP 分支继续使用 `AwsKms`/`GcpKms` 和公共字符串字段构造 KMS provider。

密文链路由 `mem_backend.rs::EncryptContent` 创建 `EncryptedContent`：AES-GCM 密文写入 `Content`，method、IV 和认证 tag 写入 `Metadata`。文件后端直接转交这个值；KMS 解密路径还从同一 map 读取 KMS vendor 和密文数据密钥。`manager.rs::Decrypt` 取得一个或多个 `EncryptedContent` 后，最终经 `MultiMasterKeyBackend::Decrypt` 把借用传给候选后端；本文件本身不推进状态，只定义各步骤共享的数据载体。

## 数据与状态

所有字段都拥有其数据：`Vec<u8>`、`String`、`HashMap` 和可选 KMS 配置在结构内保存，调用者不受输入缓冲区生命周期约束。`Clone` 会复制这些容器及其内容；对大密文或较大元数据频繁克隆会产生与数据量线性相关的分配和复制成本，正常解密 API 因此使用 `&EncryptedContent` 借用。

`Metadata` 是开放的字符串键空间，结构本身不验证必需键、重复语义、算法版本或字节长度。具体不变量由消费者执行：`mem_backend.rs::DecryptContent` 校验 method、IV 和 tag，`kms_backend.rs::DecryptWithContext` 校验 vendor 和 KMS ciphertext key。`MasterKey` 也允许构造 `Unset`、空路径、空 vendor 或不完整 KMS 配置；有效性在后端工厂中延迟检查。

## 依赖与调用关系

本文件的标准库依赖只有 `std::collections::HashMap`。外部类型依赖是 `astersql_br_pkg_kms::{AwsKmsConfig, GcpKmsConfig}`，其来源由当前 crate 的 `Cargo.toml` 路径依赖 `../../kms` 声明。清单中的 `aes-gcm` 和 `rand` 由相邻模块使用，不是本文件的直接调用依赖。

已核验的主要关系如下：

- `lib.rs -> pub mod pb -> pub use pb::*`，使五个类型同时可从 `crate::pb` 和 crate 根访问。
- `mem_backend.rs::EncryptContent -> EncryptedContent`，写入 `Content` 和 `Metadata`；`DecryptContent` 读取二者。
- `file_backend.rs::{Encrypt, Decrypt}`、`kms_backend.rs::{Decrypt, DecryptWithContext}` 和 `multi_master_key_backend.rs::Decrypt` 都以 `EncryptedContent` 为边界值。
- `master_key.rs::CreateBackend -> MasterKey.Backend`，并进一步读取 `MasterKeyFile.Path` 或调用 `createCloudBackend(&MasterKeyKms)`。
- `manager.rs::NewManager -> NewMultiMasterKeyBackend(Some(&MasterKeys))`；`manager.rs::Decrypt` 把保存的 `EncryptedContent` 交给聚合后端。

RustCodeGraph 的文件节点记录了 `multi_master_key_backend_test.rs`、`pb_test.rs` 及三个 `metautil` 测试文件对本模块的文件级使用；纯结构体没有可执行 callee，符号级 `callers` 未返回可靠的完整引用集合，所以上述生产关系由索引源码与精确引用搜索交叉核验。

## 错误处理与边界

本文件没有返回 `Result` 的函数，因而不直接产生或包装错误。其边界风险来自“可构造但尚未验证”的状态：默认 `MasterKey` 是 `Unset`，`CreateBackend` 将其拒绝为 `unknown master key backend type`；`Plaintext` 虽是合法 proto oneof 分支，也被该后端工厂按设计拒绝；空文件路径、未知 KMS vendor 和不完整厂商配置由后续构造器报告。

`EncryptedContent::default()` 是空密文和空元数据，但不代表可解密内容。解密器会按自己的协议检查缺失键，AES-GCM 认证还会拒绝错误 tag 或错误主密钥。开放 `HashMap` 也意味着未知元数据会被保留在内存中但通常被当前消费者忽略；新增键不能假定旧消费者会理解它。

最重要的兼容边界是本类型不提供线格式保证。不能用 Rust `Debug` 表示、字段内存布局或手写序列化替代 `encryptionpb.proto`；需要持久化或网络传输时必须使用完整 protobuf 类型或实现经过兼容测试的显式转换。

## 并发与资源生命周期

本文件没有锁、线程、异步任务、通道、文件句柄、网络连接或显式清理逻辑。值的生命周期由 Rust 所有权管理；后端通常借用配置或密文完成一次操作，聚合对象则把由配置创建出的实际后端保存到自己的生命周期中。

类型内部没有可变共享状态。是否跨线程传递由字段类型的 `Send`/`Sync` 自动推导，本文件没有添加 `unsafe impl` 或额外同步保证。`EncryptedContent` 可能含敏感密文和被包装的数据密钥元数据，析构时只是常规释放，未主动清零；日志中也不应因为实现了 `Debug` 就打印完整值。真正需要关闭的 KMS provider 由 `Backend::Close` 管理，不由这些配置结构管理。

## 与 Go 版本的对应关系

Go 生产代码直接使用 `github.com/pingcap/kvproto/pkg/encryptionpb` 生成类型；同仓 [`encryptionpb.proto`](../../../../pkg/util/resourcegrouptag/proto/kvproto/encryptionpb.proto) 是字段语义依据。Rust `MasterKeyBackend` 对应 Go oneof：`Plaintext`、`File`、`Kms` 分别映射三个分支，额外的 `Unset` 显式表达 Go 中 `Backend == nil`，`pb_test.rs::master_key_default_represents_unset_go_oneof` 固定了这一默认语义。

Rust `MasterKeyFile.Path` 对齐 proto `path`。Rust `MasterKeyKms` 对齐 vendor、key_id、region、endpoint、aws_kms 和 gcp_kms，但省略 `azure_kms`；尽管枚举工厂识别 Azure vendor，当前 `createCloudBackend` 明确返回 `not implemented Azure KMS`。AWS/GCP 子配置使用 KMS crate 的轻量配置类型，而非 protobuf 生成类型。

Rust `EncryptedContent.Content/Metadata` 对齐 proto 的同名字段，但省略 `master_key`、`iv` 和 `ciphertext_key`。当前 Go/Rust 后端共同依赖 metadata 协议完成 AES-GCM 与 KMS 解密，这能覆盖本包当前路径，却不等同于完整消息等价。字段采用 PascalCase 是为贴近 Go 使用面；`lib.rs` 通过 lint allow 接受这些非惯用 Rust 名称。

## 扩展指南

- 新增主密钥后端时，应先更新 `MasterKeyBackend`，再同步 `master_key.rs::CreateBackend`、Go/proto 对照、聚合后端及独立测试；只增加枚举变体会导致工厂语义缺失。测试应放在 `pb_test.rs` 或对应后端的独立 `*_test.rs`，不要嵌入 `pb.rs`。
- 增加 KMS 厂商配置字段时，要确认字段属于本 crate 实际消费范围，并同步 `createCloudBackend` 到 `astersql-br-pkg-kms::MasterKeyKms` 的转换。Azure 若从错误桩升级为实现，必须补齐 Azure 配置投影与构造测试，不能只接受 vendor 字符串。
- 扩展 `EncryptedContent` 前先判断需求是 crate 内部字段还是 protobuf 持久化字段。后者应以 `.proto` 演进和兼容转换为中心，并验证未知字段、旧数据和 Go/Rust 往返；不应仅在轻量结构中添字段后宣称线格式已支持。
- 修改默认值时必须保留未设置 oneof 的可表达性，或同步调整 `CreateBackend` 的错误契约和 `pb_test.rs`。对 map 键协议的修改还需同步 `common.rs`、内存/KMS 后端及其测试。
- 性能方面优先沿用借用式解密接口，避免克隆 `EncryptedContent`；安全方面避免输出 `Debug` 全量内容，并评估敏感元数据在释放前是否需要显式清零。

## 验证依据

事实核验读取了：`br/pkg/encryption/master_key/pb.rs`、`Cargo.toml`、`lib.rs`、`pb_test.rs`、`master_key.rs`/`.go`、`mem_backend.rs`/`.go`、`file_backend.rs`、`kms_backend.rs`、`multi_master_key_backend.rs`、`parity_test.rs`、`br/pkg/encryption/manager.rs`，以及 `pkg/util/resourcegrouptag/proto/kvproto/encryptionpb.proto`。

RustCodeGraph 证据包括：`status`（索引含 7032 个 Rust 文件）、`files --filter br/pkg/encryption/master_key`、`node --file br/pkg/encryption/master_key/pb.rs`，以及对 `EncryptedContent`、`MasterKeyFile`、`MasterKeyKms`、`MasterKeyBackend`、`MasterKey` 的查询。目标节点确认文件为 48 行、11 个符号，并给出五个测试侧文件使用者；数据类型没有函数调用边，生产消费者通过索引范围内的精确符号引用补证。

独立测试证据包括：`pb_test.rs` 验证默认 `MasterKey` 对应未设置 oneof；`parity_test.rs::go_rust_public_contract_matches` 验证 `Plaintext`/`Unset` 错误、File 构造和 Azure 未实现分支；相邻 file/KMS/multi-master-key 测试覆盖 `EncryptedContent` 的实际构造与消费。按任务约束未运行 Cargo；交付只执行 11 个固定章节的结构检查，并人工复核链接、字段差异、调用关系和扩展风险。
