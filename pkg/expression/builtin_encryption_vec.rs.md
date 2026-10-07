# `pkg/expression/builtin_encryption_vec.rs`

## 文件定位

本文件属于 `astersql-expression` crate（见 `pkg/expression/Cargo.toml`），实现一组以列为输入、按行求值的加密、摘要、压缩和密码强度 Rust 内核。`pkg/expression/lib.rs` 通过 `#[path = "builtin_encryption_vec.rs"] mod builtin_encryption_vec_kernel;` 编译该模块，但只在 `#[cfg(test)] mod expression_encryption_vec` 中重导出其公开项。仓库内对这些公开函数的直接引用也只出现在 `builtin_encryption_vec_test.rs` 和 `builtin_encryption_vec_10_aster_unit_test.rs`，因此当前事实是：它们是经过独立测试的移植内核，尚未接入 Rust 生产表达式的 `builtinFunc`/chunk 调度链。

对应的 Go 生产实现位于 `pkg/expression/builtin_encryption_vec.go`，其 `builtin*Sig.vecEvalString`/`vecEvalInt` 方法直接承接表达式参数列并写入 `chunk.Column`。Rust 文件不负责函数注册、表达式参数求值、返回类型推导或 protobuf 签名绑定；这些职责在 Go 侧由 `pkg/expression/builtin_encryption.go` 及表达式框架承担。

## 核心职责

- 用 `ByteColumn = Vec<Option<Vec<u8>>>` 和 `IntColumn = Vec<Option<i64>>` 表达带 SQL NULL 的批量输入输出，并保持输入行序。
- 实现 AES-128/192/256 的 ECB、CBC、OFB、CFB 加解密，包括 MySQL 异或折叠密钥派生、PKCS#7 填充、IV 规则以及行级密码失败转 NULL。
- 实现历史 SQL `ENCODE`/`DECODE` 置换算法、`RANDOM_BYTES`、MD5/SHA1/SHA2/SM3、旧式 `PASSWORD()`。
- 实现与 TiDB/MySQL 字节格式兼容的 `COMPRESS`、`UNCOMPRESS`、`UNCOMPRESSED_LENGTH`，并限制解压输出不超过头部声明长度。
- 根据 `PasswordPolicy` 为 `VALIDATE_PASSWORD_STRENGTH` 计算 0、25、50、75、100 五档得分。
- 通过 `EvalError` 区分会终止整批的结构/参数错误，通过 `Option::None` 表达 SQL NULL 或行级失败，通过 `EvalContext` 累积可继续执行的警告。

## 主要符号

- `ByteColumn`、`IntColumn`：本文件的简化列容器。`None` 是 SQL NULL，字节值不经过 UTF-8 转换，只有密码强度评分显式调用 `from_utf8`。
- `Warning` 与 `EvalContext`：记录 `IvIgnored`、`PasswordDeprecated`、`ZlibData`、`ZlibBuffer`；`warnings()` 只读暴露当前顺序，`clear_warnings()` 清空，内部 `warn()` 追加。
- `EvalError`：公开的整批错误，覆盖列长不一致、不支持的 AES 模式、缺失/过短 IV 和随机字节长度越界。
- `AesMode::parse`：只接受精确的 `aes-{128|192|256}-{ecb|cbc|ofb|cfb}`，保存密钥字节数和私有 `BlockMode`；`key_size()`、`name()`提供只读信息。
- `aes_encrypt_vec`、`aes_decrypt_vec`：AES 批量入口。它们调用 `check_len`、`derive_key_mysql`、`checked_iv` 以及各模式的加解密辅助函数。
- `SqlRand`、`SqlCrypt`、`crypt_columns`：复刻 MySQL 历史 SQL crypt 的种子、置换表和逐字节状态机；公开入口为 `sql_encode_vec`、`sql_decode_vec`。
- `random_bytes_vec`：对每个非 NULL 长度生成 1 到 1024 字节随机数据。
- `digest_vec`、`md5_vec`、`sha1_vec`、`sm3_vec`、`sha2_vec`：输出小写十六进制 ASCII 字节；SHA2 的 0 与 256 都选择 SHA-256，非法位数行返回 NULL。
- `password_vec`：空值和空字节串都输出空字节串；非空值做两轮 SHA1、加 `*` 并转大写十六进制，同时逐行追加弃用警告。
- `deflate`、`inflate`、`compress_vec`、`uncompress_vec`、`uncompressed_length_vec`：维护四字节小端原长头、zlib 载荷和错误警告语义。
- `PasswordPolicy`、`password_score`、`validate_password_strength_vec`：承载用户名、长度、字符类别和词典策略并完成评分。

## 执行流程

1. 调用者先构造等长列和需要的上下文/策略；多参数入口立即用 `check_len` 校验列长。AES 模式通常先经 `AesMode::parse` 解析。
2. 每个向量入口按索引或 `zip` 顺序遍历。必需参数任一为 NULL 时直接写入对应行的 NULL；`password_vec` 是例外，它按 Go 行为把 NULL 转为空串。
3. AES 入口将每行密钥经 `derive_key_mysql` 循环异或折叠到 16、24 或 32 字节。ECB 使用 PKCS#7 填充且忽略传入 IV并告警；CBC/OFB/CFB 要求 IV 列存在、等长，非 NULL IV 至少 16 字节且只取前 16 字节。底层密码错误被 `encrypted.ok()`/`decrypted.ok()` 转成该行 NULL。
4. SQL crypt 为每一行重新用口令构造 `SqlCrypt`，使伪随机状态不会跨行泄漏；随后原地编码或解码克隆出的字节。
5. 摘要函数逐行一次性计算哈希并十六进制编码；`sha2_vec` 同时匹配每行算法长度。`random_bytes_vec` 复用一次 `thread_rng`，但每行单独分配目标缓冲区。
6. `compress_vec` 对非空值调用 `deflate`，前置原长的四字节小端值；若最终字节为空格则追加 `.`。`deflate` 特意改写 flate2 flush 块并追加 Adler-32，使输出字节与 Go zlib 布局一致。
7. `uncompress_vec` 先解析声明长度，再由 `inflate` 以 8 KiB 栈缓冲循环解压；实际输出一旦超过声明长度立即报 `InflateError::Buffer`，停滞或 zlib 错误报 `Data`。两者都转为警告和行 NULL。
8. 密码评分先检查密码是否包含认证用户名/用户名或其“字节逆序”，再验证 UTF-8、字符数、Unicode 字符类别计数及词典命中，依次返回 0、25、50、75 或 100；功能关闭时非 NULL 行固定为 0。

## 数据与状态

所有列都由调用者拥有，函数返回新列，不修改输入。`ByteColumn` 的每行字节缓冲通常会被复制或新分配：AES/摘要/压缩生成新缓冲，SQL crypt 克隆输入后原地变换。结果容量多数按输入行数预留，但未复用 Go chunk allocator 或字节池。

跨行可见的可变状态仅有调用者传入的 `EvalContext.warnings`：警告按遇到顺序累积，函数不会自动清空。AES 的密钥和密码对象、SQL crypt 的随机状态、摘要状态均为调用或行内局部状态。`random_bytes_vec` 在一次批处理中持有一个线程本地随机数生成器；密码强度策略仅借用读取。

压缩格式的不变量是：非空合法值由四字节小端原始长度和 zlib 数据组成，可选末尾 `.` 只用于避免压缩串以空格结尾。解压只把头部声明长度当作硬上限，不要求实际解压长度必须恰好等于声明值。

## 依赖与调用关系

RustCodeGraph 将 `aes_encrypt_vec` 定位在第 354 行，并确认其下游调用边包括 `check_len`、`derive_key_mysql`、`checked_iv`、`encrypt_ecb`、`encrypt_cbc`、`crypt_ofb`、`encrypt_cfb` 和 `EvalContext::warn`。文件级索引显示本模块由 `pkg/expression/lib.rs` 编译，并被加密相关实现/测试识别；直接引用复核则表明公开向量 API 当前只由两份 Rust 独立测试调用。

外部 crate 依赖均由 `pkg/expression/Cargo.toml` 声明：`aes`、`cbc`、`cfb-mode`、`ofb`、`cipher` 实现 AES；`md-5`、`sha1`、`sha2` 和本地 `parser-auth` 提供摘要；`flate2`、`adler2` 提供压缩；`rand` 提供随机字节；`unicode-general-category` 用于密码字符分类；`hex` 与 `thiserror` 分别负责编码和错误派生。

在完整 SQL 主链中，已接线的参照仍是 Go：函数类在 `builtin_encryption.go` 构造具体 `builtin*Sig`，执行器选择 `vectorized() == true` 后调用 `builtin_encryption_vec.go` 的 `vecEvalString`/`vecEvalInt`。Rust 内核要进入等价主链，还需要适配真实表达式参数列、语句上下文警告/系统变量和结果列，而不是仅从测试门面调用。

## 错误处理与边界

- `ColumnLengthMismatch`、`UnsupportedBlockMode`、`MissingIv`、`ShortIv`、`RandomBytesLength` 是语句级 `Result::Err`，可能在已处理若干行后返回；调用者不应使用部分结果。
- AES 数据/填充/密码初始化失败使用私有 `CryptoError`，公开入口将其降级为对应行 NULL。CBC/ECB 解密要求块长是 16 的倍数；非法 PKCS#7 填充也返回行 NULL。OFB/CFB 不要求块对齐。
- ECB 收到 IV 时每个实际执行的非 NULL 数据/密钥行追加一次 `IvIgnored`。由于 NULL 检查在警告前，NULL 行不会告警。
- `random_bytes_vec` 的合法闭区间是 1..=1024；任一非 NULL 行越界就终止整批。当前使用 `rand::thread_rng()`，与 Go 的 `crypto/rand.Reader` 并非同一种安全性接口，接线前需明确安全需求。
- 摘要保持 NULL；SHA2 非法长度或长度 NULL返回行 NULL。`password_vec` 则把 NULL 与空串都映射为空串，并只为非空输入告警。
- 压缩内部 I/O 错误在 `compress_vec` 中转为行 NULL且不告警。解压输入为空时返回空；长度不超过 4、zlib 损坏或解压停滞产生 `ZlibData`；输出超过声明长度产生 `ZlibBuffer`。读取四字节头前已有长度守卫，因此 `try_into().unwrap()` 的前提由分支保证。
- 密码评分对非法 UTF-8 返回 0。用户名检查在 UTF-8 检查前按原始字节窗口完成；空用户名被跳过，词典只考虑字节长度 4..=100 的词。

## 并发与资源生命周期

本文件没有锁、通道、异步任务、全局可变状态或后台资源；一次函数调用在当前线程顺序处理全部行，因此同一输入行序决定警告顺序。不同调用可以并行，但同一个 `&mut EvalContext` 受 Rust 借用规则约束，不能被并发共享写入。

密码器、哈希值、zlib 编解码器和 SQL crypt 状态都在调用或单行结束时释放。`inflate` 的固定 8 KiB 缓冲位于循环局部，累计输出由声明长度约束；初始容量最多预留 8 KiB，避免仅凭伪造的巨大声明长度立即进行同等规模分配。不过声明长度本身可达 `u32::MAX`，真实压缩流仍可能逐步产生很大但不超过声明值的输出；生产接线时还需结合查询级内存配额。Go 版本的 `UNCOMPRESS` 使用 memory tracker，`COMPRESS` 使用字节池，Rust 当前均没有等价资源记账或复用。

## 与 Go 版本的对应关系

`pkg/expression/builtin_encryption_vec.go` 是逐函数语义对照：AES、ENCODE/DECODE、随机字节、摘要、PASSWORD、压缩/解压和密码强度都有相应 `builtin*Sig.vecEval*`。Rust 测试固定了典型 Go/MySQL 向量，例如 AES-128-ECB 的密文、SQL crypt 的历史反向命名、摘要十六进制、COMPRESS 完整字节布局和各档密码评分。

已对齐的重要行为包括 NULL 传播、AES 的 MySQL 密钥派生、IV 截断和短 IV 错误、坏密文行转 NULL、SHA2 长度选择、PASSWORD 弃用警告、COMPRESS 小端长度头及尾随点、zlib 数据/缓冲警告。用户名反向检查刻意逆序 UTF-8 字节而不是 Unicode 标量；`builtin_encryption_vec_test.rs` 用多字节用户名验证了这一点。

仍有必须如实保留的结构差异：Go 入口直接操作 `chunk.Chunk`/`chunk.Column`，复用表达式缓冲区，并可识别常量密钥；Rust 每行重新派生密钥并返回自有 `Vec`。Go 从会话与全局变量取得密码校验开关、用户和策略，Rust 要求调用者显式传入 `enabled` 与 `PasswordPolicy`。Go 解压接入查询内存 tracker，Rust 只有声明长度硬上限。Go `RANDOM_BYTES` 使用 `crypto/rand.Reader`，Rust 使用 `thread_rng`。这些差异意味着 Rust 文件虽有行为测试，不能据此宣称已完成生产替换。

## 扩展指南

- 新增 AES 模式或密钥规格时，应同步修改 `BlockMode`、`AesMode::parse/name`、加解密分派及 IV 规则；在独立测试文件增加已知向量、NULL、短 IV、坏密文和三种密钥长度用例。
- 改动列接口时先设计真实表达式/chunk 适配层，保留“语句错误、行 NULL、警告”三种结果通道，不要把所有失败统一成 `Result::Err` 或静默 NULL。
- 新增摘要算法应优先复用 `digest_vec`，明确 SQL 接受的算法选择值、大小写和 NULL 语义，并在 `Cargo.toml` 统一声明依赖。
- 修改压缩实现必须维持 Go 兼容的逐字节输出和四字节小端头，并保留解压上限；同时评估 memory tracker、总输出配额和批内多行累计内存，而不仅是单行声明长度。
- 修改密码策略应同步核对 `builtinValidatePasswordStrengthSig.validateStr` 和密码校验系统变量，特别注意字节长度、Unicode 字符数、大小写转换和用户名字节逆序的区别。
- 测试必须继续放在独立的 `pkg/expression/builtin_encryption_vec_test.rs` 或 `builtin_encryption_vec_10_aster_unit_test.rs`，不要内嵌到生产源文件。生产接线完成后还应增加从表达式构建到向量执行的集成测试，证明真实上下文警告、系统变量和资源记账已连接。

## 验证依据

- 源码全量阅读：`pkg/expression/builtin_encryption_vec.rs`，包括公开类型/入口、私有密码与压缩辅助函数以及所有分支。
- crate 与模块边界：`pkg/expression/Cargo.toml`；`pkg/expression/lib.rs` 中 `builtin_encryption_vec_kernel` 的模块声明及仅测试可见的 `expression_encryption_vec` 重导出。
- RustCodeGraph：`status` 显示目标仓库索引包含该文件；`files --filter pkg/expression/builtin_encryption_vec.rs` 确认文件已索引；`node --file ... --offset 1/501` 读取 914 行源码和文件使用关系；`query aes_encrypt_vec`、`query compress_vec`、`query validate_password_strength_vec` 定位入口；`callees aes_encrypt_vec` 核对其长度、密钥、IV、模式和警告下游边。自然语言 `explore` 与部分 `callers` 查询未返回可用文本，因此又以 `rg` 直接引用补证，未据此推断不存在未索引调用。
- Go 对照：`pkg/expression/builtin_encryption_vec.go` 的全部向量签名及关键实现，`pkg/expression/builtin_encryption.go` 的函数类、标量行为、压缩内存跟踪和密码策略入口，`pkg/expression/builtin_encryption_vec_test.go` 的向量框架覆盖。
- Rust 独立测试：`pkg/expression/builtin_encryption_vec_test.rs` 与 `pkg/expression/builtin_encryption_vec_10_aster_unit_test.rs`，覆盖 AES 四模式、NULL/警告/错误、摘要、随机长度、SQL crypt、压缩格式与防膨胀、PASSWORD 和密码评分。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前使用任务指定命令验证本文恰有十一个固定二级章节，并人工复核没有把测试门面描述为生产接线。
