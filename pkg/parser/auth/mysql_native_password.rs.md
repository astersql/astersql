# `pkg/parser/auth/mysql_native_password.rs`

## 文件定位

本文件实现 MySQL 旧式 `mysql_native_password` 认证所需的双重 SHA-1 密码摘要、摘要文本编解码以及握手响应校验。它属于 Cargo 包 `astersql-parser-auth`；该包以 `pkg/parser/auth/lib.rs` 为 crate 根，并由公开模块 `mysql_native_password` 直接导出。因此，这里是认证算法的无状态基础工具层，不负责网络握手、用户记录读取或认证插件选择。

在当前 Rust 调用链中，`EncodePassword` 已被 `pkg/executor/utils.rs::encodedPassword` 用于默认/原生认证插件的用户密码落库编码，也被 `pkg/expression/builtin_encryption.rs::mysql_password` 用于兼容已弃用的 SQL `PASSWORD()`。RustCodeGraph 将目标文件列为由上述表达式实现和两个测试文件使用；仓库内文本检索还确认了执行器调用。`CheckScrambledPassword`、`DecodePassword`、`EncodePasswordBytes` 和 `Sha1Hash` 是公开 API，但当前仓库内可见 Rust 调用主要集中在独立测试，仍可能供 crate 外部消费者调用。

## 核心职责

- `Sha1Hash` 计算任意字节串的 20 字节 SHA-1 摘要，为本文件其他算法提供唯一的基础散列步骤。
- `EncodePassword` 与 `EncodePasswordBytes` 计算 `SHA1(SHA1(password))`，再编码为 `*` 加 40 位大写十六进制文本；空密码按 MySQL/TiDB 约定返回空字符串，而不是空串的双重摘要。
- `DecodePassword` 跳过存储文本的首字节并进行十六进制解码，将 `*<40 hex>` 恢复为 stage2 摘要。该函数只做字节级前缀跳过与 hex 解码，不验证首字节确为 `*`，也不验证结果固定为 20 字节。
- `CheckScrambledPassword` 使用服务端 salt、已存储的 stage2 摘要和客户端 auth token 恢复 stage1 摘要，再散列并与 stage2 比较，以判定一次 native-password 握手响应是否匹配。

本文件只实现兼容算法，不表示 SHA-1 是新认证方案的安全推荐；源码注释也明确将其限定为 MySQL 旧协议兼容用途。

## 主要符号

- `pub fn CheckScrambledPassword(salt: &[u8], hpwd: &[u8], auth: &[u8]) -> bool`：认证校验入口。`hpwd` 语义上是 `SHA1(SHA1(password))`；函数先算 `SHA1(salt || hpwd)`，要求其长度与 `auth` 相同，逐字节异或得到候选 stage1，再检查 `Sha1Hash(candidate_stage1) == hpwd`。
- `pub fn Sha1Hash(bytes: &[u8]) -> Vec<u8>`：调用 `sha1::Sha1::digest` 的薄封装，返回新分配的 20 字节向量。
- `pub fn EncodePassword(password: &str) -> String`：字符串入口，以 UTF-8 原始字节计算双重 SHA-1；非空结果由 `hex::encode_upper` 生成大写 hex，并添加 `*`。
- `pub fn EncodePasswordBytes(password: &[u8]) -> String`：原始字节入口，可处理非 UTF-8 密码字节；其余行为与 `EncodePassword` 相同。
- `pub fn DecodePassword(password: &str) -> Result<Vec<u8>, hex::FromHexError>`：从输入原始字节的索引 1 开始解码 hex。返回 `hex::FromHexError`，但输入为空时在切片阶段会 panic，错误类型无法覆盖该情形。

文件没有自定义类型、trait、`impl`、模块级常量或条件编译项；五个函数全部公开，命名保留 Go 风格，并由 crate 根的 `#![allow(non_snake_case)]` 接受。

## 执行流程

密码落库编码流程如下：调用方把明文交给 `EncodePassword` 或 `EncodePasswordBytes`；空输入立即返回 `""`；非空输入先生成 stage1=`SHA1(password)`，再生成 stage2=`SHA1(stage1)`；最后输出 `"*" + uppercase_hex(stage2)`。`pkg/executor/utils.rs::encodedPassword` 在用户以明文指定密码且认证插件不是 caching-SHA2、TiDB-SM3 或 socket 时走此路径。

SQL 兼容流程由 `pkg/expression/builtin_encryption.rs::mysql_password` 发起：空字符串直接返回空结果；非空字符串调用 `EncodePassword`，并同时产生 `PasswordDeprecated` 警告。警告属于表达式层职责，本文件自身不会记录警告。

握手校验流程由 `CheckScrambledPassword` 完成：

1. 连续向 SHA-1 状态写入 `salt` 和 `hpwd`，得到 scramble hash=`SHA1(salt || stage2)`。
2. 若客户端 `auth` 长度不等于 SHA-1 输出长度（20 字节），立即返回 `false`，避免 zip 截断后接受畸形 token。
3. 计算 `scramble_hash XOR auth`。按协议 `auth = stage1 XOR scramble_hash`，所以结果应恢复 stage1。
4. 对恢复值再次调用 `Sha1Hash`，并与传入的 `hpwd` 做完整字节比较；相等返回 `true`，否则返回 `false`。

解码流程由 `DecodePassword` 完成：它按 Go 字符串的字节索引语义跳过第一个字节，再交给 `hex::decode`。正常编码值会得到 20 字节 stage2，供握手校验使用。

## 数据与状态

所有输入都通过借用传入，函数不保存全局状态。SHA-1 上下文、stage1、stage2、异或缓冲区和输出字符串均为单次调用内的局部值；返回后不会保留 salt、密码或摘要引用。

`CheckScrambledPassword` 中的 `hash` 是新分配的可变 `Vec<u8>`，异或只修改该局部缓冲区，不修改调用方的 `salt`、`hpwd` 或 `auth`。编码函数会为两个摘要及最终字符串分配内存；`Sha1Hash` 和 `DecodePassword` 也返回拥有所有权的 `Vec<u8>`。

关键数据不变量是 SHA-1 输出固定 20 字节，因此规范编码文本为 41 个 ASCII 字节（一个 `*` 加 40 位 hex），规范 auth token 也是 20 字节。不过只有握手函数显式校验 auth 长度；`DecodePassword` 并不强制前缀、总长度或解码后长度。

## 依赖与调用关系

直接外部依赖只有 Cargo 中声明的 `sha1 = "0.10"` 和 `hex = "0.4"`：前者提供 `Digest` trait 与 `Sha1`，后者提供大写编码、解码和 `FromHexError`。`pkg/parser/auth/Cargo.toml` 将 crate 命名为 `astersql-parser-auth`，`pkg/parser/auth/lib.rs` 通过 `pub mod mysql_native_password` 暴露本文件，并在兼容命名空间 `parser::auth` 中再次聚合导出。

已验证的上游调用边包括：

- `pkg/executor/utils.rs::encodedPassword -> EncodePassword`：选择默认/`mysql_native_password` 等非特殊插件时生成存储哈希。
- `pkg/expression/builtin_encryption.rs::mysql_password -> EncodePassword`：实现 SQL `PASSWORD()` 的历史格式。
- `pkg/parser/auth/mysql_native_password_test.rs` 与 `pkg/parser/auth/migration_aster_unit_test.rs` 调用全部五个函数，验证 Go 测试向量与迁移边界。

文件内部调用边为 `EncodePassword -> Sha1Hash`（两次）、`EncodePasswordBytes -> Sha1Hash`（两次）以及 `CheckScrambledPassword -> Sha1Hash`（一次）。`DecodePassword` 直接调用 `hex::decode`，没有调用其他本地符号。RustCodeGraph 的 `callers/callees` 命令未为这些自由函数返回边，但文件使用关系、索引源码和仓库文本检索共同确认了上述直接调用；文档没有据此推断未出现的网络握手接线。

## 错误处理与边界

- `CheckScrambledPassword` 不返回错误细节。auth 长度不是 20 字节或最终摘要不匹配时均返回 `false`；独立 Rust/Go 测试明确覆盖短 token `b"xxyyzz"`，要求拒绝而不 panic。
- `Sha1Hash` 使用 RustCrypto 的不可失败内存 API，因此不像 Go 版 `hash.Write` 路径那样经过 `terror.Log(errors.Trace(err))`；这不是认证结果语义的删减，而是依赖 API 的错误模型不同。
- 两个编码函数把空输入特殊映射为空字符串；调用者不能把该结果传给 `DecodePassword`，因为后者对空输入执行 `[1..]` 会越界 panic。
- `DecodePassword` 对非空输入不检查首字节是否为星号，任何单字节前缀都会被丢弃。剩余部分含非法 hex、奇数个 hex 字符或非 ASCII 字节时返回 `hex::FromHexError`。迁移测试以 `"é23"` 验证多字节 UTF-8 前缀不会因字符边界切片而 panic，而会进入 hex 解码并返回错误。
- 摘要比较使用普通字节切片相等比较，没有声明或保证常量时间。若将该函数置于可被远程精细计时的安全边界，需单独评估时序侧信道兼容方案，不能只改此处而忽略协议层。

## 并发与资源生命周期

本文件没有锁、原子变量、通道、异步任务、线程、事务、文件或网络资源。各函数仅操作调用栈上的摘要状态及本次调用拥有的堆缓冲区，无共享可变状态，因此可以由多个线程并发调用。

密码字节和中间摘要不会被显式清零；它们随局部值析构并交还分配器。若未来引入长期缓存、硬件密钥接口或显式清零要求，应在认证系统整体威胁模型下处理，而不能假定当前 `Vec<u8>` 析构会擦除内存。

## 与 Go 版本的对应关系

直接对照文件为 `pkg/parser/auth/mysql_native_password.go`，独立测试为 `pkg/parser/auth/mysql_native_password_test.go`。五个 Rust 公开函数分别对应同名 Go 函数，核心公式、空密码规则、`*` 前缀大写 hex 格式、首字节跳过解码和无效 auth 长度拒绝语义一致。Rust 测试沿用 Go 的固定期望：`"123"` 编码为 `*23AE809DDACAF96AF0FD78ED04B6A265E05AA257`；`"abc"` 的固定 salt/auth 向量可以通过校验；短 auth 返回 `false`。

实现层差异包括：Go 用 `crypto/sha1`、`encoding/hex` 和 `fmt.Sprintf("*%X")`，Rust 用 RustCrypto `sha1` 与 `hex` crate；Go 对理论上的 `hash.Write` 错误执行 `terror.Log(errors.Trace(err))`，Rust 的 digest/update API 不产生对应错误；Go 的 `DecodePassword` 用字符串字节切片 `pwd[1:]` 并用 PingCAP errors 包装解码错误，Rust 用 `password.as_bytes()[1..]` 保留字节切片语义并直接返回 `hex::FromHexError`。两版对空字符串解码都会在跳过首字节时越界，而正常调用应先通过编码/存储格式约束排除该输入。

Rust 额外的 `migration_aster_unit_test.rs::mysql_native_password_vectors_match_go` 覆盖空密码编码和多字节前缀解码错误；这些是移植语义的补充证据，不改变 Go 算法。

## 扩展指南

- 调整 native-password 摘要文本格式时，应同时修改 `EncodePassword`、`EncodePasswordBytes` 和 `DecodePassword`，并核对执行器中 `PWDHashLen + 1` 与 `starts_with('*')` 的验证约束；还要扩展独立测试 `mysql_native_password_test.rs`，不能把测试嵌回生产源文件。
- 调整握手公式、salt 拼接顺序或 token 长度策略时，应以 `CheckScrambledPassword` 为入口，并新增正确密码、错误密码、不同 salt、空/短/长 auth 与异常 hpwd 长度测试；协议兼容风险很高，必须继续与 Go 函数和客户端向量逐项对齐。
- 若要支持新的认证插件，不应复用或改写 SHA-1 公式来模拟新协议；应新增独立模块，并在 `pkg/executor/utils.rs::encodedPassword` 的插件分派层接线，避免改变既有 `mysql_native_password` 存量账号行为。
- 若要强化解码校验，可在 `DecodePassword` 中验证非空、`*` 前缀、41 字节规范长度和 20 字节结果，但这会收紧当前接受面，必须先调查所有 crate 外调用者并与 Go 行为协调。至少应为当前空输入 panic 和任意前缀接受行为添加明确回归测试，再决定兼容策略。
- 性能上每次编码会生成两个摘要向量和一个格式化字符串。除非基准证明它处于热点，不应为减少小额分配而牺牲清晰度；任何优化都必须保持原始字节入口，避免强制 UTF-8 转换。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；目标目录中的 Rust、Go 实现及测试均已索引。
- RustCodeGraph `files --filter pkg/parser/auth` 与 `node --file pkg/parser/auth/mysql_native_password.rs`：确认目标文件的五个公开函数、文件内流程，以及索引报告的使用文件 `pkg/expression/builtin_encryption.rs`、`pkg/parser/auth/migration_aster_unit_test.rs`、`pkg/parser/auth/mysql_native_password_test.rs`。
- RustCodeGraph `query`：分别定位 Rust/Go 的 `CheckScrambledPassword`、`Sha1Hash`、`EncodePassword`、`EncodePasswordBytes`、`DecodePassword`；`node` 读取了执行器、表达式和迁移测试中的直接调用位置。对目标自由函数运行 `callers/callees` 未返回图边，因此调用关系另用索引文件内容和精确文本检索复核，没有扩大推断范围。
- 已读源码与配置：`pkg/parser/auth/mysql_native_password.rs`、`pkg/parser/auth/lib.rs`、`pkg/parser/auth/Cargo.toml`、`pkg/executor/utils.rs`、`pkg/expression/builtin_encryption.rs`。
- 已读 Go 对照与测试：`pkg/parser/auth/mysql_native_password.go`、`pkg/parser/auth/mysql_native_password_test.go`；已读 Rust 独立测试：`pkg/parser/auth/mysql_native_password_test.rs`、`pkg/parser/auth/migration_aster_unit_test.rs`。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前使用任务指定命令验证文档存在且恰有十一个固定二级章节，并人工复核本说明只陈述上述源码、调用和测试能够支持的事实。
