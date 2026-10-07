# [`br/pkg/encryption/lib.rs`](./lib.rs)

## 文件定位

`br/pkg/encryption/lib.rs` 是 Cargo library `astersql-br-pkg-encryption` 的 crate 根。`br/pkg/encryption/Cargo.toml` 通过 `[lib] path = "lib.rs"` 指向它，仓库根 `Cargo.toml` 又把 `br/pkg/encryption` 列为 workspace member。该文件自身不实现密码算法，而是把 `manager.rs` 纳入模块树、在测试构建中挂载 `parity_test.rs`，并把 `manager` 的公开项重导出到 crate 根。

Rust 生产侧的直接消费者是 `br/pkg/stream`：其 `Cargo.toml` 以路径依赖引入本 crate，`stream_mgr.rs` 从根命名空间导入 `FileEncryptionInfo` 和别名为 `EncryptionManager` 的 `Manager`。因此这个入口文件存在的主要价值，是为流备份元数据读取提供稳定的扁平 API，同时保持真正的状态和分支逻辑在独立实现文件中。

## 核心职责

该文件只有装配职责，没有运行时主动行为：

1. `#[path = "manager.rs"] pub mod manager` 将加密管理器的数据类型、构造函数和解密逻辑声明为公开子模块。
2. `#[cfg(test)] #[path = "parity_test.rs"] mod parity_test` 只在测试配置下编译独立契约测试，避免把测试逻辑放进生产源文件。
3. `pub use manager::*` 将 `manager.rs` 的所有公开符号提升到 crate 根，使调用方使用 `astersql_br_pkg_encryption::{Manager, FileEncryptionInfo, ...}`，无需经过 `manager` 子路径。
4. crate 级 `#![allow(...)]` 容纳从 Go 迁移而来的命名和暂未使用成员；它只影响编译告警，不改变校验、错误或加解密行为。

该文件不是生成代码、桩或密码学实现。实际 AES-CTR 解密由依赖 `astersql-util-encrypt` 提供，主密钥解封由依赖 `astersql-br-pkg-encryption-master-key` 提供，选择和编排逻辑位于 `manager.rs`。

## 主要符号

- `pub mod manager`（`lib.rs:20-21`）：公开实现模块。其主要 API 包括 `EncryptionMethod`、`CipherInfo`、`MasterKeyConfig`、`FileEncryptionMode`、`FileEncryptionInfo`、`Manager`、`IsEffectiveEncryptionMethod`、`DecryptContent` 和 `NewManager`。
- `mod parity_test`（`lib.rs:22-24`）：私有测试模块，仅在 `cfg(test)` 下存在。它覆盖构造参数、模式选择、AES-CTR 向量、空内容、非法算法、空主密钥列表和未设置 oneof 模式。
- `pub use manager::*`（`lib.rs:25`）：crate 根兼容面。任何在 `manager.rs` 新增的 `pub` 项都会自动成为根级 API；这种 glob re-export 便利但会放大意外公开和未来名称冲突的风险。

`lib.rs` 本身没有常量、类型、trait、函数或 `impl`，也没有条件 feature；它唯一的条件编译项是测试模块。公开 API 的签名和行为必须以 `manager.rs` 为准，不能从重导出语句推断额外能力。

## 执行流程

通过根重导出进入的管理器流程如下：

1. 调用方准备 `CipherInfo` 与 `MasterKeyConfig`，调用 `NewManager(Option<CipherInfo>, Option<MasterKeyConfig>)`。任一参数为 `None` 都报错；明文 data-key 配置中的算法有效时优先建立仅含 `cipherInfo` 的管理器；否则主密钥配置算法有效时构造 `MultiMasterKeyBackend`；两者都无效则返回 `Ok(None)`。
2. `br/pkg/stream/stream_mgr.rs` 通过 `MetadataHelper::with_encryption_manager` 注入一个已构造的 `Manager`，并用 `Arc<Mutex<Manager>>` 保存。
3. `MetadataHelper::ReadFileWithEncryption` 先读取文件并在存在 checksum 时校验密文；若文件声明加密却未设置管理器，则返回明确错误。之后它锁住管理器并调用 `Manager::Decrypt`，最后才解压解密结果。
4. `Manager::Decrypt` 根据 `FileEncryptionMode` 分流。`PlainTextDataKey` 使用构造时保存的 `CipherInfo`；`MasterKeyBased` 取第一份加密 data key，经主密钥后端解封后临时组装 `CipherInfo`；`Unset` 直接报错。
5. 两条有效分支最终调用 `DecryptContent`：空内容原样返回，`Plaintext` 原样返回，三种 AES-CTR 枚举委托 `AESDecryptWithCTR`，其他枚举报非法 cipher type。
6. `MetadataHelper::Close` 锁住管理器并调用 `Manager::Close`；后者若持有主密钥后端，则继续关闭后端资源。

`lib.rs` 在加载时不会自动创建管理器、打开 KMS、启动线程或执行解密。当前仓库搜索只确认 `parity_test.rs` 直接调用 `NewManager`；生产流模块提供管理器注入和消费路径，但本文未找到其生产侧构造接线，因此不把完整 BR 启动链描述为已经贯通。

## 数据与状态

根文件不持有状态。重导出的实现数据集中在 `manager.rs`：

- `EncryptionMethod` 区分 `Unknown`、`Plaintext` 与 AES-128/192/256-CTR；只有后三种 AES 值被 `IsEffectiveEncryptionMethod` 视为“有效加密”。
- `CipherInfo` 保存算法和原始 data key；`MasterKeyConfig` 保存 data-key 算法及一组主密钥配置。
- `FileEncryptionInfo` 保存每个文件的模式、IV 和算法。`MasterKeyBased` 还携带 `Vec<EncryptedContent>`；当前实现只使用第一个元素，保留列表是为了未来由不同主密钥后端包装同一 data key 的扩展。
- `Manager` 在两种构造模式间保持互斥状态：明文 data-key 路径保存 `cipherInfo`，主密钥路径保存 `masterKeyBackends` 与 `encryptionMethod`。其中 `encryptionMethod` 当前只被保存，没有在本文件所追踪的解密路径中读取。

内容与 key 都以 `Vec<u8>`/字节切片传递；`DecryptContent` 返回新的 `Vec<u8>`。代码未实现 key 的显式清零，文档不能宣称内存会在使用后立即擦除。

## 依赖与调用关系

编译依赖方向为 `lib.rs -> manager.rs`。`manager.rs` 再调用：

- `astersql_br_pkg_encryption_master_key::{NewMultiMasterKeyBackend, MultiMasterKeyBackend, MasterKey, EncryptedContent}`，用于构建、解封和关闭主密钥后端；
- `astersql_util_encrypt::AESDecryptWithCTR`，用于实际 AES-CTR 内容解密。

反向依赖方面，`br/pkg/stream/Cargo.toml` 是仓库中查到的本 crate 直接 Cargo 消费者。`br/pkg/stream/stream_mgr.rs` 使用根重导出的 `FileEncryptionInfo` 和 `Manager`，在 `ReadFileWithEncryption` 中形成“读密文 -> 校验 checksum -> 解密 -> 解压”的调用顺序，并在 helper 关闭时转发资源关闭。

RustCodeGraph 的文件视图确认 `lib.rs` 只有一个生产模块和一个测试模块；对 `NewManager` 的节点追踪显示其调用 `IsEffectiveEncryptionMethod`，源码还直接调用 `NewMultiMasterKeyBackend`。图工具对 glob re-export 与常见方法名的跨 crate 解析不完整，因此生产反向边同时使用 Cargo 声明与精确 `rg` 引用核验，而不是把模糊的同名搜索结果当作调用证据。

## 错误处理与边界

`lib.rs` 不自行构造错误，根 API 暴露的是 `manager.rs` 的 `Result<_, String>` 契约：

- `NewManager` 要求两份配置都存在；这与 Go 注释所述“配置应有默认值，因此 nil 不应发生”一致，但 Rust 仍保留防御性错误。
- 明文 data-key 模式要求管理器保存 `cipherInfo`；主密钥模式要求至少一份 `EncryptedContent` 且管理器保存后端，否则返回针对缺失状态的错误。
- 当前主密钥模式只尝试 `DataKeyEncryptedContent[0]`。列表中的后续密文不会在管理器层轮询；后端内部的多后端尝试属于 master-key crate 的职责。
- `Unset` 模式对应 Go protobuf oneof 未设置，返回 `internal error: unsupported encryption mode type <nil>`。
- 空内容在检查 cipher 枚举与 key/IV 之前直接成功返回空向量；非空 `Plaintext` 原样返回；非空 `Unknown` 报错。
- AES/key/IV 细节错误由 `AESDecryptWithCTR` 转成字符串，外层再附加“plaintext data key”或“decrypted data key”上下文。

根级 glob 导出也是 API 边界：新增公开实现符号会自动扩大 crate 的稳定面。扩展时应检查是否确实需要 `pub`，并避免与未来其他根导出重名。

## 并发与资源生命周期

`lib.rs` 和 `Manager` 本身不创建线程、异步任务或 channel，`Manager` 也没有内部锁。生产消费者 `MetadataHelper` 用 `Arc<Mutex<Manager>>` 允许共享所有权并串行化 `Decrypt`/`Close`；锁策略属于 `br/pkg/stream/stream_mgr.rs`，不是本 crate 对所有调用方强制的线程安全保证。

主密钥资源从 `NewMultiMasterKeyBackend` 成功返回后由 `Manager` 持有，显式 `Manager::Close` 向后端转发关闭。Rust 类型没有实现 `Drop` 来自动调用 `Close`，因此调用方必须遵守显式关闭约定；`MetadataHelper::Close` 提供了当前生产消费路径的转发点。明文 data-key 管理器没有后端，关闭是空操作。

`Decrypt` 对 `&self` 工作，主密钥后端的 `Decrypt` 也通过共享引用调用；是否允许真实 KMS 客户端并发以及内部重试/取消行为，需要继续查看 master-key 子 crate，不能从这个门面推断。与 Go 方法不同，当前 Rust `Decrypt` 不接收 `context.Context`，所以该层没有调用方取消信号。

## 与 Go 版本的对应关系

`Cargo.toml` 的 `package.metadata.porting.go-package = "br/pkg/encryption"` 明确了对照包，核心实现对应 `br/pkg/encryption/manager.go`。结构上的对应关系是：Go 包级 `Manager`、`NewManager`、`(*Manager).Decrypt`、`(*Manager).Close` 被移植到 `manager.rs`，`lib.rs` 负责模拟 Go 包级扁平命名空间。

主要一致点包括：两份配置的 nil/None 防御；明文 data-key 配置优先；无有效加密时返回 nil/`None`；主密钥模式只取第一份加密 data key；错误上下文；以及 Close 向后端转发。`parity_test.rs` 用 NIST AES-128-CTR 向量和明确错误字符串验证这些契约。

已确认的表达差异包括：Go 数据类型来自 kvproto，Rust 当前使用本地 enum/struct；Go `Decrypt` 把 `context.Context` 传给主密钥后端，Rust 方法没有 context；Go `Close` 可安全接收 nil receiver，Rust 必须先有一个 `Manager` 实例；Go 错误使用 `errors.Trace/Annotate`，Rust 使用 `String` 拼接；Go protobuf oneof 通过动态类型分支，Rust 用 `FileEncryptionMode` enum 表达。

同目录没有 `manager_test.go`。Rust 的直接管理器测试集中在独立的 `br/pkg/encryption/parity_test.rs`；Go 的 `master_key/*_test.go` 验证下游主密钥后端，但不能代替管理器层测试证据。

## 扩展指南

- 新增管理器行为或数据模式应修改 `manager.rs`，保持 `lib.rs` 只做模块装配；只有确需 crate 根公开时才使用 `pub`，并评估 glob re-export 对兼容面的影响。
- 新增文件加密模式时，需要同步 `FileEncryptionMode`、`Manager::Decrypt` 分支、Go `manager.go`/protobuf 语义，以及独立的 `parity_test.rs`；不要把测试嵌入 `lib.rs` 或 `manager.rs`。
- 若要支持多份 encrypted data key 的回退，必须先明确 Go 的选择顺序、错误优先级和后端匹配规则，不能简单遍历到成功就弱化当前“使用第一份”的兼容契约。
- 若要补 context 取消、并行 KMS 调用或 key 清零，应同时审查 master-key crate 和 `MetadataHelper` 的锁持有范围；这类改变涉及兼容性、阻塞时间、敏感数据驻留与资源关闭顺序。
- 修改解密次序时要保持流模块的“密文 checksum 在解密前、解压在解密后”不变量，否则会改变可观察错误优先级和数据解释方式。
- 外部 Rust 依赖必须按仓库规则在独立上游仓库移植、提交并发布 tag，再由统一 tag 的 Git 依赖引用；不能复制进 vendor/third_party 或用本地 `[patch]`。

本任务只新增说明文档，不修改 Rust/Go/Cargo。后续行为修改应同步最近的独立 Rust 测试，保留现有 PingCAP 版权，并在真正修复 Rust 生产代码后遵守仓库的 AsterSQL 版权标记要求。

## 验证依据

本说明基于以下直接证据：

- `br/pkg/encryption/lib.rs`：crate lint 属性、`manager` 公开模块、`cfg(test)` 独立测试模块和根级 glob re-export。
- `br/pkg/encryption/Cargo.toml` 与根 `Cargo.toml`：crate 名、`[lib]` 路径、Go package metadata、两个直接依赖及 workspace member；`br/pkg/stream/Cargo.toml`：反向路径依赖。
- `br/pkg/encryption/manager.rs`：全部公开类型、`IsEffectiveEncryptionMethod`、`DecryptContent`、`NewManager`、`Manager::{Decrypt, Close}` 及其错误分支。
- Go 对照 `br/pkg/encryption/manager.go`：构造优先级、context、protobuf 类型、第一份 encrypted data key、错误注释和关闭语义。
- Rust 独立测试 `br/pkg/encryption/parity_test.rs`：公开契约、真实 AES-CTR 向量、空输入、非法算法、空主密钥列表和 unset mode。
- 生产调用证据 `br/pkg/stream/stream_mgr.rs` 与测试 `br/pkg/stream/stream_mgr_test.rs`：管理器注入、共享锁、checksum/解密/解压顺序、缺少管理器错误和 Close 转发。
- RustCodeGraph：`status` 显示索引覆盖 7032 个 Rust 文件；`files --filter br/pkg/encryption` 列出目标、实现、Go 对照和独立测试；`node --file` 核对 `lib.rs`、`manager.rs`、`parity_test.rs` 与流消费源码；`query/node NewManager` 核对定义及对 `IsEffectiveEncryptionMethod` 的调用轨迹。
- `rg`：核对 workspace、Cargo 正反向依赖、crate 名称引用、`NewManager`/`Decrypt`/`Close` 的直接使用位置，以及同目录不存在 `manager_test.go`。

本任务为纯文档分析，按计划不运行 Cargo。结构验证要求文档存在并恰好包含规定的十一个二级标题；关于仓库外调用者、未接线的生产构造入口和 master-key 内部并发语义均不作无证据推断。
