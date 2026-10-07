# `pkg/expression/builtin_encryption.rs`

## 文件定位

本文件属于 `astersql-expression` crate；`pkg/expression/Cargo.toml` 以 `lib.rs` 为库入口，`pkg/expression/lib.rs:194-195` 再用 `#[path = "builtin_encryption.rs"] mod builtin_encryption_kernel;` 将它编入 crate。它是加密、摘要、MySQL 历史密码/编码、压缩和密码强度算法的标量运行时内核，不负责 SQL 函数注册、参数表达式求值、类型推导或会话变量读取。

当前生产接线必须与算法覆盖范围分开理解：RustCodeGraph 将本文件列为被 `builtin.rs`、两份独立测试、`builtin_encryption_vec.rs` 和其测试/可选属性测试等文件使用；直接源码复核显示，`pkg/expression/builtin.rs:4491-4500` 的 `CoreBuiltinKind::Md5` 会调用这里的 `md5_hash`。其余公开函数目前主要通过 `pkg/expression/lib.rs:749-751` 的 `#[cfg(test)]` 门面暴露给独立测试。`pkg/expression/builtin_encryption_vec.rs` 是一套独立向量化实现，并不以本文件函数作为统一内核，因此不能据本文件的函数齐全程度推断所有 SQL 标量路径已经接线。

## 核心职责

- 用 `aes_mode`、`checked_iv`、`aes_encrypt` 和 `aes_decrypt` 实现 MySQL/TiDB AES 模式解析、密钥派生、IV 规则、加解密及失败转 SQL `NULL` 的语义（`builtin_encryption.rs:121-209`）。支持 128/192/256 位密钥下的 ECB、CBC、OFB、CFB；不支持的模式是语句错误。
- 用 `sql_decode`/`sql_encode`、`mysql_password`、`random_bytes` 和各摘要函数承载 MySQL 兼容算法及其可观察结果（`builtin_encryption.rs:211-268`）。
- 用 `compress`、`uncompress`、`uncompressed_length` 实现 MySQL 压缩载荷协议：四字节小端原长、zlib 数据以及必要时的 `.` 后缀，并将损坏数据分类为警告（`builtin_encryption.rs:544-623`）。为保持 Go `compress/flate` 的精确输出字节，文件还解析 DEFLATE 块并重写结束布局（`builtin_encryption.rs:270-542`）。
- 用 `validate_password_strength` 对预先解析好的 `PasswordPolicy` 评分，返回 `0/25/50/75/100`；本文件不读取全局变量或当前用户（`builtin_encryption.rs:625-716`）。
- 通过 `Eval<T>` 同时返回 SQL 值/`NULL` 和可追加到语句上下文的警告；通过 `EncryptionError` 区分必须中止求值的参数、随机源和 zlib 内部错误（`builtin_encryption.rs:50-101`）。

## 主要符号

| 符号 | 可见性与语义 |
| --- | --- |
| `AES_BLOCK_SIZE`, `SHA0`, `SHA224`, `SHA256`, `SHA384`, `SHA512` | 公共兼容常量；分别约束 IV 截取长度及 SHA2 的合法长度参数。 |
| `EncryptionWarning` | 公共警告枚举：`IvIgnored`、`ZlibData`、`ZlibBuffer`、`PasswordDeprecated`。 |
| `Eval<T>` | 公共结果容器；`value: None` 表示 SQL `NULL`，`warnings` 保留非致命诊断。内部构造器 `some`、`null_with` 统一常见返回。 |
| `EncryptionError` | 公共硬错误：不支持的 AES 模式、缺少参数、IV 过短、随机长度/随机源失败、zlib 内部失败。 |
| `aes_encrypt`, `aes_decrypt` | 公共 AES 入口；先解析模式和 IV，再调用 `astersql-util-encrypt` 的 MySQL 密钥派生与具体模式函数。 |
| `sql_decode`, `sql_encode` | 公共 MySQL SQL crypt 入口；名称沿用 Go 层历史方向，分别调用 `SQLDecode` 与 `SQLEncode`。 |
| `mysql_password`, `random_bytes` | 公共 PASSWORD/RANDOM_BYTES 算法入口；前者返回弃用警告，后者强制长度 `1..=1024` 并使用 `OsRng`。 |
| `md5_hash`, `sha1_hash`, `sha2_hash`, `sm3_hash` | 公共摘要入口，输出小写十六进制；SHA2 非法长度返回 `None`，0 等同 256。 |
| `DeflateBits`, `DeflateHuffman` 及 DEFLATE 辅助函数 | 私有、无输出解压的结构扫描器，只为定位最终块并生成与 Go 一致的 zlib 尾部。 |
| `compress`, `uncompress`, `uncompressed_length` | 公共 MySQL 压缩协议入口；`uncompress` 通过 `inflate_bounded` 将输出限制在声明长度内。 |
| `PasswordPolicy`, `validate_password_strength` | 公共、纯数据策略与评分入口；`Default` 与 Go 默认值对齐，但默认 `enabled = false`。 |

文件没有 trait、异步函数或条件编译项。内部类型 `AesKind`、`AesMode`、`InflateError` 和 DEFLATE 类型不构成 crate 外 API；模块本身也是私有模块，测试仅经 `#[cfg(test)]` 门面重导出。

## 执行流程

1. AES：`aes_encrypt`/`aes_decrypt` 调用 `aes_mode`，模式名先转 ASCII 小写；`checked_iv` 对 ECB 忽略已提供 IV 并产生 `IvIgnored`，对其他模式要求 IV 存在且至少 16 字节，只取前 16 字节。随后 `DeriveKeyMySQL` 将任意用户密钥派生到模式长度，再分派到 ECB/CBC/OFB/CFB。底层密码库失败被 `.ok()` 转成 `Eval { value: None, ... }`，而模式或 IV 错误仍作为 `Err` 返回。
2. 简单算法：摘要函数直接计算并十六进制编码；`sha2_hash` 按长度选择实现；非空 `mysql_password` 调用 `EncodePassword` 并带弃用警告；`random_bytes` 校验范围后让操作系统随机源填满新缓冲区；SQL crypt 直接委托 `astersql-util-encrypt::crypt`。
3. 压缩：`compress` 对空输入直接返回空；否则以默认 zlib 压缩。`normalize_go_zlib_end` 跳过 zlib 头/校验尾，`final_deflate_block` 遍历 stored/fixed/dynamic DEFLATE 块，清除原最终块的 `BFINAL`，追加最终空 stored 块和原 Adler-32 尾，从而匹配 Go 字节布局。最后在前面写入 `u32` 小端原长；zlib 流若以空格结尾则追加 `.`。
4. 解压：`uncompress` 对空输入返回空，对不超过四字节的非空载荷返回 `NULL + ZlibData`。它读取声明长度后调用 `inflate_bounded`，每次最多读 8 KiB；解码错误映射为 `ZlibData`，实际输出一旦超过声明长度映射为 `ZlibBuffer`。`uncompressed_length` 不验证或解压主体，只读取长度头；短载荷返回 `0 + ZlibData`。
5. 密码评分：`validate_password_strength` 先传播 `None`，再按顺序检查最短四字符/策略开关、用户名或其字节反转命中、策略最短长度、Unicode 类别计数、大小写无关字典包含，分别在首个失败点返回 0、25、50 或 75，全部通过才返回 100。

## 数据与状态

本文件没有全局可变状态。AES 模式和警告是按调用栈创建的小值；摘要、加密和压缩结果由调用者拥有。`Eval<T>` 把值与警告并列保存，防止用错误通道表示 MySQL 的非致命诊断。调用上层需要把 `EncryptionWarning` 映射/追加到真实 SQL statement context，本文件不会自行写会话状态。

`PasswordPolicy` 是评分时刻的不可变快照，包含开关、当前/认证用户名、长度与字符数阈值和字典。Go 版本从当前用户和全局系统变量动态读取这些数据（`builtin_encryption.go:1084-1151`）；Rust 算法入口要求适配层先构造快照。用户名检测使用原始 UTF-8 字节窗口及“字节反转”，而长度与字符类别使用 Unicode `char`；这是扩展或修正 Unicode 语义时必须留意的边界。

压缩路径的主要资源是内存缓冲。`compress` 会同时持有 zlib 输出和最终输出；`inflate_bounded` 的初始容量最多 8 KiB，并且永不接受超过声明长度的输出。声明长度来自不可信的四字节头，但不会直接按该长度分配全部内存。`uncompressed_length` 只暴露头中数值，不保证载荷真实可解压。

## 依赖与调用关系

- crate 边界：`pkg/expression/Cargo.toml` 声明 `encrypt`（本地 `astersql-util-encrypt`）、`flate2`、`md-5`、`parser-auth`、`rand`、`sha1`、`sha2`、`thiserror`、`unicode-general-category` 和 `hex`；本文件的每项外部算法调用都来自这些已声明依赖。
- 上游生产调用：已直接确认 `pkg/expression/builtin.rs:4491-4500` 的 `CoreBuiltinKind::Md5` 在完成参数求值和 SQL `NULL` 传播后调用 `builtin_encryption_kernel::md5_hash`。未发现其他生产 Rust 文件以限定路径直接调用本文件入口；不能把 Go 的全部 builtin class 当成 Rust 已接线事实。
- 模块与测试入口：`pkg/expression/lib.rs:194-195` 编入内核，`lib.rs:749-751` 仅在测试配置下将其重导出为 `expression_encryption::builtin_encryption::*`。`pkg/expression/builtin_encryption_test.rs` 和 `builtin_encryption_11_aster_unit_test.rs` 由此覆盖公共算法。
- 相邻实现：`pkg/expression/builtin_encryption_vec.rs` 自行定义 AES、摘要、压缩和密码评分逻辑；它与本文件共享目标语义而非复用实现。后续改变兼容行为时必须同步核对该文件及 `builtin_encryption_vec_test.rs`，否则标量算法测试与向量化路径可能分歧。
- 下游：AES 委托 `encrypt::aes::{DeriveKeyMySQL, AES*}`；历史编码委托 `encrypt::crypt::{SQLDecode, SQLEncode}`；PASSWORD/SM3 委托 `parser_auth`；压缩委托 `flate2` 并由本文件补充 DEFLATE 尾部规范化；随机字节依赖 `rand::rngs::OsRng`。

RustCodeGraph 的文件节点给出本文件被 5 个文件使用的概览；精确 `query` 定位到 `builtin_encryption.rs::aes_encrypt` 等符号。当前索引上的 `callers`/`callees` 命令在限定时间内未返回边，因此上述精确边均由模块入口、限定路径调用和测试导入的源码复核补足，未把超时视为“无调用者”。

## 错误处理与边界

- 硬错误与 SQL `NULL` 有意分离：不支持模式、CBC/OFB/CFB 缺 IV、IV 少于 16 字节、RANDOM_BYTES 越界/熵源失败、内部 zlib 构造错误返回 `EncryptionError`；密码库对具体数据处理失败则由 AES 公共入口转为 `value: None`。
- ECB 即使收到第三参数也不使用它，只返回 `IvIgnored`；需要 IV 的模式缺参数是 `IncorrectParameterCount`，而存在但过短是 `IvTooShort`。超过 16 字节的 IV 被截断。
- `sql_decode`/`sql_encode` 对下游结果使用 `unwrap()`。按当前依赖契约测试输入可成功，但如果 `SQLDecode`/`SQLEncode` 将来新增可达错误，这里会 panic，而不是返回 `EncryptionError`。
- PASSWORD 空串返回空串且没有弃用警告；非空才附加 `PasswordDeprecated`。SHA2 不认可 224/256/384/512/0 之外的长度，并用 `None` 表示 SQL `NULL`。
- COMPRESS 的输入长度被截为 `u32` 写入头部；正常 SQL 层应先受值长度上限约束，本函数签名本身没有拒绝超过 `u32::MAX` 的切片。解压接受 COMPRESS 可能追加的 `.`，因为 zlib decoder 在流结束后可忽略尾随字节。
- DEFLATE 扫描器验证截断、stored 块反码、Huffman 树/码、距离和重复长度溢出；错误包装为 `EncryptionError::Zlib`。`inflate_bounded` 同时防止畸形头诱发无界输出和“真实输出大于声明值”的兼容错误。
- 密码策略短路顺序是外部可观察的评分规则；字典只考虑字节长度在 4 到 100 的词。字符计数按 Unicode general category，除大写字母、小写字母和十进制数字外都计作 special。

## 并发与资源生命周期

所有函数均为同步函数，不创建线程、异步任务、锁、通道或事务，也不保存跨调用状态，因此算法对象天然可并发调用。`OsRng` 每次调用临时取得操作系统随机源；错误直接结束该次调用。zlib encoder/decoder、DEFLATE 游标、Huffman 表和输出缓冲都在函数返回时释放，没有需要调用者关闭的句柄。

这不等于完整 SQL builtin 已自动具备会话共享语义：Go `builtinCompressSig` 等类型明确要求新增字段线程安全或不可变，而本 Rust 文件根本不保存 expression 实例字段。未来把更多入口接入 Rust 表达式对象时，警告追加、会话策略快照和内存追踪必须在外层定义生命周期。本文件的 `uncompress` 只施加声明长度上限，没有像 Go `builtinUncompressSig.getMemTracker` 那样把分配计入 session statement memory tracker。

## 与 Go 版本的对应关系

主对照文件是 `pkg/expression/builtin_encryption.go`，其 SQL 层同时负责 function class 构建、参数/返回类型、PB code、会话变量、警告写入和实际算法。本 Rust 文件抽取了其中的算法结果与警告分类：

- `aes_encrypt`/`aes_decrypt` 对应 `builtinAesEncrypt{,IV}Sig.evalString` 和 `builtinAesDecrypt{,IV}Sig.evalString`：模式、MySQL 密钥派生、IV 截断、ECB 忽略警告及密码库失败转 `NULL` 一致；Rust 在一个入口中覆盖四种模式。
- `sql_decode`/`sql_encode`、`mysql_password`、摘要和随机字节分别对应 Go 的同名 builtin sig；`builtin_encryption_test.rs` 用固定向量验证 MD5/SHA/SM3、PASSWORD 和 SQL crypt 输出，并验证随机长度边界。
- `compress` 特意用 `normalize_go_zlib_end` 保持 Go `compress/flate` 的字节级结果，而不仅保证可往返解压。Rust 测试断言 `hello world` 的完整十六进制输出；Go 测试 `TestCompress`/`TestUncompress` 是来源对照。
- `uncompress` 与 Go 一样把损坏流映射为 zlib data 警告，把膨胀结果超过声明长度映射为 buffer 警告；`builtin_encryption_test.rs::scalar_uncompress_rejects_output_beyond_declared_length` 与 Go 的 `TestUncompressRejectsInflatedDataLargerThanDeclaredLength`、手工载荷用例对应。
- `validate_password_strength` 保留 Go 的 0/25/50/75/100 阶梯，但将 Go `CurrentUser`、session/global vars 和 `pwdValidator` 查询前移为 `PasswordPolicy`。因此算法结果可独立测试，但当前文件本身不证明 SQL 层动态配置接线完整。

Go 版本还有表达式共享约束、PB 下推码、可选属性、statement memory tracker 和更完整的 SQL 错误类型；这些属于适配层而非本算法文件。相反，Rust 当前只有 MD5 已直接进入 `builtin.rs` 的生产标量分派；其余函数是已实现、已由独立测试验证的迁移内核，不应描述为已全部替代 Go builtin。

## 扩展指南

1. 新增 AES 模式或密钥规格时，同时修改 `AesKind`、`aes_mode`、`aes_encrypt`、`aes_decrypt`；确认 `checked_iv` 的规则，并在 `pkg/expression/builtin_encryption_test.rs` 增加精确 Go 向量与失败路径。还必须同步检查独立实现 `builtin_encryption_vec.rs` 及其测试。
2. 改摘要/PASSWORD/SQL crypt 时优先保持输出编码、空串、非法长度和警告语义；新增依赖要在 `pkg/expression/Cargo.toml` 声明。若要暴露给 SQL，必须在真实 expression builder/分派处接线并验证 NULL、类型和 warning context，而不是只新增此文件函数。
3. 改压缩时保留四字节小端头、Go DEFLATE 结尾、尾空格保护和有界解压。任何 DEFLATE 解析调整都应覆盖 stored/fixed/dynamic 块、截断/非法树以及大输出；同步 `builtin_encryption_test.rs` 与 Go `builtin_encryption_test.go` 的相关向量。若接入 SQL 执行，还需补 statement memory tracker 行为。
4. 改密码评分时维持短路顺序，并明确用户名反转究竟按字节还是 Unicode 字符；同步普通与 Unicode 用例，以及向量实现的 `PasswordPolicy`/评分逻辑。策略数据应在调用边界构造，避免算法内隐式读取共享会话状态。
5. Rust 单元测试必须继续放在独立文件，不要嵌入本生产文件。最近的标量测试是 `pkg/expression/builtin_encryption_test.rs`，补充覆盖还可参考 `builtin_encryption_11_aster_unit_test.rs`；向量行为测试在 `builtin_encryption_vec_test.rs`。修改实现后按仓库规则运行 `cargo fmt --all`，但本次纯文档任务不运行 Cargo。

兼容风险集中在可观察密文/摘要/压缩字节、SQL `NULL` 与硬错误的分界、警告类别和顺序；性能风险集中在压缩复制、Huffman 线性查表、密码字典扫描及 Unicode 分类。引入缓存或共享状态前必须重新评估并发安全和租户/会话隔离。

## 验证依据

- 源码与模块：完整读取 `pkg/expression/builtin_encryption.rs:1-716`；复核 `pkg/expression/lib.rs:194-197, 749-751` 和 `pkg/expression/builtin.rs:4491-4500`。
- crate 声明：读取 `pkg/expression/Cargo.toml`，确认库入口、`autotests = false`、算法依赖和 Go 包迁移元数据。
- RustCodeGraph：`status` 显示当前项目索引包含 11,467 个文件；`node --file pkg/expression/builtin_encryption.rs --offset 1 --limit 1200` 返回完整文件及 5 个使用文件；`query aes_encrypt --kind function --json` 精确定位 `builtin_encryption.rs::aes_encrypt`。限定符号的 `callers`/`callees` 查询在 30 秒内未返回，调用边因此由限定路径与导入源码补证。
- Rust 测试：读取 `pkg/expression/builtin_encryption_test.rs`，确认 AES 全模式/密钥长度向量、IV/坏密文、摘要/PASSWORD/SQL crypt、随机长度、COMPRESS 精确字节、损坏/超声明长度解压及密码评分；抽查 `builtin_encryption_11_aster_unit_test.rs` 的重复 Go 向量。测试由 `lib.rs:477-481` 显式装配，因为 crate 关闭自动测试发现。
- Go 对照：读取/定位 `pkg/expression/builtin_encryption.go` 的 AES sig（约 143-380）、压缩/解压（约 876-1071）和密码评分（约 1084-1151）；定位 `pkg/expression/builtin_encryption_test.go` 的 `TestSQLDecode`、`TestSQLEncode`、`TestAESEncrypt`、`TestAESDecrypt`、`TestMD5Hash`、`TestRandomBytes`、`TestCompress`、`TestUncompress`、`TestUncompressLength`、`TestValidatePasswordStrength`、`TestPassword` 及膨胀长度防护用例。
- 本任务只新增说明文档，未运行 Cargo。交付结构检查要求目标文件存在且固定二级标题恰好 11 个；同时人工复核文档区分了当前生产接线、测试门面、独立向量实现和 Go 完整 SQL 适配层。
