# `br/pkg/kms/common.rs`

## 文件定位

[`common.rs`](./common.rs) 属于 `astersql-br-pkg-kms` library crate；[`Cargo.toml`](./Cargo.toml) 把 crate 根指定为 [`lib.rs`](./lib.rs)，后者以 `pub mod common` 装载本文件，并通过 `pub use common::*` 把全部公开项提升到 crate 根。`package.metadata.porting.go-package = "br/pkg/kms"` 明确表明它是同路径 [`common.go`](./common.go) 的 Rust 移植。

本文件位于 KMS 厂商实现与上层主密钥后端之间：它不访问 AWS/GCP，也不执行加解密，而是为“被 KMS 包装的密文数据密钥”和“已解包的明文数据密钥”提供不同类型，并集中约束算法标签和密钥长度。RustCodeGraph 将其识别为 10 个符号，并记录 `br/pkg/encryption/master_key/kms_backend.rs`、`mem_backend.rs` 与 `br/pkg/kms/parity_test.rs` 等消费者。

## 核心职责

本文件承担三项职责：

1. `EncryptedKey` 将任意非空密文字节包装成独立类型，避免与明文材料在接口上混用；`Equal` 为上层 KMS 后端的密文 key 缓存提供按字节判等。
2. `CryptographyType` 固定明文与 AES-GCM-256 的协议判别值，并通过 `TargetKeySize` 把算法映射到长度约束。
3. `PlainKey` 把算法标签与明文字节绑定；`NewPlainKey` 在构造期校验需要固定长度的算法，`KeyTag` 与 `Key` 分别提供标签和值的只读访问。

它不验证密文格式或来源，不调用 KMS，不选择云厂商，不保存全局 key，也不清零内存；这些边界使该文件保持为小型值对象与输入门禁层。

## 主要符号

- `EncryptedKey(pub Vec<u8>)`：公开元组结构，拥有密文字节；派生 `Clone`、`Debug`、`Default`、`PartialEq`、`Eq`。`Default` 或直接构造可以得到空值，因此“非空”只由 `NewEncryptedKey` 保证，不是类型级强不变量。
- `NewEncryptedKey(Vec<u8>) -> Result<EncryptedKey, String>`：拒绝空向量，成功时把输入所有权直接移入包装，不额外复制。
- `EncryptedKey::Equal(&self, &EncryptedKey) -> bool`：比较两个内部 `Vec<u8>` 的完整字节内容，不比较对象身份。
- `CryptographyType(pub i32)`：开放整数新类型，允许保留 Go `type CryptographyType int` 可表示未知值的语义。
- `CryptographyTypePlain = 0`、`CryptographyTypeAesGcm256 = 1`：与 Go `iota` 顺序一致的关联常量。
- `CryptographyType::TargetKeySize(self) -> usize`：AES-GCM-256 返回 32；明文及所有未知判别值返回 0。
- `PlainKey { pub tag, pub key }`：拥有算法标签和明文字节的公开结构；同样可绕过构造器直接创建，调用方应优先使用 `NewPlainKey`。
- `NewPlainKey(Vec<u8>, CryptographyType) -> Result<PlainKey, String>`：仅当 `TargetKeySize() > 0` 时要求长度精确相等。
- `PlainKey::KeyTag() -> CryptographyType`：按值返回复制型标签。
- `PlainKey::Key() -> &[u8]`：零拷贝返回明文 key 的只读借用，生命周期不超过 `PlainKey`。

## 执行流程

密文 key 进入上层解密链时，`KmsBackend::DecryptWithContext` 从 `EncryptedContent.Metadata` 取得 `kms_ciphertext_key` 字节并调用 `NewEncryptedKey`。空值立即失败；成功值先与缓存中的 `cachedCiphertextKey` 调用 `Equal`。字节相等则复用已有 `MemAesGcmBackend`，不同则调用云 KMS Provider 解包，因此这里的判等决定是否发生远程解密请求。

解包得到明文字节后，`KmsBackend::DecryptWithContext` 用 `CryptographyTypeAesGcm256` 调用 `NewPlainKey`。构造器先调用 `TargetKeySize` 得到 32，再比较输入长度；通过后把字节与标签绑定，并由 `Key()` 交给 `NewMemAesGcmBackend`。`mem_backend.rs::NewMemAesGcmBackend` 自身也执行同样的 `NewPlainKey(...AesGcm256)` 门禁，确保直接构造内存后端时仍要求 256 位 key。

Plain 或未知算法走另一分支：`TargetKeySize` 返回 0，`NewPlainKey` 跳过长度校验并原样保存输入。`parity_test.rs::unknown_cryptography_type_matches_go_default_branch` 明确固定了未知值 `CryptographyType(99)` 可携带任意长度字节的当前契约；这不是“已支持未知算法”，只是与 Go 默认分支一致的表示行为。

## 数据与状态

三个类型都拥有其数据，没有借用外部缓冲区。`NewEncryptedKey` 与 `NewPlainKey` 接收 `Vec<u8>` 并移动所有权，成功路径不复制；`Clone` 则会深拷贝向量。`Key()` 只借用内部字节，不能在 `PlainKey` 被释放后继续使用。

模块没有静态可变状态、缓存、锁或延迟初始化。算法标签是 `i32` 新类型而非封闭枚举：值 0 表示 Plain、值 1 表示 AES-GCM-256，其他整数仍可保存。长度 0 在本协议中表示“不施加长度限制”，而不是“密钥必须为空”；因此 Plain 和未知类型均接受空或非空 key。相反，`EncryptedKey` 的安全构造入口始终拒绝空值。

`EncryptedKey.0` 与 `PlainKey` 两个字段均为公开字段，外部代码可以直接构造空密文、错误长度的 AES key，或在构造后修改字节。文档所述校验保证仅适用于构造函数返回的值；维护者不能把公开结构本身误当成不可破坏的不变量。

## 依赖与调用关系

本文件只依赖 Rust 标准库的 `Vec`、切片、比较与字符串格式化；[`Cargo.toml`](./Cargo.toml) 中 AWS、GCP、Tokio 等依赖服务于同 crate 的其他模块，`common.rs` 本身不直接使用它们。crate 根的重导出允许外部 crate 使用 `astersql_br_pkg_kms::{EncryptedKey, NewEncryptedKey, CryptographyType, NewPlainKey, PlainKey}`。

已核验的关键调用关系如下：

- `lib.rs -> common.rs`：声明模块并重导出公开 API。
- `encryption/master_key/kms_backend.rs::DecryptWithContext -> NewEncryptedKey -> EncryptedKey::Equal`：校验元数据中的密文 key，并以字节相等决定缓存命中。
- `KmsBackend::DecryptWithContext -> Provider::DecryptDataKey -> NewPlainKey(AesGcm256) -> PlainKey::Key -> NewMemAesGcmBackend`：把远端 KMS 解包结果限制为 32 字节后交给本地 AES-GCM 后端。
- `encryption/master_key/mem_backend.rs::NewMemAesGcmBackend -> NewPlainKey(AesGcm256)`：直接构造本地后端时复用相同长度门禁；加解密再通过 `PlainKey::Key` 取得字节。
- `kms/parity_test.rs` 直接覆盖两个构造器、`Equal`、`TargetKeySize`、`KeyTag` 和 `Key`。

RustCodeGraph 的 `explore` 输出确认 `NewEncryptedKey` 的生产消费者是 `kms_backend.rs::DecryptWithContext`，`NewPlainKey` 的生产消费者包括该函数，并列出 parity 测试调用；其宽泛名称查询还可能混入 Go 或其他模块同名符号。精确 `callers/callees` 对带文件限定的目标未返回边，因此上述完整局部链以图的文件使用关系和源码引用搜索交叉核验，不宣称为全仓库闭包。

## 错误处理与边界

`NewEncryptedKey` 唯一失败条件是 `key.is_empty()`，错误文本固定为 `encrypted key cannot be empty`。它不检查最小/最大长度、编码、供应商、CRC 或真实性；任意非空字节都是合法包装值。上层 `kms_backend.rs` 会把该错误加上 `failed to create encrypted key` 上下文。

`NewPlainKey` 先求目标长度；只有目标大于 0 且实际长度不等时失败。AES-GCM-256 的 0、31、33 等所有非 32 长度都返回 `encryption method and key length mismatch, expect 32 got {actual}`。32 字节内容本身不做熵、全零或来源验证。Plain 和未知类型的目标长度为 0，因而任何长度均成功；新增算法若忘记在 `TargetKeySize` 中建立非零映射，也会静默继承这一不限长行为。

所有错误使用 `String`，没有结构化错误类别或错误源链。函数无 panic 分支；但公开字段和元组构造器允许调用方绕过错误路径。`Equal` 与派生 `PartialEq` 都是普通字节比较，不承诺常数时间，因此不应被扩展为认证标签或口令验证接口。

## 并发与资源生命周期

本模块不创建线程、异步任务、锁、通道、文件或网络连接，也没有需要 `Close` 的资源。各构造与访问方法只处理传入值或不可变借用，彼此没有共享状态，因而可由多个线程独立调用；类型是否在线程间移动由其标准库字段自动决定。

密钥字节随拥有它的 `EncryptedKey`/`PlainKey` 生命周期存在，释放时由 `Vec` 正常回收，但代码没有显式内存清零，派生 `Debug` 还会把原始字节格式化出来。调用方应避免在日志中使用这些值的 `Debug` 输出；若安全要求升级，应统一评估 `zeroize`、调试脱敏、字段可见性和克隆策略，而不是只在一个调用点擦除临时副本。

上层并发缓存位于 `kms_backend.rs::KmsBackend.cached: Mutex<Option<CachedKeys>>`，不属于本文件。`EncryptedKey::Equal` 在锁内被调用，但自身不加锁也不修改数据。

## 与 Go 版本的对应关系

[`common.go`](./common.go) 是直接语义基准。两端都拒绝空 `EncryptedKey`，按字节比较密文 key；`CryptographyTypePlain`/`CryptographyTypeAesGcm256` 分别为 0/1；AES-GCM-256 目标长度为 32，Plain 与未知类型为 0；`NewPlainKey` 只在目标长度非零时精确校验，并提供算法标签与字节访问。

实现差异主要来自语言所有权：Go 的 `EncryptedKey` 是 `[]byte` 别名且构造器返回原 slice，Rust 用拥有 `Vec<u8>` 的元组结构；Go 返回 `*PlainKey`，Rust 按值返回 `PlainKey`；Go `Key()` 返回 `[]byte`，Rust 返回不能修改的 `&[u8]`。Go 的 `Equal` 参数是 `*EncryptedKey`，可因 nil 指针使用不当而 panic；Rust 要求有效引用。Go 用 `pingcap/errors`，Rust 使用 `String`，但现有错误文本保持一致。

Go 目录没有独立 `common_test.go`。Rust 的直接契约集中在 `parity_test.rs::common_key_errors_and_plain_key_boundaries_match_go`、`unknown_cryptography_type_matches_go_default_branch` 与 `go_rust_public_contract_matches`；上层 `encryption/master_key/kms_backend_test.rs` 验证相同密文 key 命中缓存、不同 key 再调 Provider，`mem_backend_test.rs::test_new_mem_aes_gcm_backend` 验证 32 字节成功、16 字节失败。

## 扩展指南

- 新增加密算法时，必须为其分配与 Go/持久化协议一致的判别值，并同步更新 `TargetKeySize`；若算法允许多种长度，当前单一 `usize` 模型不足，应明确重构校验接口，不能用返回 0 假装“已校验”。
- 修改 AES-GCM-256 长度、Plain 默认行为或未知值分支会改变跨语言契约及上层后端构造语义，需要同步 `common.go`、`kms/parity_test.rs`、`encryption/master_key/mem_backend_test.rs` 和 `kms_backend_test.rs`。Rust 单元测试应继续放在独立测试文件，不能嵌回 `common.rs`。
- 强化不变量时，可考虑收窄 `EncryptedKey.0`、`PlainKey.tag/key` 与 `CryptographyType.0` 的可见性并增加安全访问器；这属于公开 API 兼容变化，必须先盘点 `lib.rs` 重导出的下游 crate。
- 若加入敏感内存清零或调试脱敏，应同时处理 `Clone`、`Debug`、错误路径、缓存替换和 Provider 返回缓冲区，避免只清零最终包装值却遗留副本。
- 性能方面，成功构造应保持移动 `Vec`、`Key()` 保持零拷贝；`Equal` 是 O(n) 字节比较，并在 KMS 缓存锁内执行，未来若密文 key 可能很大，应评估锁持有时间，但不能用有碰撞风险的非认证摘要替代正确判等。

## 验证依据

事实核验读取了：`br/pkg/kms/common.rs`、`common.go`、`Cargo.toml`、`lib.rs`、`parity_test.rs`、`kms.rs`，以及直接消费端 `br/pkg/encryption/master_key/kms_backend.rs`、`kms_backend.go`、`kms_backend_test.rs`、`mem_backend.rs`、`mem_backend.go`、`mem_backend_test.rs`。目标包不存在 `doc.go`，同目录也不存在独立 `common_test.rs`/`common_test.go`。

RustCodeGraph 证据包括：`status`（索引含 7032 个 Rust 文件）、`files --filter br/pkg/kms`、`node --file br/pkg/kms/common.rs`、`explore "br/pkg/kms/common.rs main symbols callers callees role in KMS"`，以及 `NewEncryptedKey`、`NewPlainKey`、`TargetKeySize` 等查询。图给出了文件级消费者和关键调用者；精确限定符号的 `callers/callees` 无输出，故以 `rg` 的直接引用结果补足并在文中限定证据范围。

独立 Rust 测试所固定的行为包括：空密文报错、密文按字节相等、AES-GCM-256 的 32 字节成功与错误长度失败、Plain 不限长、未知算法沿 Go 默认分支不限长、访问器返回原标签/字节，以及上层缓存对相同/不同密文 key 的分流。按任务约束这是纯文档分析，未运行 Cargo 或代码测试；交付只执行任务指定的 11 章节结构检查、文档差异检查和人工事实复核。
