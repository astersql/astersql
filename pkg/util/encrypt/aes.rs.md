# `pkg/util/encrypt/aes.rs`

## 文件定位

`aes.rs` 是 `astersql-util-encrypt` crate 的 AES 基础实现，位于 SQL 表达式层与 RustCrypto 算法 crate 之间。crate 入口 `pkg/util/encrypt/lib.rs` 以 `pub mod aes` 声明此模块，并以 `pub use aes::*` 再导出它的公开 API。`pkg/util/encrypt/Cargo.toml` 将该 crate 映射到 Go 包 `pkg/util/encrypt`，并声明 `aes`、`cipher`、`cbc`、`cfb-mode`、`ctr` 和 `ofb` 等密码依赖。

应用主链中，`pkg/expression/builtin_encryption.rs::aes_encrypt` 和 `aes_decrypt` 先解析 SQL 的 `block_encryption_mode`、校验 IV，再用本文件的 `DeriveKeyMySQL` 派生密钥，最后调用 ECB/CBC/OFB/CFB 入口。该上层会把底层加解密错误转换为 SQL `NULL`。CTR API 是本 crate 的通用公开能力，RustCodeGraph 在当前生产路径中未找到标量 SQL 调用者，但有独立测试向量验证。向量化 SQL 路径 `pkg/expression/builtin_encryption_vec.rs` 自带分组模式实现；不应把它概括为全部调用本文件。

## 核心职责

- 定义统一的 `EncryptError`，把无效密钥、IV、密文块长和 PKCS#7 填充错误暴露给上层。
- 实现 `PKCS7Pad`/`PKCS7Unpad`，为 ECB 和 CBC 块模式提供补位与严格解补位。
- 为 16/24/32 字节密钥分派 `Aes128`/`Aes192`/`Aes256`，提供 ECB、CBC、OFB、CTR 和 CFB 的成对加解密 API。
- 实现 `DeriveKeyMySQL`，以循环异或口令字节的方式模拟 MySQL AES 密钥派生语义。
- 保持与 `pkg/util/encrypt/aes.go` 的密文向量、错误边界和公开函数名兼容，供 Rust 迁移路径使用。

## 主要符号

- `pub struct EncryptError(String)`：轻量错误容器，实现 `Display` 和 `std::error::Error`；其字符串字段与构造函数 `EncryptError::new` 都不公开，外部只能接收和展示错误。
- `check_key(key)`：内部不变式守门，只接受 16、24、32 字节；它在分派具体 AES 类型前运行。
- `check_iv(iv)`：要求 IV 恰为 AES 块大小 16 字节，被 CBC/OFB/CTR/CFB 路径共享；ECB 无 IV。
- `PKCS7Pad(data, blockSize)` 与 `PKCS7Unpad(data, blockSize)`：公开的通用填充 API。即使输入已对齐，`PKCS7Pad` 仍增加一个完整填充块；`PKCS7Unpad` 同时检查整体对齐、末字节表示的填充长度和其余填充字节。
- `ecb_crypt(data, key, encrypt)`：内部 ECB 块循环；不自行填充，要求输入长度是 16 的倍数，根据 `encrypt` 选择 `encrypt_block` 或 `decrypt_block`。
- `AESEncryptWithECB`/`AESDecryptWithECB`：在 `ecb_crypt` 外分别组合 PKCS#7 填充和去填充。
- `AESEncryptWithCBC`/`AESDecryptWithCBC`：使用 `cbc::Encryptor`/`Decryptor` 和 RustCrypto `Pkcs7`；解密前额外拒绝非 16 字节整倍的密文。
- `ofb` 与 `ctr`：内部密钥流变换，在输入副本上 `apply_keystream`；各自的公开加密/解密函数共用同一实现。CTR 明确使用 `Ctr128BE`。
- `AESEncryptWithCFB`/`AESDecryptWithCFB`：分别使用 `cfb_mode::Encryptor` 和 `Decryptor`，因 CFB 方向不同而不共用单一函数。
- `DeriveKeyMySQL(key, blockSize)`：创建 `blockSize` 个零字节，以 `index % blockSize` 为位置累积异或；`blockSize == 0` 时直接返回空向量，避免取模除零。

## 执行流程

1. SQL 标量调用者根据会话模式确定 AES 种类和目标密钥长度，对非 ECB 模式检查 IV，再调用 `DeriveKeyMySQL`。
2. 公开模式函数进入本文件后，先由 `check_key` 选定 AES-128/192/256；需要 IV 的模式继续由 `check_iv` 限定为 16 字节。
3. ECB 加密先执行 `PKCS7Pad(..., 16)`，再以 16 字节分块原地加密；ECB 解密反向执行分块解密和 `PKCS7Unpad`。
4. CBC 加密由 RustCrypto 填充并加密；CBC 解密先检查块对齐，再由 RustCrypto 解密和校验 PKCS#7。
5. OFB 和 CTR 生成密钥流并与输入异或，因此加密和解密是同构操作，输出长度与输入相同。CFB 也不填充，但使用分离的加密器和解密器。
6. 所有路径都返回新的 `Vec<u8>`；底层失败以 `EncryptError` 传回。SQL 标量封装层再决定是否将其降格为 `NULL`。

## 数据与状态

本文件不保存全局状态、密钥缓存或随机数生成器。所有 API 仅借用输入切片，并在局部 `Vec<u8>` 上完成操作，不修改调用者的明文、密文、密钥或 IV 缓冲区。`EncryptError` 仅持有错误消息字符串。

块模式的关键状态是 16 字节 AES 块边界。ECB/CBC 加密使用 PKCS#7，所以空输入或已对齐输入也会得到额外的 16 字节密文块；OFB/CTR/CFB 不填充，保持长度。`DeriveKeyMySQL` 的累积器是指定长度的零向量，超过该长度的口令字节折返到前面的位置异或。

## 依赖与调用关系

上游与内部调用边（RustCodeGraph）：

- `pkg/expression/builtin_encryption.rs::aes_encrypt` 调用 `DeriveKeyMySQL`、`AESEncryptWithECB/CBC/OFB/CFB`；`aes_decrypt` 调用对应解密函数。当前 SQL 模式分支没有 CTR。
- `pkg/util/encrypt/aes_test.rs` 的 `assert_encrypt_cases`、`assert_decrypt_cases`、`test_pad`、`test_unpad` 和 `test_derive_key_mysql` 直接覆盖公开 API；`migration_aster_unit_test.rs` 另有 Go 向量和错误字符串回归。
- 内部调用边为 `AESEncryptWithECB -> PKCS7Pad -> ecb_crypt`、`AESDecryptWithECB -> ecb_crypt -> PKCS7Unpad`，OFB/CTR 的成对公开 API 分别汇入 `ofb`/`ctr`。`check_key` 被所有模式路径使用，`check_iv` 被 CBC/OFB/CTR/CFB 使用。

下游依赖来自 `pkg/util/encrypt/Cargo.toml`：`aes` 提供三种密钥宽度的块密码，`cipher` 提供 trait、`GenericArray`和 `Pkcs7`，`cbc`/`cfb-mode`/`ctr`/`ofb` 提供对应工作模式。该 Cargo 文件没有为 AES 逻辑声明条件 feature，因此本文件没有条件编译分支。`rand` 属于同 crate 其他模块的依赖，本文件未使用它。

RustCodeGraph 的文件级索引还列出 `pkg/expression/builtin_encryption_vec.rs` 和 `pkg/util/codec/codec.rs` 为使用相关文件；精确符号检查显示，向量化表达式层自行实现 AES 模式，而 `codec.rs::shared_error` 仅有通用 `Display` 错误转换，未找到对本文件公开 AES 函数的精确调用边；因此本文档不将它们宣称为直接 API 调用者。

## 错误处理与边界

- 密钥长度非 16/24/32 时，`check_key` 返回 `crypto/aes: invalid key size N`，不进入底层构造器。
- IV 不是 16 字节时，需要 IV 的所有模式返回 `invalid IV length`。ECB 完全忽略 IV 概念。
- `PKCS7Pad` 拒绝块大小 0 或大于 255，避免取模除零以及填充长度无法用单字节表示。
- `PKCS7Unpad` 拒绝块大小 0、空数据、非块对齐数据、填充长度 0、超过块大小的填充，以及任一受检查填充字节不一致的数据。长度类错误为 `Invalid padding size`，内容类错误为 `Invalid padding`。
- ECB/CBC 解密在密文长度不是 16 的倍数时返回 `Corrupted data`；块对齐但填充错误时返回填充错误。流模式允许任意长度，不会通过填充检测错误密钥、IV 或被篡改数据。
- 在 `check_key`/`check_iv` 成功后，各模式的 `new_from_slices(...).unwrap()` 依赖前置长度校验保证不会失败；新增模式或改变长度规则时必须继续维持该不变式。
- 这些模式提供的是机密性而非认证加密，本文件不生成或校验 MAC/tag；调用者不能依赖 OFB/CTR/CFB 或未认证的 ECB/CBC 检测密文篡改。

## 并发与资源生命周期

所有函数都是无共享可变状态的同步计算：每次调用创建局部 cipher/mode 对象和输出向量，在返回前完成全部加解密。没有锁、通道、异步任务、线程、文件句柄或事务，也没有需要显式关闭的资源。因此多线程可同时调用这些无状态 API，但每次调用都会复制数据并重新构造 cipher，大输入或高频调用时应评估分配与构造开销。

密钥、IV 和中间明文随局部对象和 `Vec<u8>` 的普通 Rust 生命周期释放；代码未使用内存锁定或显式归零，所以不应声称销毁后会清除敏感字节。

## 与 Go 版本的对应关系

Rust 公开 API 与 `pkg/util/encrypt/aes.go` 保持同名对应：`PKCS7Pad`/`Unpad`、`AESEncryptWith*`/`AESDecryptWith*` 以及 `DeriveKeyMySQL`。两版本都支持 AES-128/192/256，ECB/CBC 使用 PKCS#7，OFB/CTR 用同一密钥流函数完成双向变换，CFB 区分加密和解密方向，MySQL 密钥派生则逐字节循环异或。`pkg/util/encrypt/aes_test.rs` 保留了 `aes_test.go` 的表驱动密文向量、非法密钥和填充分支；`migration_aster_unit_test.rs` 另以固定 Go 密文和错误文本做迁移回归。

实现差异主要是安全边界的表达方式：Go 的 ECB `CryptBlocks` 在非整块或目标缓冲不足时 `panic`，Rust 的内部 `ecb_crypt` 直接返回 `EncryptError`，并在内部自行分配输出。Go 的 `PKCS7Pad` 使用 `append`，注释提醒容量充足时可改写底层内存；Rust 总是复制到新 `Vec`。Rust 额外拒绝 `PKCS7Pad` 的 0 或大于 255 块大小，并让 `DeriveKeyMySQL(..., 0)` 返回空向量；Go 实现没有这两个显式防护，因此扩展或改动时要区分“密文兼容”与“无效参数的宿主语言行为”。

## 扩展指南

- 新增工作模式时，先明确它是块模式还是流式变换，是否需要填充、IV/nonce 的确切长度以及是否提供认证。共享前置校验应接入 `check_key`/`check_iv` 或新的明确守门，不应仅依赖 `unwrap()`。
- 新增 AES 密钥宽度或更换依赖时，需同步修改 `check_key`、所有 `match key.len()` 分支、`pkg/util/encrypt/Cargo.toml` 和独立测试；不完整修改会使前置校验与 `unreachable!()`/`unwrap()` 不变式失配。
- 改动填充或模式行为时，必须与 `pkg/util/encrypt/aes.go` 的密文语义核对，并在独立文件 `pkg/util/encrypt/aes_test.rs` 中添加正常向量、边界长度和错误向量；不要把 Rust 测试内嵌到 `aes.rs`。必要时同步 `migration_aster_unit_test.rs` 的 Go 兼容回归。
- 若要向 SQL 暴露新模式，仅修改本文件不足；还需在 `pkg/expression/builtin_encryption.rs` 的模式解析、IV 规则和加解密分派中接线，并评估 `pkg/expression/builtin_encryption_vec.rs` 的独立向量化路径是否需要对齐。
- 保持无认证模式的兼容性时，不要将解密成功解释为完整性证明。如果新增 AEAD，必须单独设计 nonce/tag 输入输出和错误合约，避免破坏现有密文向量。
- 性能优化应重点衡量输入复制、输出分配和每次构造 cipher 的成本；如果引入缓存或可复用状态，必须重新审查线程安全、密钥生命周期和跨调用状态泄漏风险。

## 验证依据

- 源码与 crate 边界：`pkg/util/encrypt/aes.rs`、`pkg/util/encrypt/lib.rs`、`pkg/util/encrypt/Cargo.toml`。源文件已通过 RustCodeGraph `node --file` 完整读取，确认 295 行内的全部公开函数、内部辅助函数、宏分派与无条件编译项。
- RustCodeGraph 状态：项目索引可用，包含 `pkg/util/encrypt/aes.rs`。精确查询确认 `check_key` 的 7 个内部调用者、`check_iv` 的 6 个内部调用者，以及 ECB/CBC/OFB/CFB 公开 API 到 `pkg/expression/builtin_encryption.rs::aes_encrypt`/`aes_decrypt` 的上游调用边。CTR 公开 API 在当前索引中只见独立测试调用边。
- 上层精确源码：RustCodeGraph `node aes_encrypt --file pkg/expression/builtin_encryption.rs` 和对应 `aes_decrypt` 节点，确认 MySQL 密钥派生、模式分派和错误转 SQL `NULL` 的逻辑；`node crypt_ofb --file pkg/expression/builtin_encryption_vec.rs` 证实向量化路径有独立实现。
- Go 对照：`pkg/util/encrypt/aes.go` 的 ECB 类型、通用填充、五种模式和 `DeriveKeyMySQL`；该文件已完整读取并与 Rust 分支逐项核对。
- 测试证据：`pkg/util/encrypt/aes_test.rs` 覆盖 PKCS#7 正反例、AES-128/192/256 ECB 基础向量、五种模式的固定密文与非法密钥/填充，以及 MySQL 密钥派生；`pkg/util/encrypt/aes_test.go` 是其 Go 意图来源；`pkg/util/encrypt/migration_aster_unit_test.rs` 覆盖迁移密文向量和错误文本。
- 本任务是纯文档分析，按计划不运行 Cargo。交付时以任务指定的 11 个固定二级标题结构检查、路径存在性检查和人工事实复核作为完成证据。
