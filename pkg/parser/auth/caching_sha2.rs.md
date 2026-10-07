# `pkg/parser/auth/caching_sha2.rs`

## 文件定位

[`caching_sha2.rs`](caching_sha2.rs) 属于 `astersql-parser-auth` crate；[`Cargo.toml`](Cargo.toml) 以同目录的 `lib.rs` 为 crate 根，并依赖 `rand`、`sha2` 以及认证插件常量所在的 `astersql-parser-mysql`。`lib.rs` 将它公开为 `caching_sha2`，同时在兼容迁移代码的 `parser::auth::caching_sha2` 路径下再导出。

该文件实现 `caching_sha2_password` 与 `tidb_sm3_password` 共用的“存储认证字符串”生成和校验，不负责 MySQL 握手、权限查找或会话认证。当前可确认的生产接线是 [`pkg/executor/utils.rs`](../../executor/utils.rs) 中 `encodedPassword`：创建或修改用户且输入为明文时，两个内置插件都会调用 `NewHashPassword`。仓库内对 `CheckHashingPassword` 的 Rust 引用目前只出现在独立测试中，因而不能据此声称 Rust 运行时认证校验已经接入。

## 核心职责

- 用 `Sha256Hash` 或相邻 [`tidb_sm3.rs`](tidb_sm3.rs) 的 `Sm3Hash` 提供固定 32 字节摘要，再由 `hashCrypt` 执行同一套 SHA-crypt 混合流程。
- 将结果编码成 `$A$<三位十六进制轮数单位>$<20 字节盐><43 字节 crypt-base64 摘要>`；默认生成路径使用 5,000 轮，因此轮数字段为 `005`，完整结果通常为 70 字节。
- 由 `CheckHashingPassword` 解析已有认证字符串、按插件名选择摘要算法并重算比对。
- 由 `NewHashPassword` 从操作系统随机源生成安全分段所需的盐，再产生新认证字符串。

这里的“密码哈希”是数据库中保存的认证字符串构造过程，不是客户端快速认证交换本身。算法的格式和步骤直接对照同目录 [`caching_sha2.go`](caching_sha2.go)。

## 主要符号

- `MIXCHARS: usize = 32`：每次混入或截取摘要的固定宽度，等于 SHA-256 和 SM3 的摘要长度。
- `SALT_LENGTH: usize = 20`：认证字符串盐长度；解析时也据此从最后一段取前 20 字节。
- `ITERATION_MULTIPLIER: usize = 1000`：字符串只存实际迭代次数除以 1,000 后的十六进制单位。
- `b64From24bit(b, n, buf)`：私有 crypt-base64 编码器。它把三个输入字节拼成 24 位整数，按低 6 位优先顺序写入 `./0-9A-Za-z` 字母表；不是标准 Base64。
- `Sha256Hash(input) -> Vec<u8>`：公开的 SHA-256 一次性摘要包装，返回 32 字节动态数组。
- `hashCrypt(plaintext, salt, iterations, hash) -> String`：私有核心算法。`hash` 是函数指针，隐含契约是每次必须返回至少 32 字节。
- `CheckHashingPassword(pwhash, password, hash_name) -> Result<bool, String>`：公开校验入口。格式错误返回 `Err`，格式有效但口令或插件不匹配返回 `Ok(false)`。
- `NewHashPassword(password, hash_name) -> String`：公开生成入口。支持 `AuthCachingSha2Password` 和 `AuthTiDBSM3Password`；未知插件返回空串。

文件没有类型、trait、`impl` 或条件编译项，状态均局限在函数栈和局部 `Vec<u8>` 中。

## 执行流程

`NewHashPassword` 的流程如下：

1. 用 `rand::rngs::OsRng` 填充 20 字节盐。
2. 每字节清除最高位，使其保持 7 位；若得到 `$` 或 NUL，则重新抽取该字节。这样盐不会引入额外分隔段，也不会产生 NUL 或多字节 UTF-8。
3. `caching_sha2_password` 选择 `Sha256Hash`，`tidb_sm3_password` 选择 `Sm3Hash`，两者都以 5,000 轮调用 `hashCrypt`；未知插件直接返回空串。
4. `hashCrypt` 依 SHA-crypt 步骤构造 A/B、DP、DS 缓冲区，得到与口令长度相同的 `p` 和与盐长度相同的 `s`。
5. 迭代阶段按轮号奇偶及能否被 3、7 整除选择 `p`、`s` 和上一轮摘要进行混合。
6. 输出 `$A$`、三位十六进制轮数单位、`$` 和盐，再按 MySQL 指定的摘要字节置换顺序调用 `b64From24bit`，产生 43 字节摘要文本。

`CheckHashingPassword` 先按 `$` 切分输入，要求恰好四段且摘要类型为 `A`；随后把轮数字段按十六进制解析为 `i64`，乘以 1,000，取最后一段前 20 字节为盐。它再按 `hash_name` 重走 `hashCrypt`，最后比较完整输入字节与新字符串字节是否完全相等。因此盐、轮数字段、摘要编码或口令任一不同都会导致 `Ok(false)`，而不是只比较摘要尾部。

## 数据与状态

认证字符串的四个 `$` 切分结果依次是空前缀、摘要类型 `A`、轮数单位、`salt+hash`。盐固定 20 字节；32 字节摘要经过十组 4 字符和一组 3 字符的自定义编码后为 43 字节。默认生成结果由 7 字节头部（`$A$005$`）、20 字节盐和 43 字节摘要组成。

`hashCrypt` 的主要临时数据是：初始摘要 `sum_a`/`sum_b`，口令派生序列 `p`，盐派生序列 `s`，以及每轮摘要 `sum_c`。这些值不离开函数。缓冲区按输入长度增长；尤其 `buf_dp` 写入口令长度份完整口令，空间和写入量对口令字节长度呈平方级增长。迭代阶段每轮重新创建 `buf_c` 并复制摘要材料，时间成本与解析或指定的 `iterations` 线性相关。

所有长度运算使用 UTF-8 字节长度：`plaintext.len()` 与 `plaintext.as_bytes()` 一致，这与 Go 的字符串字节语义对齐。`String::from_utf8_lossy` 用于最终转换；正常生成路径的头、受限盐和编码字母表均为 ASCII，因此不会发生替换。

## 依赖与调用关系

RustCodeGraph 对目标文件给出的内部边为：

- `CheckHashingPassword -> hashCrypt`
- `NewHashPassword -> hashCrypt`
- `hashCrypt -> b64From24bit`

算法依赖 `sha2::Sha256`、`sha2::Digest`、`rand::RngCore`、`rand::rngs::OsRng`，并通过 `super::tidb_sm3::Sm3Hash` 复用 SM3 实现。插件分派依赖 `astersql-parser-mysql` 暴露的 `AuthCachingSha2Password` 与 `AuthTiDBSM3Password` 常量。

RustCodeGraph 的跨文件 callers 查询没有返回结果，但其文件关系和精确文本搜索补充确认：[`pkg/executor/utils.rs`](../../executor/utils.rs) 的 `encodedPassword` 生产调用 `NewHashPassword`；[`caching_sha2_test.rs`](caching_sha2_test.rs)、[`tidb_sm3_test.rs`](tidb_sm3_test.rs) 与 [`migration_aster_unit_test.rs`](migration_aster_unit_test.rs) 同时覆盖生成/校验。该差异是索引调用边覆盖限制，不应把“无静态 caller”解释成函数未使用。

## 错误处理与边界

`CheckHashingPassword` 显式区分三类解析错误：分段数不是四段时返回 `failed to decode hash parts`；类型不是 `A` 时返回 `digest type is incompatible`；轮数字段不是 UTF-8 或不是合法十六进制时返回 `failed to decode iterations`。合法格式但密码错误返回 `Ok(false)`。未知插件不会报错，而是用空字符串参与最终比较，通常得到 `Ok(false)`。

以下是调用方必须遵守、但类型签名没有表达的前置条件：

- 最后一段必须至少有 20 字节；`&parts[3][..SALT_LENGTH]` 对更短输入会 panic。现有“短哈希”测试只覆盖无法切成四段的输入，没有覆盖“四段但盐过短”。
- 实际迭代次数必须大于零。零轮不会设置 `sum_c`，后续索引其 32 个摘要字节会 panic。极端十六进制值经 `wrapping_mul` 后若不能转成 `usize` 会退为零，也会触发这一风险。
- 摘要回调必须返回至少 32 字节，否则多处切片或索引会 panic；当前两个内置算法满足契约。
- 输入中多余的 `$` 会使分段数不等于四并返回错误。盐生成逻辑专门排除了 `$`，但校验外部字符串时仍依赖该格式约束。

`NewHashPassword` 保留 Go 形状而不返回随机源错误；`OsRng.fill_bytes` 的失败行为由 `rand` API 处理。未知插件返回空串，调用者必须避免把空串当作有效认证数据。

## 并发与资源生命周期

文件没有全局可变状态、锁、通道、异步任务、事务或后台资源。每次调用独立分配缓冲区，摘要与盐只活到函数返回；因此算法本身可被多个线程并发调用。`NewHashPassword` 每次直接访问 `OsRng`，不缓存或共享伪随机生成器状态。

敏感中间值使用普通 `Vec<u8>` 和 `String`，离开作用域后由 Rust 释放，但没有主动清零；这与 Go 实现一致，却不提供内存擦除保证。超长口令会放大 `buf_dp` 的平方级分配，外部输入边界和资源限制仍应由更上层控制。

## 与 Go 版本的对应关系

[`caching_sha2.go`](caching_sha2.go) 是逐步对照基准：常量、crypt-base64 字母表、A/B/DP/DS 构造、轮次混合条件、摘要字节置换、`$A$` 格式以及 SHA-256/SM3 插件分派均保持一致。Rust 使用函数指针 `fn(&[u8]) -> Vec<u8>` 对应 Go 的 `func([]byte) []byte`，并用 `Vec<u8>` 对应 `bytes.Buffer`。

需要注意的语言层映射包括：Rust 用 `wrapping_mul` 模拟 Go `int64` 二补码乘法，再尝试转为 `usize`；转换失败按零轮处理。Go 对过短盐切片同样会 panic。Go 忽略 `rand.Read` 返回的错误，Rust 也没有把随机源错误加入返回类型。Rust 的未知插件行为仍是生成空串或校验为 false。

[`caching_sha2_test.rs`](caching_sha2_test.rs) 对照 [`caching_sha2_test.go`](caching_sha2_test.go)，共享 `foobar` 固定向量，覆盖正确/错误口令、分段错误、摘要类型错误、非法轮数和随机盐往返。[`tidb_sm3_test.rs`](tidb_sm3_test.rs) 与 Go 的 `tidb_sm3_test.go` 复用同一核心路径验证 SM3 分支；综合迁移测试还明确断言未知插件生成空串。

## 扩展指南

新增摘要算法时，应先确认它严格输出 32 字节；随后在 `CheckHashingPassword` 与 `NewHashPassword` 两处分派中同步接入，并在 `astersql-parser-mysql` 定义稳定插件名。若新算法不是 32 字节摘要或不使用相同 `$A$` 格式，不应直接复用 `hashCrypt`，而应建立独立格式和解析入口，避免破坏现有字节置换不变量。

修改轮数编码、盐长度或摘要排列时，必须同时评估已存储认证字符串兼容性，并同步 Go 对照和独立测试。错误处理增强（例如拒绝短盐、零轮或过大轮数）应首先在 [`caching_sha2_test.rs`](caching_sha2_test.rs) 添加对应回归用例；SM3 共用解析逻辑，因此还应同步 [`tidb_sm3_test.rs`](tidb_sm3_test.rs)。测试逻辑应继续保留在独立测试文件，不嵌入生产源文件。

性能修改应重点观察口令长度平方级的 DP 缓冲区和每轮 `buf_c` 分配，任何优化都必须用固定 Go 向量证明输出逐字节不变。若要在生产认证校验链接入 `CheckHashingPassword`，应从实际认证服务入口单独完成接线与端到端测试，不能仅凭本模块单元测试推断已经可用。

## 验证依据

- RustCodeGraph：`status` 确认索引包含 11,467 个文件；`files --filter pkg/parser/auth` 确认目标、Go 对照与独立测试均在索引内；`node --file pkg/parser/auth/caching_sha2.rs` 读取完整 239 行及 6 个符号；`callers/callees` 确认内部调用边，并暴露跨文件 callers 未覆盖的限制。
- 源码与 crate 边界：[`caching_sha2.rs`](caching_sha2.rs)、[`tidb_sm3.rs`](tidb_sm3.rs)、[`lib.rs`](lib.rs)、[`Cargo.toml`](Cargo.toml)。
- 生产调用证据：[`pkg/executor/utils.rs`](../../executor/utils.rs) 的 `encodedPassword`。
- Go 对照：[`caching_sha2.go`](caching_sha2.go)、[`caching_sha2_test.go`](caching_sha2_test.go) 和 `tidb_sm3_test.go`。
- Rust 测试证据：[`caching_sha2_test.rs`](caching_sha2_test.rs)、[`tidb_sm3_test.rs`](tidb_sm3_test.rs)、[`migration_aster_unit_test.rs`](migration_aster_unit_test.rs)。
- 本任务是纯文档分析，按计划不运行 Cargo；验收使用任务指定的 11 章节结构命令，并人工检查所有行为结论均可回到上述符号、调用点或测试。
