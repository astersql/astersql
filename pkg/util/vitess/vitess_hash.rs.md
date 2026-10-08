# `pkg/util/vitess/vitess_hash.rs`

## 文件定位

本文件是 `astersql-util-vitess` crate 的业务实现文件，提供与 Vitess/TiDB Go 实现兼容的 64 位分片键哈希。crate 边界由 `pkg/util/vitess/Cargo.toml` 定义，库入口是 `pkg/util/vitess/lib.rs`；入口通过 `pub mod vitess_hash` 声明模块，并用 `pub use vitess_hash::*` 再导出本文件的公开函数。workspace 根 `Cargo.toml` 同时把该 crate 纳入成员列表，并以 `facade_util_vitess` 依赖名接入 `pkg/lib.rs` 的 `util::vitess` 门面。

当前接线需要区分 Go 与 Rust：Go 的 SQL 表达式实现 `pkg/expression/builtin_miscellaneous.go` 和向量实现 `pkg/expression/builtin_miscellaneous_vec.go` 会直接调用 Go 版 `vitess.HashUint64`；代码搜索未发现生产 Rust 文件直接调用本文件的 `HashUint64`。Rust expression 侧目前在 `pkg/expression/builtin_miscellaneous.rs::vitess_hash_u64` 和 `pkg/expression/builtin_miscellaneous_vec.rs::vitess_hash` 中各自保留了同算法实现。因此，本文件已经是公开的兼容工具，但不能据现有证据声称它已经成为 Rust SQL 表达式执行主链的唯一实现。

## 核心职责

`HashUint64` 把一个 `u64` 分片键编码成 8 字节大端块，使用固定的 8 字节全零 DES 密钥执行单块加密，再把密文按大端解释成 `u64`。这不是面向安全用途的密码接口，而是为了复现 Vitess 的确定性键空间映射；算法、密钥和字节序都是兼容协议的一部分，不能在不改变哈希结果的前提下替换。

文件还负责缓存 DES cipher：`NULL_KEY_BLOCK` 通过 `LazyLock<Des>` 在首次使用时初始化一次，后续调用共享同一不可变 cipher，避免每次哈希都重新建立密钥调度。固定密钥长度恰为 DES 所需的 8 字节，所以构造失败被视为不可达的程序不变量。

## 主要符号

- `static NULL_KEY_BLOCK: LazyLock<Des>`：模块私有的全局 DES cipher。初始化闭包调用 `Des::new_from_slice(&[0_u8; 8])`，以固定全零密钥建立 cipher；`expect` 的信息记录了“8 字节 DES key 必须有效”的不变量。
- `pub fn HashUint64(shardKey: u64) -> Result<u64, Infallible>`：唯一公开业务 API。参数是完整的无符号 64 位分片键；返回值保留 Go API 的“值加错误”形状，但 Rust 类型把运行期错误收窄为 `Infallible`。函数命名沿用 Go 风格，crate 根的 lint 配置允许 `non_snake_case`。
- `Block::<Des>`、`BlockEncrypt::encrypt_block` 与 `KeyInit`：来自 `des = "0.8"` 的块类型和 trait。`Block::<Des>` 恰为 8 字节，与 `u64::to_be_bytes`/`u64::from_be_bytes` 的宽度一致。

本文件没有自定义 struct、enum、trait、`impl`、条件编译分支或可变模块级状态；公开面只有 `HashUint64`，`NULL_KEY_BLOCK` 不对 crate 外暴露。

## 执行流程

1. 调用者把分片键作为 `u64` 传给 `HashUint64`。
2. 函数用 `Block::<Des>::default()` 分配并清零一个 8 字节块，再用 `shardKey.to_be_bytes()` 覆盖全部字节。这里的大端序与 Go 的 `binary.BigEndian.PutUint64` 一致。
3. 首次调用会触发 `NULL_KEY_BLOCK` 的惰性初始化；后续调用直接复用已初始化的全零密钥 DES cipher。
4. `encrypt_block(&mut hashed)` 原地把输入块变换为 8 字节密文。它是单块、无 IV、无填充的确定性变换。
5. `u64::from_be_bytes(hashed.into())` 以大端序把密文还原成数值，并包装成 `Ok` 返回。

固定向量可由 `pkg/util/vitess/vitess_hash_test.rs::TestVitessHash` 复核，例如输入 `u64::MAX` 得到 `0x3555_50b2_150e_2451`。`pkg/util/vitess/migration_aster_unit_test.rs::hash_uint64_is_deterministic_and_not_an_identity_hash` 另行验证相同输入结果稳定，并对选定样例验证输出不是输入原值。

## 数据与状态

输入、工作块和输出都固定为 64 位/8 字节，不发生长度协商、堆积缓存或跨调用数据保留。`hashed` 是函数栈上的局部块，先被输入的全部 8 字节覆盖，再被 DES 原地改写；返回前转成独立的 `[u8; 8]` 和 `u64`。

唯一跨调用状态是 `NULL_KEY_BLOCK`。它的值由编译期固定的 `[0_u8; 8]` 决定，初始化后不再变化；没有随机数、时间、租户、session 或配置输入。因此同一输入在所有调用、进程和平台上都应得到同一输出。大端转换显式写在两端，使结果不依赖主机原生端序。

DES 对 64 位块在固定密钥下是置换，所以该函数在完整 `u64` 域上不会因算法本身产生两个不同输入映射到同一 64 位输出；不过代码把它作为兼容哈希使用，并未提供密码学抗攻击保证。

## 依赖与调用关系

直接标准库依赖是 `std::convert::Infallible` 和 `std::sync::LazyLock`。外部依赖仅有 `des = "0.8"`，由 `pkg/util/vitess/Cargo.toml` 声明；本文件使用其 `Des` 类型以及 `cipher::{Block, BlockEncrypt, KeyInit}` trait/API。crate 没有 feature 条件。

导出链为 `vitess_hash.rs::HashUint64` → `pkg/util/vitess/lib.rs` 的 glob 再导出 → workspace 依赖 `facade_util_vitess` → `pkg/lib.rs::util::vitess` 门面。RustCodeGraph 将目标文件识别为已索引文件并定位到 `HashUint64`，但对该精确符号的 callers/callees 查询没有返回调用边；后续 `rg` 核验到的 Rust 直接调用仅位于 `vitess_hash_test.rs` 和 `migration_aster_unit_test.rs`。

Go 应用链则是明确的：`builtinVitessHashSig::evalInt` 实现 SQL `VITESS_HASH`，`builtinTidbShardSig::evalInt` 在哈希后对 bucket 数取模，向量化表达式逐行调用 `vitess.HashUint64` 并传播 NULL。上述是 Go 版同名函数的调用者，不应误记为本 Rust 函数的调用边。Rust expression 中的标量和向量实现与本文件依赖相同的 null-key DES 规则，但目前是重复实现。

## 错误处理与边界

`HashUint64` 的返回错误类型是 `Infallible`，函数体也只有 `Ok(...)` 路径；对任意 `u64`，包括 `0` 和 `u64::MAX`，没有可返回的业务错误。与 Go 的 `(uint64, error)` 相比，这保留了调用形状，同时在类型层表达固定密钥初始化不会产生可恢复错误。

唯一可能中止进程的路径位于 `NULL_KEY_BLOCK` 初始化闭包中的 `expect`。其输入永远是源码内固定的 8 字节数组，而 DES 构造器接受 8 字节密钥，所以在当前依赖契约下该 panic 不可达；如果未来密钥来源或 cipher 类型变化，必须重新设计错误边界，不能继续假设 `Infallible`。

函数不检查分片键的业务含义，也不处理 SQL NULL 或有符号数。调用层负责 NULL 传播；有符号 SQL 整数在 Go `evalInt` 中先以 `uint64(shardKeyInt)` 按位转换。Rust expression 的 `vitess_hash(i64)` 同样用 `as u64` 保留二进制位模式。本 API 只接收转换后的 `u64`。

## 并发与资源生命周期

`LazyLock` 提供线程安全的一次性初始化：并发首次调用只会发布一个完整的 `Des` 值，其他线程等待或读取初始化结果。初始化后的 cipher 通过共享引用执行 `encrypt_block`，每次调用只修改自己的局部 `hashed` 块，因此函数没有数据竞争，也不需要调用者持锁。

本文件不创建线程、异步任务、通道、文件、网络连接或事务；没有显式关闭/回收动作。`NULL_KEY_BLOCK` 生命周期与进程一致，局部块随函数返回释放。若未来引入可变缓存或动态密钥，必须重新评估同步边界和跨租户状态隔离，不能沿用当前“全局、不可变、全零密钥”的并发结论。

## 与 Go 版本的对应关系

Go 基准实现位于 `pkg/util/vitess/vitess_hash.go`。对应关系如下：

- Go 的包级 `nullKeyBlock cipher.Block` 加 `init()` 对应 Rust 的 `LazyLock<Des>`；前者在包初始化期构造，后者推迟到首次访问，但可观察的哈希结果相同。
- Go 的 `des.NewCipher(make([]byte, 8))` 对应 Rust 的 `Des::new_from_slice(&[0_u8; 8])`，都使用 8 字节全零密钥。
- Go 的 `binary.BigEndian.PutUint64`/`Uint64` 对应 Rust 的 `to_be_bytes`/`from_be_bytes`。
- Go 的 `nullKeyBlock.Encrypt(hashed[:], keybytes[:])` 对应 Rust 的 `encrypt_block(&mut hashed)`；两者都是对一个 8 字节块的原地/等价单块加密，没有 IV 或 padding。
- Go 声明返回 `(uint64, error)` 且正常实现总是返回 `nil` error；Rust 返回 `Result<u64, Infallible>`，以类型明确没有可恢复失败。

`pkg/util/vitess/vitess_hash_test.go::TestVitessHash` 的五个固定向量被 `pkg/util/vitess/vitess_hash_test.rs::TestVitessHash` 逐项迁移，Rust 测试还保持了大端、大写十六进制比较。`migration_aster_unit_test.rs` 重复核验数值向量并补充确定性/非恒等性质。现有测试没有覆盖输入 `0`，也没有跨 crate 调用或与 Rust expression 重复实现的一致性测试；这些是覆盖现状，不应被描述为已验证。

## 扩展指南

若只是新增兼容向量，应扩展独立测试文件 `pkg/util/vitess/vitess_hash_test.rs`，并优先同步 Go 的 `vitess_hash_test.go`；不要把测试嵌入生产源文件。边界向量可包括 `0`、最高位刚置位的值和有符号调用层转换后的位模式，并应同时断言数值或大端十六进制结果。

若要消除 Rust expression 中的重复实现，最可能的接入点是 `pkg/expression/builtin_miscellaneous.rs::vitess_hash_u64` 与 `pkg/expression/builtin_miscellaneous_vec.rs::vitess_hash`。接线时应复用本 crate 的公开函数，并同步 expression 的标量、向量测试；还需处理 `Result<u64, Infallible>`，验证 SQL NULL 传播、负 `i64` 到 `u64` 的按位转换和 `TIDB_SHARD` 取模行为不变。该重构超出本文档任务范围，当前文件没有做行为修改。

若要改变算法、密钥或端序，必须把它视为兼容性变更：已有分片键到 keyspace 的映射和固定测试向量都会变化。若 `des` 依赖 API 或密钥初始化可能失败，则需要把 `NULL_KEY_BLOCK` 改为可表达失败的初始化状态，并将公开错误类型从 `Infallible` 扩展到真实错误；同时评估所有门面与调用者。性能修改应保留一次性密钥调度和无额外分配的单块路径，并用独立基准或针对性测试证明结果不变。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 `pkg/util/vitess/vitess_hash.rs`；`files --filter pkg/util/vitess` 列出实现、crate 入口及测试；`node --file pkg/util/vitess/vitess_hash.rs --offset 1 --limit 240` 核对完整 43 行源码；`query HashUint64 --kind function` 精确定位 Rust 与 Go 同名实现；对精确 Rust 符号执行 `callers`/`callees` 未返回调用边。
- 生产源码：`pkg/util/vitess/vitess_hash.rs`（`NULL_KEY_BLOCK`、`HashUint64`）；`pkg/util/vitess/lib.rs`（模块声明和再导出）；`pkg/lib.rs`（`util::vitess` 门面）；`pkg/expression/builtin_miscellaneous.rs` 与 `builtin_miscellaneous_vec.rs`（当前 Rust expression 的同算法独立实现）。
- 配置：`pkg/util/vitess/Cargo.toml`（crate 名、`des = "0.8"`、测试依赖和 Go 包映射）；根 `Cargo.toml`（workspace 成员与 `facade_util_vitess` 依赖）。
- Go 对照：`pkg/util/vitess/vitess_hash.go`（全零密钥 DES 与大端转换）；`pkg/expression/builtin_miscellaneous.go` 和 `builtin_miscellaneous_vec.go`（Go SQL 标量、`TIDB_SHARD` 与向量调用位置）。
- 独立测试：`pkg/util/vitess/vitess_hash_test.rs`、`pkg/util/vitess/migration_aster_unit_test.rs`，以及对应的 `pkg/util/vitess/vitess_hash_test.go`。测试证明五个固定向量、输出大端 hex、确定性和一个非恒等样例；本纯文档任务按计划未运行 Cargo。
