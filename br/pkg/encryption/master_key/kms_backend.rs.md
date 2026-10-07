# `br/pkg/encryption/master_key/kms_backend.rs`

## 文件定位

本文件属于 Cargo crate `astersql-br-pkg-encryption-master-key`；crate 的清单是 [`br/pkg/encryption/master_key/Cargo.toml`](Cargo.toml)，入口 [`lib.rs`](lib.rs) 通过 `pub mod kms_backend` 加载本模块并重导出其公开符号。它位于备份恢复（BR）的信封加密解密链路中：备份元数据携带 KMS 厂商名和被 KMS 加密的数据密钥，本模块先向云 KMS 解开数据密钥，再把明文数据密钥交给内存 AES-256-GCM 后端解密实际内容。

上游装配入口在 [`master_key.rs`](master_key.rs) 的 `CreateBackend`、`createCloudBackend` 和 `CreateKmsBackendWithProvider`。AWS/GCP 配置先被构造成 `astersql-br-pkg-kms` 的 `Provider`，随后由 `NewKmsBackend` 包装成 `AnyBackend::Kms`；Azure 分支当前在装配层直接返回未实现错误，不会进入本文件。模块不是独立进程入口，也不负责创建云厂商客户端、解析主密钥配置或执行内容加密。

## 核心职责

`KmsBackend` 承担四项聚焦职责：

1. 校验 `EncryptedContent.Metadata` 中的 `kms_vendor` 与当前 `Provider::Name` 一致，并取得 `kms_ciphertext_key`。
2. 把密文数据密钥包装为 `EncryptedKey`，在缓存未命中时调用 `Provider::DecryptDataKey`，并按固定策略重试。
3. 强制把 KMS 返回值校验为 32 字节 AES-GCM-256 明文密钥，构造 `MemAesGcmBackend`，再解密内容。
4. 只缓存最近一次“密文数据密钥 → 内存解密后端”的映射，并把 `Close` 转发给 KMS provider。

它不缓存多个 key，不持久化明文 key，也不刷新或轮换 provider 凭据。缓存只避免同一 `KmsBackend` 实例对相同密文 key 重复访问 KMS。

## 主要符号

- `CachedKeys`：一个成功解封后的缓存条目。`encryptionBackend: MemAesGcmBackend` 持有已校验的明文 AES key；`cachedCiphertextKey: EncryptedKey` 是比较缓存命中的标识。两个字段公开，但该结构本身只在本文件内被构造和读取。
- `KmsBackend`：后端主体。`cached: Mutex<Option<CachedKeys>>` 初始为 `None`，只保存最新条目；`kmsProvider: Box<dyn Provider + Send>` 持有动态分派的云 KMS 实现。
- `NewKmsBackend(Box<dyn Provider + Send>) -> Result<KmsBackend, String>`：构造空缓存后端。当前构造过程没有可失败步骤，但保留 `Result` 以对齐 Go 构造函数和上游统一错误接口。
- `KmsBackend::Decrypt(&self, &EncryptedContent)`：便利入口，以全新的默认 `Context` 调用 `DecryptWithContext`；该入口没有调用方可用的取消句柄。
- `KmsBackend::DecryptWithContext(&self, &Context, &EncryptedContent)`：核心路径，完成元数据校验、缓存判断、KMS 解封、明文 key 校验、内存后端建立和内容解密。
- `KmsBackend::Close(&mut self)`：直接调用 `Provider::Close`，不返回错误，也不显式清空缓存。
- `with_retry`：私有同步重试器。最多调用闭包 10 次，初始延迟变量为 500 ms；每次失败后先翻倍再休眠，因此实际失败后的等待从 1 s 开始，并封顶 5 s。全部失败时用 `"; "` 拼接各次错误。

本文件没有条件编译项、异步函数或自建后台任务。

## 执行流程

从应用装配到一次解密的主流程如下：

1. [`master_key.rs`](master_key.rs) 的 `CreateBackend` 根据 `MasterKeyBackend::Kms` 进入 `createCloudBackend`，按厂商创建 AWS/GCP provider，再由 `CreateKmsBackendWithProvider` 调用 `NewKmsBackend`。
2. `Decrypt` 创建 `Context::default()`；需要传播取消信号的调用者应直接使用 `DecryptWithContext`。
3. `DecryptWithContext` 先读取 provider 名称，然后校验 `MetadataKeyKmsVendor`。缺字段立即报错；字节值与 provider 名称不相等也立即报错。
4. 函数取得 `MetadataKeyKmsCiphertextKey`，并用 `NewEncryptedKey` 拒绝空密文 key。以上检查都发生在加锁和网络调用之前。
5. 函数锁住 `cached`。若缓存存在且 `EncryptedKey::Equal` 做出的逐字节比较命中，直接复用其中的 `MemAesGcmBackend::DecryptContent`，不调用 KMS。
6. 缓存未命中时，`with_retry` 调用 `Provider::DecryptDataKey(ctx, ciphertext_key)`。首次成功立即停止；失败则累计错误，在上下文已取消时停止，否则按指数退避同步休眠后继续。
7. KMS 返回的字节经 `NewPlainKey(..., CryptographyTypeAesGcm256)` 校验必须恰好 32 字节，再经 `NewMemAesGcmBackend` 构造内容解密器。
8. 新条目整体替换旧缓存，然后用新后端调用 `DecryptContent`。内容解密失败不会撤销已经写入的 key 缓存；下次相同密文 key 仍会复用它。

## 数据与状态

输入 `EncryptedContent` 包含密文字节 `Content` 和字符串到字节数组的 `Metadata`。本文件直接消费两个元数据键：[`common.rs`](common.rs) 定义的 `kms_vendor` 与 `kms_ciphertext_key`；实际 AES-GCM 解密还由 [`mem_backend.rs`](mem_backend.rs) 消费 `method`、`iv` 和 `aes_gcm_tag`。

状态变化集中在 `cached`：构造时为 `None`，第一次成功解封数据密钥后变为 `Some(CachedKeys)`；相同密文 key 不改变状态，不同 key 成功解封后覆盖旧条目。KMS 解封失败、明文 key 长度错误或内存后端创建失败都不会更新缓存。相反，缓存写入发生在内容认证/解密之前，因此内容格式或认证标签错误不会移除刚写入的缓存。

明文 key 存放在 `MemAesGcmBackend` 内的 `PlainKey(Vec<u8>)`，生命周期与缓存条目相同；当前类型没有显式清零（zeroize）逻辑。`Close` 也只关闭 provider，不抹除该内存状态。

## 依赖与调用关系

上游调用链由 RustCodeGraph 和源码共同确认：`CreateBackend` → `createCloudBackend` → `CreateKmsBackendWithProvider` → `NewKmsBackend`。统一的 `AnyBackend::Decrypt` 再分派到 `KmsBackend::Decrypt`。多主密钥后端 [`multi_master_key_backend.rs`](multi_master_key_backend.rs) 通过 `CreateBackend` 间接持有并尝试这些后端，而不是直接构造本类型。

本文件的直接下游依赖是：

- `crate::common`：提供 KMS 元数据键。
- `crate::pb::EncryptedContent`：定义待解密的内容及元数据载体。
- `crate::mem_backend`：用已解封的明文 key 完成 AES-256-GCM 内容解密。
- `astersql-br-pkg-kms`：相邻 path dependency `../../kms`，提供 `Provider`、`Context`、`EncryptedKey`、`PlainKey` 及算法标签；Cargo 清单另声明 `aes-gcm`、`rand`，但本文件并不直接使用后二者。
- 标准库 `Mutex`、`thread::sleep` 和 `Duration`：分别承载单条缓存的互斥访问和同步退避。

RustCodeGraph 显示本文件由独立测试模块 [`kms_backend_test.rs`](kms_backend_test.rs) 直接使用；生产装配关系则通过 [`master_key.rs`](master_key.rs) 的模块内调用建立。

## 错误处理与边界

错误统一以 `Result<_, String>` 传播，并在层级边界添加上下文：缺 vendor、vendor 不匹配、缺密文 key 各有稳定消息；`NewEncryptedKey` 的空值错误被包装为 `failed to create encrypted key`；KMS 重试耗尽和 32 字节校验失败被包装为 `decrypt encrypted key failed`；内存 AES 后端构造失败被包装为 `failed to create MemAesGcmBackend`。最终 AES 元数据缺失、算法不符、IV/标签错误或认证失败由 `MemAesGcmBackend::DecryptContent` 原样返回。

锁中毒通过 `cached.lock().map_err(|e| e.to_string())` 转成字符串错误。`with_retry` 对所有 provider 错误一视同仁，没有不可重试错误分类；10 次均失败时返回按发生顺序拼接的消息。取消只在某次 provider 调用失败后检查：即使上下文调用前已取消，当前实现仍会至少调用 provider 一次；若取消发生在 `thread::sleep` 期间，睡眠本身不可中断，且下一轮会先再次调用 provider，之后才检查取消。测试只固定了“provider 在第一次失败中触发取消时，不进入睡眠且只调用一次”的场景。

空密文 key、非 32 字节的解封结果及 AES-GCM 元数据错误都有下游显式校验。超大元数据、超长错误串和恶意 provider 的执行时间没有本文件级上限；provider 调用本身能否响应 `Context` 由具体实现负责。

## 并发与资源生命周期

缓存互斥锁从缓存检查开始，一直持有到 KMS 重试、明文 key/内存后端构造以及最终内容解密结束。这保证并发请求不会对同一未缓存 key 重复解封，也保证缓存替换与使用是原子的；代价是缓存未命中的远程调用和退避等待会阻塞同一实例上的其他解密请求，缓存命中的实际 AES 解密也被串行化。

`kmsProvider` 的 trait object 只有 `Send` 约束而没有 `Sync` 约束，因此类型声明本身没有承诺可由多个线程通过共享引用并发调用；`Mutex` 只保护缓存，不包裹 provider。若未来要把后端放入 `Arc` 并跨线程共享，需要先审查并明确 provider 的 `Sync` 契约，而不能仅依赖现有缓存锁。

`with_retry` 使用阻塞线程睡眠，没有 Tokio 任务或异步定时器。provider 由 `KmsBackend` 独占；显式 `Close(&mut self)` 要求独占可变借用并转发清理，但没有 `Drop` 自动调用，因此调用者负责在统一后端生命周期末尾执行 `Backend::Close`。缓存及其中明文 key 最终随对象析构释放，但不保证内存清零。

## 与 Go 版本的对应关系

Go 对照文件 [`kms_backend.go`](kms_backend.go) 具有相同的 `CachedKeys`/`KmsBackend` 状态模型、vendor 和密文 key 校验顺序、单条缓存策略、AES-GCM-256 长度约束以及 `Close` 转发。Go 的 `sync.Mutex` 与 Rust 的 `Mutex<Option<CachedKeys>>` 都覆盖 KMS 调用和内容解密，因此主要串行化语义一致。

Rust 将 Go 的 `Decrypt(ctx, content)` 拆为默认上下文的 `Decrypt(content)` 和显式 `DecryptWithContext(ctx, content)`，以适配当前 `Backend` trait；只有后者完整暴露 Go API 的取消能力。Go 使用 `utils.WithRetryV2(NewBackoffRetryAllErrorStrategy(10, 500ms, 5s))`；其策略也会在计算第一次等待前把延迟翻倍，所以 Rust 的 1 s 起步、5 s 封顶和最多 10 次尝试与当前 Go 实现一致。两端都会累计所有失败，但具体字符串拼接格式来自不同错误设施，调用方不应依赖跨语言完全相同的重试错误排版。

独立 Rust 测试 [`kms_backend_test.rs`](kms_backend_test.rs) 移植了 Go 测试 [`kms_backend_test.go`](kms_backend_test.go) 的核心契约：首次访问调用 provider、相同密文 key 命中缓存、不同 key 再次调用，以及三种元数据错误。Rust 额外覆盖了 `DecryptWithContext` 的即时取消传播。现有成功路径测试故意忽略最终 `DecryptContent` 结果，因为测试内容不是完整 AES-GCM 密文；它证明的是 KMS 调用计数和缓存行为，不证明内容解密成功。

## 扩展指南

- 新增缓存策略时，主要修改点是 `CachedKeys`、`KmsBackend::cached` 和 `DecryptWithContext` 的锁定区间。应在独立的 [`kms_backend_test.rs`](kms_backend_test.rs) 增加容量、淘汰、并发去重和 key 轮换测试，并评估明文 key 驻留及清零风险。
- 调整重试策略时，应修改私有 `with_retry`，并补充尝试次数、实际退避边界、预取消、睡眠期间取消及错误聚合测试；同时逐项核对 Go 的 `WithRetryV2`/`NewBackoffRetryAllErrorStrategy`，避免只让 Rust 测试通过而偏离 Go 行为。
- 新增 KMS 厂商通常不在本文件分支判断，而是在 [`master_key.rs`](master_key.rs) 的 `createCloudBackend` 创建新的 `Provider`；本文件只要求 provider 的 `Name` 与内容元数据编码一致。需同步 provider crate 测试、装配测试和本后端 vendor 匹配测试。
- 若要支持异步或高并发解密，必须同时设计 provider 的 `Sync` 约束、避免持锁跨网络/睡眠、处理同 key 请求合并，并保持“缓存只在成功获得且校验明文 key 后更新”的不变量。性能优化不能绕过 AES-GCM 内容认证。
- 若改变公开构造或错误类型，要同步 [`master_key.rs`](master_key.rs) 的 `AnyBackend` 接线和独立测试；测试逻辑继续放在 `kms_backend_test.rs`，不要嵌入生产源文件。

## 验证依据

- RustCodeGraph 索引状态：项目索引包含 7,032 个 Rust 文件；`files --filter br/pkg/encryption/master_key` 确认目标源码、模块入口、Go 对照及独立测试均在对应目录。
- RustCodeGraph 源码/符号检查：`node --file br/pkg/encryption/master_key/kms_backend.rs --offset 1 --limit 260` 完整读取 133 行实现；`query KmsBackend`、`query CreateKmsBackendWithProvider`、`query DecryptWithContext` 确认主要定义；`explore` 确认 `CreateBackend → createCloudBackend` 以及 `CreateKmsBackendWithProvider → NewKmsBackend` 的装配关系，并识别 `DecryptWithContext → with_retry/Provider::DecryptDataKey/NewPlainKey/NewMemAesGcmBackend` 的下游链路。
- 读取的 Rust/Cargo 直接证据：[`Cargo.toml`](Cargo.toml)、[`lib.rs`](lib.rs)、[`master_key.rs`](master_key.rs)、[`common.rs`](common.rs)、[`mem_backend.rs`](mem_backend.rs)、[`../../kms/kms.rs`](../../kms/kms.rs)、[`../../kms/common.rs`](../../kms/common.rs) 和 [`kms_backend_test.rs`](kms_backend_test.rs)。目标目录没有 `doc.go`。
- 读取的 Go 对照证据：[`kms_backend.go`](kms_backend.go)、[`kms_backend_test.go`](kms_backend_test.go)、[`../../utils/retry.go`](../../utils/retry.go) 和 [`../../utils/backoff.go`](../../utils/backoff.go)。
- 本任务是纯文档分析，按计划不运行 Cargo；交付前另运行固定 11 章节结构检查，并人工复核唯一生产物、路径链接、调用关系、错误与并发生命周期描述。
