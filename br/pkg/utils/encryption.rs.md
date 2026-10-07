# `br/pkg/utils/encryption.rs`

## 文件定位

本文件是 `astersql-br-pkg-utils` crate 中的备份内容解密辅助模块，由 [`br/pkg/utils/lib.rs`](lib.rs) 以 `pub mod encryption` 暴露。它位于 BR 公共工具层，不负责读取对象存储、解析备份元数据或管理主密钥，只把调用方已经取得的内容、`brpb::CipherInfo` 和 IV 转换为“原样内容或 AES-CTR 解密结果”。crate 边界与依赖声明见 [`br/pkg/utils/Cargo.toml`](Cargo.toml)：协议类型来自本 crate 的 `kvproto` 桩模块，密码运算委托给 `astersql-util-encrypt`，错误类型分别来自 `astersql-br-pkg-errors` 和 `astersql-errors`。

当前 Rust 生产调用集中在 `br/pkg/metautil`：[`br/pkg/metautil/metafile.rs`](../metautil/metafile.rs) 解密完整 backup meta 和索引树子文件，[`br/pkg/metautil/statsfile.rs`](../metautil/statsfile.rs) 解密统计文件，[`br/pkg/metautil/debug.rs`](../metautil/debug.rs) 在调试导出时解密 meta/stats 内容。`br/pkg/metautil/Cargo.toml` 通过路径依赖引入本 crate，证明这些调用跨 crate 使用公开 API，而不是同文件内部辅助逻辑。

## 核心职责

- `Decrypt` 根据 `CipherInfo.CipherType` 决定是否解密：空内容、缺少 cipher 或 `Plaintext` 都直接透传；三种 AES-CTR 枚举均调用同一个底层 CTR 实现；未知或本模块不支持的方法返回参数错误。
- `IsEffectiveEncryptionMethod` 提供配置/载荷是否声明了“有效加密”的轻量判定，仅把 `Unknown` 和 `Plaintext` 视为无效。
- 本模块只做分派和错误适配。它不生成 IV、不派生或清理密钥、不校验内容摘要，也不判断密钥来源；这些职责分别留给调用方和 `astersql-util-encrypt`。

## 主要符号

- `pub fn Decrypt(content: Vec<u8>, cipher: Option<&brpb::CipherInfo>, iv: &[u8]) -> Result<Vec<u8>, SharedError>`：消费输入缓冲区，并返回明文缓冲区或共享错误。`content.is_empty()` 与 `cipher.is_none()` 是最早的短路条件。
- `EncryptionMethod::{Plaintext, Aes128Ctr, Aes192Ctr, Aes256Ctr}`：`Decrypt` 明确接受的协议枚举。其余枚举统一进入错误分支。
- `AESDecryptWithCTR(&content, cipher.get_cipher_key(), iv)`：实际密码运算入口。底层 [`pkg/util/encrypt/aes.rs`](../../../pkg/util/encrypt/aes.rs) 要求密钥长度为 16、24 或 32 字节且 IV 恰为 16 字节，并采用大端 128 位 CTR；CTR 加解密共用同一密钥流变换。
- `ErrInvalidArgument`、`Annotate`、`SharedError`：不支持 cipher 类型时，以 BR 参数错误为根因并追加 `cipher type invalid {method:?}`；底层 AES 错误则直接包装为 `SharedError`。
- `pub fn IsEffectiveEncryptionMethod(method: EncryptionMethod) -> bool`：纯枚举谓词；除了 `Unknown` 和 `Plaintext` 外均返回 `true`，因此“有效”不等于“本文件的 `Decrypt` 一定支持”。

文件内没有常量、结构体、trait、`impl` 或条件编译项，两个函数都是公开 API。

## 执行流程

`Decrypt` 的执行顺序如下：

1. 若 `content` 为空，立即返回原 `Vec<u8>`；此时即使 cipher、密钥或 IV 非法也不会被检查。
2. 若 `cipher` 为 `None`，立即返回原内容，表示未配置内容加密。
3. 读取 `cipher.get_cipher_type()`。`Plaintext` 返回原内容，不读取密钥或 IV。
4. `Aes128Ctr`、`Aes192Ctr`、`Aes256Ctr` 进入共同分支，把内容、`cipher.get_cipher_key()` 和调用方 IV 交给 `AESDecryptWithCTR`。成功时返回新明文缓冲区；密钥或 IV 长度非法时传播底层错误。
5. 其他方法以 `ErrInvalidArgument` 为根因构造带方法名上下文的错误。

在典型的 meta 恢复链上，`metafile::DecryptFullBackupMetaIfNeeded` 先用 `IsEffectiveEncryptionMethod` 跳过未加密载荷，再从载荷前 `CrypterIvLen` 字节拆出 IV，调用 `Decrypt`，随后由更上层执行哈希校验和 protobuf 解析。索引 meta、stats 和 debug 路径则从各自 protobuf 节点读取 IV，调用 `Decrypt` 后再校验明文 SHA-256；因此本模块返回成功只代表分派/密码运算成功，不代表明文可信或格式有效。

## 数据与状态

`Decrypt` 的主要数据是拥有所有权的 `Vec<u8>`、只读借用的 `CipherInfo` 和只读 IV 切片。透传分支复用并返回传入的 `Vec`；AES-CTR 分支的底层 `ctr` 先复制输入到新缓冲区，再原地施加密钥流，所以调用者收到独立结果。`CipherInfo` 提供方法枚举和原始密钥字节，但本文件不修改协议对象。

本模块没有静态可变数据、缓存、计数器或持久状态。重要不变量是：底层 AES 密钥只能为 16/24/32 字节，IV 必须为 16 字节；不过本层没有核对 `Aes128Ctr/Aes192Ctr/Aes256Ctr` 枚举与实际密钥长度是否一致，真正选择 AES-128/192/256 的是密钥长度。这与 Go 委托 `encrypt.AESDecryptWithCTR` 的结构一致。

## 依赖与调用关系

上游生产调用边（由源码导入和调用点核验）包括：

- `br/pkg/metautil/metafile.rs::DecryptFullBackupMetaIfNeeded -> encryption::Decrypt`，并在调用前使用 `IsEffectiveEncryptionMethod`；同文件的 `walkLeafMetaFileDyn` 也用 `Decrypt` 解密子 meta。
- `br/pkg/metautil/statsfile.rs::downloadOneStatsFile -> encryption::Decrypt`，仅对非内联 stats 内容执行对象存储读取和解密。
- `br/pkg/metautil/debug.rs::{DecodeStatsFile, DecodeMetaFile} -> encryption::Decrypt`，用于把解密并校验后的内容转换为调试 JSON。

下游调用边是 `Decrypt -> astersql_util_encrypt::AESDecryptWithCTR`；错误分支调用 `SharedError::new` 和 `Annotate`。`IsEffectiveEncryptionMethod` 不调用其他业务函数。

RustCodeGraph 的文件节点报告 `br/pkg/utils/encryption.rs` 被 `br/pkg/utils/parity_test.rs` 使用；对两个精确符号执行 `callers/callees` 时未输出边，因此上述生产关系又用 `rg` 检查 import 和调用点，并用 `br/pkg/metautil/Cargo.toml` 验证跨 crate 依赖。仓库中 `br/pkg/encryption/manager.rs` 另有同名判定函数和自己的解密流程，不应误认为本文件的直接调用者。

## 错误处理与边界

- 空内容优先成功返回；它会掩盖无效 cipher、密钥或 IV，符合 Go 文件的短路顺序。
- `None` cipher 和 `Plaintext` 都透传内容；后者即使携带错误长度密钥/IV也不会报错。
- AES-CTR 分支由底层拒绝非 16/24/32 字节密钥和非 16 字节 IV，错误分别类似 `crypto/aes: invalid key size N` 与 `invalid IV length`，本层包装为 `SharedError` 而不追加额外上下文。
- `Unknown` 以及所有非 Plaintext、非 AES-CTR 方法都返回以 `ErrInvalidArgument` 为根因的错误。注意 `IsEffectiveEncryptionMethod` 对这些“其他方法”可能返回 `true`，调用方不能把该谓词当作 `Decrypt` 支持列表。
- CTR 没有填充或认证标签，错误密钥通常会产生错误明文而不是密码学错误。完整性由上层保存的 SHA-256 校验承担；单独调用 `Decrypt` 的代码必须自行验证结果。
- Go 的 `Decrypt` 在无效 cipher 类型时语义上返回原 `content` 与非空错误；Rust 的 `Result` 错误分支不携带内容。正常调用方只在 `Ok` 时消费明文，因此控制流一致，但需要保留这项签名差异。

## 并发与资源生命周期

两个函数均为无共享状态的同步纯计算入口，没有锁、通道、异步任务、文件句柄或网络资源。只读借用的 cipher/IV 仅在调用期间有效，返回值拥有自己的缓冲区；错误也不保留对输入的借用。因此同一 cipher 可由多个线程并发调用，线程协调由上游负责。

在 `metafile.rs` 的索引遍历中，上游会在 scoped worker 线程内并行调用 `Decrypt`，随后汇总结果；本文件既不创建线程，也不处理取消。CPU 和内存开销主要为 AES-CTR 对内容的线性扫描以及加密分支的一次内容复制，峰值内存随单次输入大小线性增长。密钥字节由 `CipherInfo` 所有，本文件不主动清零；如需敏感内存擦除，应在协议/密钥所有权层统一设计，不能只改此借用函数。

## 与 Go 版本的对应关系

直接对照文件是 [`br/pkg/utils/encryption.go`](encryption.go)。Rust 保留了 Go 的分支顺序和支持集合：空内容或 nil/None cipher 透传，Plaintext 透传，AES-128/192/256-CTR 委托公共 AES 工具，其他类型返回 `ErrInvalidArgument`；有效加密判定同样只排除 UNKNOWN 和 PLAINTEXT。

主要语言适配有三点：Go 的 `[]byte`/`*CipherInfo` 映射为拥有所有权的 `Vec<u8>`/`Option<&CipherInfo>`；Go 的 `(content, error)` 映射为 `Result<Vec<u8>, SharedError>`；Go `errors.Annotatef` 映射为 `Annotate` 加格式化后的枚举调试名。协议枚举拼写由 Go 的 `EncryptionMethod_AES128_CTR` 变为 Rust 的 `EncryptionMethod::Aes128Ctr`，不改变 wire 值意图。

独立 Rust 测试 [`br/pkg/utils/parity_test.rs`](parity_test.rs) 验证无 cipher 透传、Plaintext 非有效加密、Unknown 解密失败。底层 [`pkg/util/encrypt/aes_test.rs`](../../../pkg/util/encrypt/aes_test.rs) 的 `test_aes_decrypt_with_ctr` 用 Go 对齐向量覆盖成功解密和非法密钥长度。仓库未发现 `br/pkg/utils/encryption_test.go`；Go 侧行为证据来自同路径实现以及通过该工具函数的 `br/pkg/metautil/metafile_test.go`。

## 扩展指南

- 新增一种内容加密方法时，应先明确它是否属于本工具的对称内容解密范围，再同步修改 `Decrypt` 的 match 支持集和 `IsEffectiveEncryptionMethod` 的策略；不要只改谓词，否则会出现“判定有效但无法解密”。
- 若增加认证加密模式，应在成功返回前完成标签验证，并明确 IV/nonce 和认证数据的载荷布局；不能沿用 CTR 依赖上层 SHA-256 的假设。
- 若收紧 AES 枚举与密钥长度的一致性，必须先对照 Go 行为和历史备份兼容性，因为当前两端都由实际密钥长度选择 AES 变体。
- 回归测试应继续放在独立测试文件中。优先扩展 `br/pkg/utils/parity_test.rs`，至少覆盖空内容、None、Plaintext、三种 CTR、非法 key/IV、Unknown 和其他未支持枚举；底层算法向量应同步放在 `pkg/util/encrypt/aes_test.rs`。涉及完整 backup meta 布局或摘要校验时，应扩展 `br/pkg/metautil/metafile_test.rs`，而不是把测试内嵌进本源文件。
- 修改错误包装时需保留 `ErrInvalidArgument` 根因和足以诊断 cipher 类型的上下文，避免破坏上层错误分类；修改所有权签名时需评估大文件额外复制和 `br/pkg/metautil` 三个调用方。

## 验证依据

- RustCodeGraph：`status` 确认索引可用；`node --file br/pkg/utils/encryption.rs --offset 1 --limit 400` 返回完整 57 行源码并报告使用文件；`query Decrypt --kind function --json` 与 `query IsEffectiveEncryptionMethod --kind function --json` 精确定位两个符号；对精确符号运行 `callers/callees` 未得到边输出，故未把图缺失当成“无生产调用”。
- Rust 源与 crate：`br/pkg/utils/encryption.rs`、`br/pkg/utils/lib.rs`、`br/pkg/utils/Cargo.toml`、`pkg/util/encrypt/aes.rs`、`br/pkg/metautil/{metafile.rs,statsfile.rs,debug.rs,Cargo.toml}`。
- Go 对照：`br/pkg/utils/encryption.go`；集成行为参考 `br/pkg/metautil/metafile_test.go`。
- Rust 独立测试：`br/pkg/utils/parity_test.rs`、`pkg/util/encrypt/aes_test.rs`，并抽查 `br/pkg/metautil/metafile_test.rs` 的实际解密调用。
- 结构检查按任务命令验证本文恰好包含规定的 11 个二级章节。本任务只新增文档，依计划不运行 Cargo；人工复核重点为支持枚举、短路顺序、AES 长度边界、完整性责任和实际调用点。
