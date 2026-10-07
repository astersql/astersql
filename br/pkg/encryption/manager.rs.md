# `br/pkg/encryption/manager.rs`

## 文件定位

本文件是 `astersql-br-pkg-encryption` crate 的核心实现，负责把备份文件携带的加密元数据转换为实际解密动作。crate 入口 `br/pkg/encryption/lib.rs` 以 `pub use manager::*` 重导出这里的公开类型和函数；`br/pkg/encryption/Cargo.toml` 表明它直接依赖主密钥适配 crate `astersql-br-pkg-encryption-master-key` 和 AES-CTR 工具 crate `astersql-util-encrypt`。

它位于“从外部存储读取备份字节”与“具体密钥后端/AES 解密”之间。当前可见的生产接线在 `br/pkg/stream/stream_mgr.rs`：`MetadataHelper::ReadFileWithEncryption` 校验密文 checksum 后，通过持有的 `EncryptionManager` 调用 `Manager::Decrypt`，再对明文做解压；`MetadataHelper::Close` 转发 `Manager::Close`。因此本文件不负责文件 I/O、checksum 或压缩，只负责选择密钥来源并解密字节。

## 核心职责

1. 用 `EncryptionMethod`、`CipherInfo`、`MasterKeyConfig`、`FileEncryptionMode` 和 `FileEncryptionInfo` 表达当前 Rust 迁移层需要的加密配置与单文件元数据。
2. `IsEffectiveEncryptionMethod` 统一判定是否真正启用加密：`Unknown` 和 `Plaintext` 都不是有效加密方法，三种 AES-CTR 方法是有效方法。
3. `NewManager` 在旧式明文 data key 配置和主密钥保护的 data key 配置之间选择一种管理器状态；两者都未启用时返回 `Ok(None)`。
4. `Manager::Decrypt` 根据每个文件的 `FileEncryptionMode` 选择直接使用配置中的 data key，或先经主密钥后端解开 data key，再调用 `DecryptContent` 解密文件内容。
5. `Manager::Close` 把资源释放传递给主密钥后端；纯明文 data key 模式没有需要关闭的后端。

## 主要符号

- `EncryptionMethod`：公开枚举，数值布局为 `Unknown = 0`、`Plaintext = 1`、`Aes128Ctr = 2`、`Aes192Ctr = 3`、`Aes256Ctr = 4`，默认值是 `Unknown`。它同时用于全局配置和单文件加密方法。
- `CipherInfo { CipherType, CipherKey }`：旧式明文 data key 配置。`CipherKey` 直接交给 AES-CTR 工具，不在本文件派生或校验长度。
- `MasterKeyConfig { EncryptionType, MasterKeys }`：主密钥模式的全局配置；`NewManager` 根据 `EncryptionType` 决定是否构造 `MultiMasterKeyBackend`。
- `FileEncryptionMode`：单文件模式。`Unset` 表示 protobuf oneof 未设置，`PlainTextDataKey` 表示直接使用管理器中的 `CipherInfo`，`MasterKeyBased { DataKeyEncryptedContent }` 表示文件携带一个或多个被主密钥加密的 data key。
- `FileEncryptionInfo { Mode, FileIv, EncryptionMethod }`：解密一个文件所需的模式、CTR IV 和算法。主密钥模式从这里取最终 data key 所用的算法。
- `Manager`：内部保存三项可选状态：`cipherInfo`、`masterKeyBackends`、`encryptionMethod`。字段不公开；构造器保证正常返回的实例只启用所选择路径所需的主要状态。当前 `encryptionMethod` 被保存但未在本文件后续读取。
- `IsEffectiveEncryptionMethod(method) -> bool`：公开纯函数，排除 `Unknown` 和 `Plaintext`。
- `DecryptContent(content, cipher, iv) -> Result<Vec<u8>, String>`：公开底层解密函数。空输入直接返回空副本，`Plaintext` 原样复制，三种 AES-CTR 调用 `AESDecryptWithCTR`，其它方法返回 `cipher type invalid ...`。
- `NewManager(cipherInfo, masterKeyConfigs) -> Result<Option<Manager>, String>`：公开构造器。两个参数都必须是 `Some`；有效 `cipherInfo` 优先于有效主密钥配置。
- `Manager::Decrypt`：公开的逐文件解密入口。
- `Manager::Close`：公开的显式关闭入口，遍历关闭已构造的主密钥后端。

## 执行流程

构造阶段由 `NewManager` 完成：

1. 任一配置为 `None` 时立即返回 `cipherInfo or masterKeyConfigs is nil`。这保留了 Go 配置“理论上有默认值，实际仍防御 nil”的契约。
2. 若 `cipherInfo.CipherType` 有效，构造只含 `cipherInfo` 的管理器，忽略主密钥配置。这一优先级与 `manager.go` 一致。
3. 否则，若 `masterKeyConfigs.EncryptionType` 有效，调用 `NewMultiMasterKeyBackend(Some(&MasterKeys))`。成功后保存后端和加密方法；后端创建错误原样返回。
4. 两种配置都未启用加密时返回 `Ok(None)`，表示调用者无需安装加密管理器，而不是错误。

文件解密由 `Manager::Decrypt` 完成：

1. `Unset` 直接报“不支持 `<nil>` 模式”，对应 Go oneof 未设置时的 default 分支。
2. `PlainTextDataKey` 要求管理器持有 `cipherInfo`；随后把内容、全局 cipher 和文件 IV 交给 `DecryptContent`，并在失败时增加 `failed to decrypt content using plaintext data key` 上下文。
3. `MasterKeyBased` 首先要求 `DataKeyEncryptedContent` 非空。当前只取索引 `0`；列表为未来让不同主密钥分别包装同一 data key 的扩展位。
4. 主密钥模式要求管理器持有 `masterKeyBackends`，并调用 `MultiMasterKeyBackend::Decrypt` 解开第一个 data key。该后端按配置顺序尝试各 backend，首个成功即返回，全部失败时汇总错误。
5. 用文件的 `EncryptionMethod` 和解开的 data key 临时构造 `CipherInfo`，再以文件 IV 调用 `DecryptContent`。

上游 `MetadataHelper::ReadFileWithEncryption` 的完整顺序是“读取密文 → 可选 SHA-256 校验 → `Manager::Decrypt` → 解压”，所以扩展本文件时不能把 checksum 或解压职责倒置到这里。

## 数据与状态

`Manager` 是配置完成后的有状态对象，但不修改密钥内容：`Decrypt` 只借用 `&self`，不会轮换或替换 `cipherInfo`、后端或算法。`CipherInfo` 与 `FileEncryptionInfo` 中的密钥、IV 都以 `Vec<u8>` 持有，离开作用域时按普通 Rust 所有权释放；本文件没有主动清零敏感字节。

构造状态存在三个重要不变量：

- 旧式 data key 路径应有 `cipherInfo = Some`，主密钥后端为空。
- 主密钥路径应有 `masterKeyBackends = Some`，`cipherInfo` 为空，并保存 `encryptionMethod`。
- 无有效加密配置时没有 `Manager`，而是 `Ok(None)`。

这些不变量由 `NewManager` 建立，但字段采用 `Option`，因此 `Decrypt` 仍在使用点防御缺失状态并返回明确错误。主密钥模式的 `DataKeyEncryptedContent` 是列表，但当前行为只消费第一项；改变选择策略会影响与 Go 的兼容性。

## 依赖与调用关系

直接上游与调用边：

- `br/pkg/encryption/lib.rs` 声明 `manager` 模块并重导出全部公开 API。
- `br/pkg/stream/stream_mgr.rs::MetadataHelper::with_encryption_manager` 接收并以 `Arc<Mutex<Manager>>` 保存管理器。
- `br/pkg/stream/stream_mgr.rs::MetadataHelper::ReadFileWithEncryption` 在 checksum 通过后调用 `Manager::Decrypt`。
- `br/pkg/stream/stream_mgr.rs::MetadataHelper::Close` 锁定管理器并调用 `Manager::Close`。
- `br/pkg/restore/log_client/log_file_manager.rs` 目前只保留 `Option<encryption::Manager>` 字段；该文件注释明确当前过滤路径未直接使用它，不能据此宣称已接入日志恢复解密。

直接下游与调用边：

- `NewManager -> IsEffectiveEncryptionMethod`，用于两套配置的启用判定。
- `NewManager -> NewMultiMasterKeyBackend`，把 `MasterKey` 列表转换为文件或 KMS 后端集合。
- `Manager::Decrypt -> MultiMasterKeyBackend::Decrypt`，解开主密钥保护的 data key。
- `Manager::Decrypt -> DecryptContent -> AESDecryptWithCTR`，完成最终内容解密。RustCodeGraph 没有解析出宏/跨 crate 的 AES 调用边，但源码导入与 `Cargo.toml` 依赖共同确认了该关系。
- `Manager::Close -> MultiMasterKeyBackend::Close -> Backend::Close`，最终关闭文件/KMS 后端；KMS 后端再关闭 provider。

RustCodeGraph 对 `manager.rs` 的文件级使用者还列出 `br/pkg/stream/stream_mgr_test.rs`、`br/pkg/task/backup_test.rs` 和 `tests/realtikvtest/brietest/gc_keyspace_test.rs`。精确文本核对显示前者实际构造 `FileEncryptionInfo`；后两者命中的 `manager`/`NewManager` 是其它管理器语义，属于名称歧义，不能当作本文件调用证据。

## 错误处理与边界

- 构造器拒绝缺少任一顶层配置，即使另一配置足以解密；这是与 Go 对齐的输入约束。
- `cipherInfo` 有效时拥有优先级，主密钥配置即使也有效也不会被创建或验证。
- 空内容在 `DecryptContent` 中先于算法分支返回成功，因此即使 cipher 类型未知，空内容也不会触发类型错误。
- `Plaintext` 对非空内容原样返回；`Unknown` 对非空内容报错。AES key 长度和 IV 合法性由 `AESDecryptWithCTR` 检查，其错误被转换为字符串。
- `PlainTextDataKey` 与管理器状态不匹配时返回 `plaintext data key info is required but not set`。
- `MasterKeyBased` 的 data key 列表为空时先报 `should contain at least one encrypted data key`；非空但无后端时返回后端的“至少一个 backend”错误。
- 主密钥后端解密错误和最终内容解密错误分别附加不同上下文，便于区分是 data key 无法解包还是文件密文无法解密。
- `Unset` 明确映射为 Go 的 nil oneof 错误。Rust 的 `FileEncryptionInfo::default()` 会产生该状态。
- 本文件不校验 IV 长度、算法与 key 长度是否匹配，也不认证 CTR 密文；调用者依靠外层 checksum 检测密文损坏。

## 并发与资源生命周期

`Manager` 自身不含锁、线程、任务或通道，`Decrypt` 只读借用状态。并发策略由调用者决定：当前 `MetadataHelper` 用 `Arc<Mutex<EncryptionManager>>` 串行化解密与关闭操作，所以 `Manager::Close(&mut self)` 不会与同一 helper 发起的解密并行执行。这里不应据此推断 `Manager` 对所有调用场景天然线程安全；公开 API 也没有在本文件声明独立的并发保证。

主密钥后端在 `NewManager` 构造时创建，在 `Manager::Close` 显式关闭。纯明文 data key 模式的 `Close` 是空操作。`MultiMasterKeyBackend::Close` 逐一调用所有 backend 的 `Close`；本文件没有 `Drop` 实现，因此需要持有者在生命周期结束时显式调用 `Close`。重复关闭是否安全取决于具体 backend，本文件不额外保证幂等。

敏感数据生命周期方面，解开的 data key 被放入局部 `Vec<u8>`，随后移动到临时 `CipherInfo`；解密返回后正常释放，但没有零化。若新增缓存、后台任务或密钥轮换，必须同时设计锁边界、关闭顺序和敏感内存处理。

## 与 Go 版本的对应关系

Rust `manager.rs` 直接对照 `br/pkg/encryption/manager.go`：`Manager` 的三项状态、`NewManager` 的分支优先级、两种文件模式、只选第一个加密 data key、错误上下文以及关闭主密钥后端的意图均保持一致。`br/pkg/encryption/parity_test.rs` 还以公开契约测试固定了 nil 配置、无有效加密返回空、真实 AES-128-CTR 向量、空主密钥列表、未知 cipher 与 unset mode 等行为。

需要注意的实现差异：

- Go 使用 kvproto 的 `backuppb`/`encryptionpb` 消息；Rust 在本 crate 内定义等价的轻量类型和 `FileEncryptionMode` 枚举。
- Go `Decrypt` 接收 `context.Context` 并传给主密钥后端；Rust `Manager::Decrypt` 没有上下文参数，当前主密钥 trait 也同步返回 `Result<_, String>`。因此不能声称 Rust 路径具有与 Go 相同的调用级取消传播。
- Go 调用 `utils.Decrypt`；Rust 在本文件提供语义相同的 `DecryptContent` 并直接依赖 `astersql-util-encrypt::AESDecryptWithCTR`。
- Go `Close` 允许 nil 接收者并直接返回；Rust 方法只能在已有 `&mut Manager` 上调用，不存在 nil 接收者调用，但可选 manager 由外层 `Option` 表达。
- Go 使用 PingCAP 错误链；Rust 使用 `String` 并手工添加上下文，保留可读文本但不保留结构化错误类型。
- Rust `Manager.encryptionMethod` 与 Go 字段一样在构造时保存，但当前 Rust 文件内未消费。

## 扩展指南

- 新增加密算法时，至少同步修改 `EncryptionMethod`、`IsEffectiveEncryptionMethod`（若启用判定变化）、`DecryptContent` 的分派及 `br/pkg/encryption/parity_test.rs` 的成功/错误向量；同时核对 Go 枚举数值和 `br/pkg/utils/encryption.go`，避免序列化值漂移。
- 支持多个加密 data key 时，应从 `Manager::Decrypt` 的 `DataKeyEncryptedContent[0]` 选择策略入手，并与 Go 的“列表为未来扩展”语义一起升级。需要定义尝试顺序、错误汇总和是否允许不同 backend/算法，不能仅循环到成功而不记录兼容约束。
- 新增主密钥供应商或后端应在 `br/pkg/encryption/master_key/master_key.rs::CreateBackend` / `createCloudBackend` 和对应独立测试中实现；本文件只负责装配 `MultiMasterKeyBackend`，不应内嵌供应商逻辑。
- 若增加取消、超时或异步解密，应先解决 Rust `Manager::Decrypt` 到 KMS provider 的上下文传播，并同步 `MetadataHelper` 的锁持有范围，避免在全局互斥锁内长期等待网络。
- 若改变错误类型，应保留三层可辨识上下文：主密钥解包、明文 data key 内容解密、解包后的 data key 内容解密，并同步对等测试的断言。
- 若增加自动资源管理，可考虑受控的 `Drop`，但必须先验证各 backend 重复关闭的幂等性以及显式 `Close` 后再次析构的行为。
- 测试仍应放在独立文件：公开契约与本文件分支放在 `br/pkg/encryption/parity_test.rs`；主密钥后端细节放在 `br/pkg/encryption/master_key/*_test.rs`；上游读取顺序和锁/关闭接线放在 `br/pkg/stream/stream_mgr_test.rs`。不要把测试模块内嵌回 `manager.rs`。

## 验证依据

- RustCodeGraph `status`：索引覆盖 11,467 个文件，其中 Rust 7,032 个；本次查询基于现有 SQLite 索引。
- RustCodeGraph `node --file br/pkg/encryption/manager.rs`：核对了文件全部 174 行、公开符号、内部字段和分支。
- RustCodeGraph `explore`、`query NewManager`、`callers`/`callees`：确认 `NewManager -> IsEffectiveEncryptionMethod`、`Manager::Decrypt -> DecryptContent` 等边，并暴露了同名 `Manager`/`NewManager` 的歧义；歧义项随后用精确路径文本检索排除。
- crate 与入口：`br/pkg/encryption/Cargo.toml`、`br/pkg/encryption/lib.rs`、根 `Cargo.toml`。
- 直接下游：`br/pkg/encryption/master_key/multi_master_key_backend.rs`、`br/pkg/encryption/master_key/master_key.rs`、`br/pkg/utils/encryption.rs`、`pkg/util/encrypt`（由 Cargo 路径依赖确认）。
- 直接上游：`br/pkg/stream/stream_mgr.rs`，重点核对 `MetadataHelper::with_encryption_manager`、`ReadFileWithEncryption` 和 `Close`。
- Go 对照：`br/pkg/encryption/manager.go`、`br/pkg/utils/encryption.go`、`br/pkg/encryption/master_key/multi_master_key_backend.go`。
- 独立测试：`br/pkg/encryption/parity_test.rs`；相关上游顺序测试位于 `br/pkg/stream/stream_mgr_test.rs`。主密钥具体实现另由 `br/pkg/encryption/master_key/*_test.rs` 覆盖。
- 人工复核结论：本文能够回答文件存在原因、构造和解密路径、状态不变量、上游接线、错误与资源边界，以及扩展时需同步的实现和独立测试位置；未把名称歧义造成的测试引用误报为本文件生产调用。
