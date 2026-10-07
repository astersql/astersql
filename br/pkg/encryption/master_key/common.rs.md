# `br/pkg/encryption/master_key/common.rs`

## 文件定位

[`common.rs`](./common.rs) 属于 `astersql-br-pkg-encryption-master-key` library crate；crate 的清单是 [`Cargo.toml`](./Cargo.toml)，入口 [`lib.rs`](./lib.rs) 以 `pub mod common` 声明本模块并通过 `pub use common::*` 重导出其公开项。它不是加密算法实现，而是文件主密钥、内存 AES-GCM 后端和 KMS 后端共同使用的元数据字段名与初始化向量（IV）值对象层。

`Cargo.toml` 的 `package.metadata.porting.go-package` 指向 `br/pkg/encryption/master_key`，因此该文件应按同路径 [`common.go`](./common.go) 的可观察语义维护。RustCodeGraph 将本文件识别为 18 个符号，并记录其被 `common_test.rs`、`file_backend.rs`、`file_backend_test.rs`、`kms_backend.rs`、`kms_backend_test.rs` 使用。

## 核心职责

本文件有三项职责：

1. 用 `MetadataKey*` 和 `MetadataMethodAes256Gcm` 常量固定 `EncryptedContent.Metadata` 的跨后端协议键和值；这些字符串还必须与 TiKV 消费的格式兼容（对应 Go 文件的注释明确要求常量与 TiKV 保持一致）。
2. 用 `IvType`、`IV`、`GcmIv12` 和 `CtrIv16` 表达 IV 的算法类别、拥有的数据及两种合法长度。
3. 提供安全构造入口：`NewIVGcm` 从操作系统密码学随机源产生 12 字节 GCM IV，`NewIVFromSlice` 对外部字节做长度分类并复制，避免构造后继续借用调用方缓冲区。

本文件不持有主密钥、不执行 AES-GCM、不读写文件，也不负责 KMS 重试；这些职责分别位于 `mem_backend.rs`、`file_backend.rs` 和 `kms_backend.rs`。

## 主要符号

- `MetadataKeyMethod = "method"`：标识加密方法字段；`mem_backend.rs::EncryptContent` 写入，`DecryptContent` 读取并校验。
- `MetadataKeyIv = "iv"`：保存 IV 原始字节；解密时交给 `NewIVFromSlice` 重新分类。
- `MetadataKeyAesGcmTag = "aes_gcm_tag"`：保存 16 字节 GCM 认证标签；缺失或被篡改会令下游解密失败。
- `MetadataKeyKmsVendor = "kms_vendor"`、`MetadataKeyKmsCiphertextKey = "kms_ciphertext_key"`：KMS 后端用来校验供应商并取得被 KMS 包装的数据密钥。
- `MetadataMethodAes256Gcm = "aes256-gcm"`：当前内存后端写入并接受的算法标识。
- `GcmIv12: usize = 12`、`CtrIv16: usize = 16`：`NewIVGcm` 的分配长度和 `NewIVFromSlice` 的合法长度集合。
- `IvType::{IvTypeGcm, IvTypeCtr}`：带显式判等能力的算法类别，判别值分别为 0、1，与 Go 的 `iota` 顺序一致。
- `IV { Type: IvType, Data: Vec<u8> }`：拥有 IV 数据的公开值对象；`Clone` 会深拷贝 `Vec`，`Copy` 未实现。
- `NewIVGcm() -> Result<IV, String>`：生产入口，将 `OsRng` 传给内部可注入随机源的实现。
- `new_iv_gcm_with_rng<R: RngCore + CryptoRng>(...)`：crate 内可见的核心生成逻辑，同时是随机源失败测试的注入缝。
- `NewIVFromSlice(src: &[u8]) -> Result<IV, String>`：只接受 12 或 16 字节，并用 `to_vec` 获取所有权。
- `IV::AsSlice(&self) -> &[u8]`：零拷贝只读借用；返回值的有效期受 `IV` 借用约束。

## 执行流程

随机 GCM IV 的流程是：`file_backend.rs::FileBackend::Encrypt` 或其他调用者调用 `NewIVGcm`；`NewIVGcm` 创建 `OsRng` 的可变借用并转交 `new_iv_gcm_with_rng`；后者分配恰好 `GcmIv12` 个零字节，以 `try_fill_bytes` 一次性填满，再返回 `Type = IvTypeGcm` 的 `IV`。随后 `mem_backend.rs::EncryptContent` 通过 `AsSlice` 把 IV 写入元数据，并把同一字节切片传给 AES-GCM nonce 构造。

外部 IV 的流程是：`mem_backend.rs::DecryptContent` 从 `MetadataKeyIv` 取得字节后调用 `NewIVFromSlice`。长度 16 被分类为 CTR，长度 12 被分类为 GCM；两条成功分支都复制输入。其他长度立即返回含实际长度的错误。当前 AES-GCM 解密路径随后把返回的 `AsSlice` 作为 nonce；因此调用方除了依赖分类，还依赖本文件严格维持合法 nonce 长度。

元数据常量的另一条链路是：`mem_backend.rs` 写入/读取 method、iv、tag，`kms_backend.rs::DecryptWithContext` 读取 vendor 和 ciphertext key。公共字符串一旦改变，已有密文以及 Go/TiKV 侧数据会失去兼容性。

## 数据与状态

`IV` 是无内部可变性的拥有型数据：`Data` 是独立 `Vec<u8>`，`Type` 是复制型枚举。`NewIVFromSlice` 的 `to_vec` 保证调用者修改原切片后不会改变已构造的 IV；`common_test.rs::new_iv_from_slice_clones_and_classifies_supported_lengths` 明确验证这一不变量。

本模块没有全局可变状态。所有元数据名称和长度都是编译期常量；随机状态只存在于一次 `NewIVGcm` 调用期间。字段目前为公开命名，调用方能够直接构造不满足长度约束的 `IV`，所以“12/16 字节合法”只由构造函数保证，并非类型系统对所有 `IV` 实例的强制不变量。安全扩展时应优先继续使用构造函数，而不是直接填写字段。

## 依赖与调用关系

唯一直接外部 Rust 依赖是 `rand 0.8`：`OsRng` 提供操作系统随机源，`RngCore::try_fill_bytes` 提供可传播失败的填充操作，`CryptoRng` 限定测试/替代随机源具备密码学随机语义。crate 清单还列出 `aes-gcm` 和 `astersql-br-pkg-kms`，但它们由相邻后端使用，不由本文件直接调用。

已核验的上游与下游关系如下：

- `file_backend.rs::FileBackend::Encrypt -> NewIVGcm -> new_iv_gcm_with_rng -> RngCore::try_fill_bytes`。
- `mem_backend.rs::EncryptContent -> IV::AsSlice`，并使用 `MetadataKeyMethod`、`MetadataKeyIv`、`MetadataKeyAesGcmTag`、`MetadataMethodAes256Gcm`。
- `mem_backend.rs::DecryptContent -> NewIVFromSlice -> IV::AsSlice`。
- `kms_backend.rs::KmsBackend::DecryptWithContext` 使用 `MetadataKeyKmsVendor` 与 `MetadataKeyKmsCiphertextKey`。
- `lib.rs` 将本模块所有公开项提升到 crate 根；`parity_test.rs` 也通过该重导出面验证公共契约。

RustCodeGraph 的精确 callee 查询确认 `NewIVGcm` 调用 `new_iv_gcm_with_rng`，后者引用 `GcmIv12`、实例化 `IV` 并调用 `try_fill_bytes`；对通用方法名 `AsSlice` 的 caller 查询未产生边，因此其调用位置用图索引文件和相邻源码交叉核验，不据此声称调用集合完整。

## 错误处理与边界

`new_iv_gcm_with_rng` 使用 `try_fill_bytes` 而不是不可失败的填充接口；随机源错误通过 `to_string` 原样降格为 `String` 并由 `?` 传播。失败时不会返回全零或部分填充的 IV。`common_test.rs::new_iv_gcm_propagates_random_source_errors` 用 `FailingRng` 验证错误文本可观察且代码不会退回 `next_u32`、`next_u64` 或 `fill_bytes`。

`NewIVFromSlice` 的成功边界仅为 12 和 16 字节。长度相等时复制所有字节，不验证随机性、是否全零或是否曾被使用；这是与 Go 实现一致的职责边界。其他长度返回 `invalid IV length, must be 12 or 16 bytes, got {n}`。测试覆盖 0、11、13、15、17 字节并精确比较错误文本。

需要注意：`NewIVFromSlice` 接受 16 字节 CTR IV，但当前 `mem_backend.rs` 的实际密码实现是 AES-256-GCM。调用方必须根据自己的算法上下文使用 `Type`；本文件只分类，不进行算法支持判定。元数据键没有版本协商或别名机制，修改字符串属于持久化格式兼容性变化。

## 并发与资源生命周期

本文件不创建线程、任务、锁、通道、文件句柄或网络连接。所有函数只操作调用栈上的枚举/结构和局部 `Vec`，天然可重入；不同线程并发调用不会共享本模块状态。

`NewIVGcm` 每次调用都临时使用 `OsRng`，随机源借用在函数返回前结束；产生的字节由返回的 `IV` 独占。`NewIVFromSlice` 在返回前完成复制，源切片可立即修改或释放。`AsSlice` 不分配、不复制，借用不能超过 `IV` 生命周期；若需要跨越该生命周期，调用方必须像 `mem_backend.rs` 写元数据时那样显式 `to_vec`。

本模块不会清零 `IV::Data`。IV 通常不需要保密，但若未来把该类型泛化为承载秘密材料，必须另行设计清零与日志策略，不能假设当前生命周期自动擦除内存。

## 与 Go 版本的对应关系

[`common.go`](./common.go) 是直接对照实现。两端的六个元数据常量、12/16 字节长度、`IvType` 枚举顺序、`IV` 的 `Type/Data` 布局语义、随机 GCM 构造、按长度分类与复制、以及错误消息形状一致。

实现手段存在可解释差异：Go 用 `crypto/rand.Read`，Rust 用 `rand::rngs::OsRng` 加 `try_fill_bytes`；Go 用 `slices.Clone`，Rust 用 `to_vec`；Go 的 `AsSlice` 返回可变 `[]byte` 值，Rust 返回受借用规则保护的 `&[u8]`。Rust 还增加 `new_iv_gcm_with_rng` 这一 `pub(crate)` 测试缝，以确定性验证随机源失败，而公开 `NewIVGcm` 的行为没有因此简化。

Go 目录没有独立 `common_test.go`。Go 的 `file_backend_test.go::TestFileBackendAes256Gcm` 通过固定 12 字节 IV 验证加密结果，Rust 的直接契约由 `common_test.rs` 覆盖，并由 `parity_test.rs::go_rust_public_contract_matches` 和相邻后端测试补充端到端证据。

## 扩展指南

- 新增元数据字段时，在本文件增加稳定常量，并同步检查 `pb.rs::EncryptedContent` 的 map 使用者、相关生产者/消费者、Go 同路径常量以及 TiKV 格式兼容性；不能只修改一侧字符串。
- 新增加密方法或 IV 长度时，应同时扩展 `IvType`、构造/解析规则和算法后端。必须新增 `common_test.rs` 中的合法长度、非法长度、复制隔离和错误测试，并同步 Go 对照测试；不要让 `NewIVFromSlice` 接受下游算法不能安全消费的长度。
- 调整随机生成逻辑时，保留 `RngCore + CryptoRng` 约束和可失败接口，继续用独立测试文件注入失败随机源。禁止把测试写回 `common.rs`。
- 若要强化不变量，可评估将 `IV` 字段收窄可见性并提供构造器，但这会影响当前 crate 外公开 API，需先盘点 `lib.rs` 重导出消费者并安排兼容迁移。
- 性能上，`AsSlice` 应继续零拷贝；`NewIVFromSlice` 的复制是所有权隔离的一部分，不应为了微小分配收益改为借用而改变生命周期契约。

## 验证依据

事实核验读取了以下文件：`br/pkg/encryption/master_key/common.rs`、`common.go`、`common_test.rs`、`file_backend.rs`、`file_backend_test.go`、`kms_backend.rs`、`mem_backend.rs`、`lib.rs`、`parity_test.rs` 和本 crate 的 `Cargo.toml`。

RustCodeGraph 证据包括：`status`（索引含 7032 个 Rust 文件）、`files --filter br/pkg/encryption/master_key`、目标文件 `node --file`、上述相邻文件的 `node --file`，以及对 `NewIVGcm`、`NewIVFromSlice`、`new_iv_gcm_with_rng`、`AsSlice` 的 `query/callers/callees` 查询。图确认目标文件级使用者与 `NewIVGcm -> new_iv_gcm_with_rng` 调用边；未返回的通用名称调用边由源码引用搜索补充，未被描述为完整闭包。

独立 Rust 测试的可复核契约是：GCM 结果类型与长度、随机源错误传播、12/16 字节分类、输入复制隔离、其他长度的精确错误消息。按任务约束这是纯文档分析，未运行 Cargo 或代码测试；交付验证使用任务指定的 11 个固定章节结构检查，并人工复查链接、符号名、职责边界与“如何安全扩展”的说明。
