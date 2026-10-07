# `br/pkg/encryption/master_key/multi_master_key_backend.rs`

## 文件定位

本文件属于 Cargo crate `astersql-br-pkg-encryption-master-key`，crate 边界由同目录 [`Cargo.toml`](./Cargo.toml) 定义，模块由 [`lib.rs`](./lib.rs) 以 `pub mod multi_master_key_backend` 装配并公开重导出。它位于 BR 文件加密的主密钥层：上层 [`br/pkg/encryption/manager.rs`](../manager.rs) 在主密钥加密模式下通过 `NewManager` 构造此后端，再由 `Manager::Decrypt` 用它解开数据密钥；本文件不直接解密备份文件正文。

RustCodeGraph 对目标文件的索引显示 5 个符号，并确认独立测试 [`multi_master_key_backend_test.rs`](./multi_master_key_backend_test.rs) 直接引用它。结合 `manager.rs` 的源码接线，可以确认它既是公开 crate API，也是加密管理器内部的顺序回退组件，而不是未接线的桩。

## 核心职责

`MultiMasterKeyBackend` 把若干 `AnyBackend` 组合成一个“依次尝试”的解密后端，目的与 Go 文件注释所述的主密钥高可用方向一致：同一份 `EncryptedContent` 交给各后端，任一后端成功即返回明文，全部失败才汇总错误。目前上层 `Manager::Decrypt` 只取 `DataKeyEncryptedContent[0]`，因此“多个加密数据密钥”的未来扩展尚未在这里实现；当前多后端能力是对同一个密文进行回退。

本文件还负责两项生命周期工作：构造时把 protobuf 风格的 `MasterKey` 配置逐一转换为具体后端，关闭时逐一调用底层后端的 `Close`。它不负责加密、密钥轮换、重试、并行竞速或后端健康检查。

## 主要符号

- `pub struct MultiMasterKeyBackend { backends: Vec<AnyBackend> }`：公开类型、私有后端列表。私有字段保证生产调用者只能通过构造函数建立列表，测试若需注入任意 trait 实现则使用独立的测试替身。
- `pub fn NewMultiMasterKeyBackend(masterKeysProto: Option<&[MasterKey]>) -> Result<MultiMasterKeyBackend, String>`：公开构造入口。`None` 返回 `must provide at least one master key`；对切片中的每项调用 [`master_key.rs`](./master_key.rs) 的 `CreateBackend`，按输入顺序保存结果。`Vec::with_capacity(5)` 只是初始容量，不是最多五个后端的限制。
- `pub fn MultiMasterKeyBackend::Decrypt(&self, encryptedContent: &EncryptedContent) -> Result<Vec<u8>, String>`：顺序回退解密。空列表先返回内部不变量错误；非空列表按顺序调用 `Backend::Decrypt`，首次成功即短路，全部失败则合并错误文本。
- `pub fn MultiMasterKeyBackend::Close(&mut self)`：按保存顺序调用每个 `AnyBackend` 的 `Backend::Close`。接口无返回值，无法向上传播关闭失败。
- `AnyBackend`、`Backend`、`CreateBackend`：均定义在 [`master_key.rs`](./master_key.rs)。`AnyBackend` 当前只有 `File(FileBackend)` 与 `Kms(KmsBackend)` 两种生产变体，并把 `Decrypt`/`Close` 分派给具体实现。

## 执行流程

构造流程如下：

1. `NewMultiMasterKeyBackend` 检查外层 `Option`；只有 `None` 立即失败。
2. 以容量 5 创建空 `Vec`，随后保持配置切片顺序遍历每个 `MasterKey`。
3. 每项交给 `CreateBackend`。文件配置创建 `FileBackend`，KMS 配置创建对应云后端；明文、未设置、未知厂商或尚未实现的 Azure 配置会失败。
4. 任一项构造失败时，`?` 立即返回该字符串错误；全部成功后把列表封装为 `MultiMasterKeyBackend`。

解密流程如下：

1. 若 `backends` 为空，返回 `internal error: should always contain at least one backend`。
2. 按配置顺序调用每个后端的 `Decrypt`。成功结果立即返回，后续后端不会被调用。
3. 失败文本按发生顺序收集。若所有后端均失败，返回带 `failed to decrypt in multi master key backend:` 前缀、并以 `; ` 连接各错误的单个 `String`。

在应用主链中，`manager.rs::NewManager` 仅当 `masterKeyConfigs.EncryptionType` 是有效加密方法时构造该类型；`Manager::Decrypt` 在 `FileEncryptionMode::MasterKeyBased` 分支用它解出数据密钥，再调用内容解密函数处理正文。`Manager::Close` 将关闭动作转交给本类型。

## 数据与状态

唯一持久状态是 `Vec<AnyBackend>`。列表顺序同时决定构造顺序、解密优先级、错误排列顺序和关闭顺序，因此不能在不评估兼容性与性能的情况下排序或去重。`Decrypt` 只借用列表，不在成功或失败后记录首选后端、失败次数或熔断状态；每次调用都从索引 0 重新开始。

构造函数对 `Some(&[])` 会成功并产生空列表。此状态随后由 `Decrypt` 的内部错误分支防守。该行为不是“至少一个”的理想不变量，而是 [`parity_test.rs`](./parity_test.rs) 明确固定的 Go 兼容现状：Go 构造条件使用 `masterKeysProto == nil && len(masterKeysProto) == 0`，因此非 nil 空切片同样能通过构造。

输入 `EncryptedContent` 只读借用；成功时返回由具体后端生成的新 `Vec<u8>`。失败列表是单次调用内的局部变量，函数返回后即释放。本文件不缓存密文、明文或密钥材料。

## 依赖与调用关系

上游关系：

- [`lib.rs`](./lib.rs) 声明并重导出本模块，使 `NewMultiMasterKeyBackend` 和类型成为 crate API。
- [`br/pkg/encryption/manager.rs`](../manager.rs) 的 `NewManager` 是已验证的 Rust 生产构造入口；`Manager::Decrypt` 和 `Manager::Close` 分别消费其解密与关闭能力。
- [`multi_master_key_backend_test.rs`](./multi_master_key_backend_test.rs) 直接验证顺序短路、回退、错误聚合、空列表和真实文件后端组合；[`parity_test.rs`](./parity_test.rs) 固定公开兼容契约。

下游关系：

- `CreateBackend` 将 `MasterKey` 配置转换为 `AnyBackend`。其文件分支依赖 `FileBackend`，云分支依赖 `KmsBackend` 以及 crate `astersql-br-pkg-kms`。
- `Backend::Decrypt` 是本文件的核心下游调用；具体文件后端执行本地 AES-GCM 解密，KMS 后端委托云 provider 解开数据密钥。
- `Backend::Close` 是资源释放边界。本文件只负责遍历，不了解具体后端内部资源。

同目录 `Cargo.toml` 直接声明 `astersql-br-pkg-kms`、`aes-gcm` 和 `rand`；本文件自身只直接使用 crate 内类型，密码算法与云 SDK 依赖分别被具体后端封装。

## 错误处理与边界

所有错误都以 `String` 表示，没有可供调用者模式匹配的错误枚举。构造阶段采用失败即停：一个配置失败会阻止整个组合后端产生，错误文本由 `CreateBackend` 原样传播。本文件没有为构造错误补充配置索引或后端标识，因此排查多个相似配置时需结合上游配置顺序。

解密阶段采用成功即停、失败继续：早期错误不会遮蔽后续成功；全部失败时所有错误文本均保留并按顺序拼接。空列表错误与“全部非空后端失败”错误相互区分。`encryptedContent` 的格式、元数据和认证失败由具体后端校验，本层不预检也不改写单个错误。

需特别注意这些边界：`None` 与空切片行为不同；初始容量 5 不限制数量；成功后不验证其他后端；`Close` 无错误通道；Rust `Decrypt` 不接收 Go 版本的 `context.Context`，因此本层无法直接传播取消或截止时间，KMS 路径只能依赖下游现有接口语义。

## 并发与资源生命周期

`Decrypt` 接收 `&self`，自身没有可变共享状态、锁、通道、任务或后台线程，并且串行调用后端，不会并行请求多个 KMS。串行设计保证确定的优先级与短路语义，但首个慢后端会阻塞后续回退；新增并行策略会改变调用次数、延迟、云成本和错误顺序，不能视为等价重构。

本类型没有实现自定义 `Drop`；资源收尾依赖上层显式调用 `Close`。`Manager::Close` 已提供这条生产释放链。`Close(&mut self)` 要求独占可变借用，并按顺序关闭全部后端，但不会清空列表，也没有“只关闭一次”的本地标志，故幂等性取决于各具体后端。构造中途失败时，已创建元素随局部 `Vec` 被丢弃，但本文件不会显式逐个调用 `Backend::Close`；若未来后端必须显式释放资源，应同步审视该失败路径。

本文件没有声明 `Send`/`Sync` 约束，也不负责跨线程共享。是否能安全跨线程使用由 `AnyBackend` 的具体字段及编译器自动 trait 决定，文档不能据此承诺并发调用契约。

## 与 Go 版本的对应关系

直接对照文件是 [`multi_master_key_backend.go`](./multi_master_key_backend.go)，总体结构逐项对应：同名组合类型、默认初始容量 5、逐项 `CreateBackend`、有序短路解密、全失败聚合，以及逐项关闭。Rust 的 `Option<&[MasterKey]>` 表达 Go 的 nil/非 nil 切片差异，并刻意保留“nil 被拒绝、非 nil 空切片可构造”的当前行为。

主要实现差异有三点。第一，Go 字段是 `[]Backend` trait 接口，可直接注入 mock；Rust 生产字段收窄为 `Vec<AnyBackend>`，所以独立 Rust 测试另建 `Vec<Box<dyn Backend + Send>>` 的测试组合来验证 mock 调用次数，并额外用真实文件后端验证生产构造链。第二，Go `Decrypt` 接受 `context.Context`，Rust 同名方法没有上下文参数。第三，Go 使用 `multierr.Append` 与 `errors.Wrap` 保留错误链，Rust 将字符串收集后以 `; ` 拼接，保留可读消息但不保留结构化错误链。

[`multi_master_key_backend_test.go`](./multi_master_key_backend_test.go) 与 Rust 独立测试共同覆盖：首个成功时后端短路、首个失败后第二个成功、全部失败包含每条错误、空列表报内部错误。Rust 的 [`parity_test.rs`](./parity_test.rs) 还覆盖构造、关闭以及空切片兼容行为。

## 扩展指南

新增后端种类时，应先在 `master_key.rs::AnyBackend`、`Backend` 分派和 `CreateBackend` 中完成生产接线，本文件通常只需继续保存 `AnyBackend`；同时扩展独立测试，覆盖构造失败、回退顺序、错误文本和 `Close`。不要把测试逻辑嵌入生产 `.rs` 文件，测试应继续放在同目录独立 `*_test.rs` 中。

若要实现真正的多主密钥轮换或高可用，需要同时检查上层 `manager.rs` 当前只选取 `DataKeyEncryptedContent[0]` 的限制、密文与后端之间的标识关系、旧密钥保留策略及错误可观测性。仅增加本文件的后端数量不能自动解密分别由不同主密钥产生的多份数据密钥。

若要增加并行竞速、首选后端缓存或熔断，至少应补充确定性测试来约束短路、调用次数、取消、超时、错误排序和关闭期间竞态，并评估 KMS 请求成本。若修正空切片构造行为或引入结构化错误，必须同步 Go 对照或明确记录兼容性偏差，并更新 `parity_test.rs`。若后端引入必须显式释放的资源，还应让构造失败路径关闭已成功创建的前序后端。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 7032 个 Rust 文件；`files --filter br/pkg/encryption/master_key` 定位目标、模块、Go 对照和测试；`node --file br/pkg/encryption/master_key/multi_master_key_backend.rs --offset 1 --limit 260` 读取目标文件全部 68 行并报告其被独立 Rust 测试使用；对 `NewMultiMasterKeyBackend`、`Decrypt`、`Close` 执行了限定目标文件的 callers/callees 查询。
- 生产源码：[`multi_master_key_backend.rs`](./multi_master_key_backend.rs)、[`master_key.rs`](./master_key.rs)、[`lib.rs`](./lib.rs)、[`br/pkg/encryption/manager.rs`](../manager.rs)。这些文件分别证明组合算法、具体后端工厂、模块公开边界和上层生产调用链。
- crate 配置：[`Cargo.toml`](./Cargo.toml)，证明 crate 名称、library 入口、Go 包映射和直接依赖。
- Go 对照：[`multi_master_key_backend.go`](./multi_master_key_backend.go) 与 [`br/pkg/encryption/manager.go`](../manager.go)，用于核对构造条件、顺序回退、错误聚合、上下文参数和应用接线。
- 测试证据：[`multi_master_key_backend_test.rs`](./multi_master_key_backend_test.rs)、[`multi_master_key_backend_test.go`](./multi_master_key_backend_test.go)、[`parity_test.rs`](./parity_test.rs)，覆盖短路、回退、全失败、空列表、真实文件后端和 nil/空切片兼容边界。
- 本任务是纯文档分析，按计划不运行 Cargo；交付前使用任务指定命令验证目标文档存在且恰有 11 个固定二级标题，并人工复核没有把未来设计写成当前能力。
