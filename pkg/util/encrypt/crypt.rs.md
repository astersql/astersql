# `pkg/util/encrypt/crypt.rs`

## 文件定位

本文件属于 `astersql-util-encrypt` crate，提供与 MySQL 历史 `ENCODE`/`DECODE` 行为兼容的字节置换算法。模块由 [`pkg/util/encrypt/lib.rs`](lib.rs) 声明并再导出，直接服务于 [`pkg/expression/builtin_encryption.rs`](../../expression/builtin_encryption.rs) 中的 SQL 标量函数运行时，而不是 AES、哈希或现代认证加密实现。

这里的命名沿用 MySQL/TiDB 历史接口：`SQLDecode` 对测试中的明文执行变换并产生不可读的二进制结果，`SQLEncode` 用相同密码将该结果还原。源文件注释明确指出该算法不具备现代密码学强度；它存在的目的仅是兼容既有 SQL 语义，不能作为新协议或静态数据的安全加密方案。

## 核心职责

文件承担三层职责：

1. `randStruct` 从密码字节派生两个 31 位种子，并产生确定性的伪随机序列。
2. `sqlCrypt::new` 用该序列洗牌 0 到 255 的字节表，再构造其逆置换表。
3. `SQLDecode` 和 `SQLEncode` 为调用者复制输入、建立一次性上下文、执行原地变换并返回新字节向量。

算法对相同输入和密码完全确定，固定向量由 [`pkg/util/encrypt/crypt_test.rs`](crypt_test.rs) 和 Go 对照测试验证。它不负责 SQL `NULL` 传播、类型转换、警告或函数注册；这些属于上层表达式框架。

## 主要符号

- `randStruct { seed1, seed2, maxValue, maxValueDbl }`：模块私有的双种子伪随机状态。整数状态用于更新，`maxValueDbl` 用于把 `seed1` 归一化为 `[0, 1)` 浮点值。
- `randStruct::randomInit(&mut self, password: &[u8])`：以常量 `1345345333`、`7`、`0x12345671` 为初态扫描密码；空格和制表符不参与派生；通过 `wrapping_add`/`wrapping_mul` 明确保持 Go `uint32` 溢出语义，最后将种子约束到 `0x3fff_ffff` 模数内。
- `randStruct::myRand(&mut self) -> f64`：按双种子递推式推进状态并返回归一化值。它既用于初始化置换表，也用于处理每个数据字节，因此调用次数和顺序是兼容性约束。
- `sqlCrypt { rand, decodeBuff, encodeBuff, shift }`：模块私有、单次变换使用的上下文。两个 256 字节数组互为逆置换，`shift` 保存逐字节反馈状态。
- `sqlCrypt::new(password: &[u8]) -> Self`：初始化 `decodeBuff` 为恒等表，执行 256 次伪随机交换，然后反向填写 `encodeBuff`。构造结束后的随机状态直接用于数据阶段。
- `sqlCrypt::encode(&mut self, data: &mut [u8])`：先更新随机 shift，以 `encodeBuff` 映射原字节并异或 shift，最后把原字节反馈进 shift。
- `sqlCrypt::decode(&mut self, data: &mut [u8])`：先更新随机 shift，对输入异或后查 `decodeBuff`，再把恢复出的原字节反馈进 shift；它与 `encode` 在相同密码和初始状态下互逆。
- `pub fn SQLDecode(str_: &[u8], password: &[u8]) -> Result<Vec<u8>, Infallible>`：公开的历史 `DECODE` 兼容入口；复制输入后调用 `decode`。
- `pub fn SQLEncode(data: &[u8], password: &[u8]) -> Result<Vec<u8>, Infallible>`：公开的历史 `ENCODE` 兼容入口；复制输入后调用 `encode`。

文件没有 trait、模块级可变变量、条件编译项或外部可见类型；公开面只有两个函数。

## 执行流程

一次调用的完整流程如下：

1. `SQLDecode` 或 `SQLEncode` 用 `to_vec` 复制调用者输入，因而不会修改原切片。
2. `sqlCrypt::new` 调用 `randomInit` 扫描完整密码。密码中的空格和制表符被跳过；其他字节（包括 UTF-8 的每个编码字节）都参与无符号 32 位运算。
3. 构造函数生成恒等 `decodeBuff`，随后做 256 次交换。每次索引为 `(myRand() * 255.0) as usize`；接着遍历洗牌结果建立逆表 `encodeBuff[decodeBuff[i]] = i`。
4. 数据阶段按输入顺序逐字节调用 `myRand`，把伪随机值折入 `shift`，执行查表和异或，并将明文字节反馈到 `shift`。反馈使任一位置的输出依赖此前处理的所有字节。
5. 公开函数返回变换后的 `Vec<u8>`。空输入仍会完成上下文构造，但数据循环不执行并返回空向量。

对同一密码分别创建全新的上下文时，有 `SQLEncode(SQLDecode(plain, password), password) == plain`。不能在不重置上下文的情况下把分段调用等同于多个公开函数调用，因为公开函数每次都会重新派生随机状态和清零 `shift`。

## 数据与状态

所有状态都是单次调用局部状态。`randStruct` 的四个字段和 `sqlCrypt::shift` 在处理过程中原地变化；`decodeBuff`/`encodeBuff` 在构造后只读。置换表覆盖全部 256 个字节值，因此输入和输出都是任意二进制数据，不要求有效 UTF-8。

确定性依赖以下不变量：两个表必须互逆；初始化阶段必须恰好消费 256 个随机值；数据阶段必须每字节消费一个随机值；`shift` 必须以零开始并反馈原始/恢复后的明文字节。修改循环边界、浮点到整数的截断方式、32 位回绕行为或反馈顺序都会改变固定密文向量。

每次调用的时间复杂度为 `O(256 + n)`，其中 `n` 是输入长度；固定上下文含两个 256 字节数组，返回值另分配并复制 `n` 字节。`pkg/util/encrypt/Cargo.toml` 声明了整个 crate 的 AES/模式/随机数依赖，但本文件自身只使用 Rust 标准库，算法也不从操作系统随机源取熵。

## 依赖与调用关系

crate 边界由 [`pkg/util/encrypt/Cargo.toml`](Cargo.toml) 的包名 `astersql-util-encrypt` 和 `lib.rs` 入口确定；`lib.rs` 通过 `pub mod crypt` 公开模块，并以 `pub use crypt::*` 再导出两个入口。工作区根清单还以 `facade_util_encrypt` 引用该 crate。

RustCodeGraph 对目标文件的索引显示，`SQLDecode` 的直接调用者包括 `pkg/expression/builtin_encryption.rs::sql_decode`、`crypt_test.rs::test_sql_decode` 和 `test_sql_encode`；`SQLEncode` 的直接调用者包括 `pkg/expression/builtin_encryption.rs::sql_encode` 与 `crypt_test.rs::test_sql_encode`。表达式 crate 在 [`pkg/expression/Cargo.toml`](../../expression/Cargo.toml) 中把 `astersql-util-encrypt` 绑定为依赖名 `encrypt`，再以 `encrypt::crypt as sql_crypt` 调用本模块。

内部调用链为：`SQLDecode -> sqlCrypt::new -> randStruct::randomInit/myRand`，随后进入 `sqlCrypt::decode -> randStruct::myRand`；编码链将最后一步换成 `sqlCrypt::encode`。模块没有文件、网络、配置、时钟或随机设备依赖。

## 错误处理与边界

两个公开函数返回 `Result<_, std::convert::Infallible>`，当前实现没有失败分支：任意长度的输入、空输入、空密码和任意字节密码都会产生结果。上层 `sql_decode`/`sql_encode` 因此直接 `unwrap`；这个结论只适用于当前 `Infallible` 契约，若未来引入可失败校验，上层也必须同步改为传播或映射错误。

密码中的 ASCII 空格和制表符会被忽略，因此只在这些字符上不同的密码可能派生相同状态。这是兼容算法事实，不应被“修正”。UTF-8 字符按编码后的多个字节处理；密文可能不是合法 UTF-8，因此 Rust API 返回 `Vec<u8>` 而不是 `String`。

算法没有认证标签、完整性校验、随机 nonce 或抗篡改保证，错误密码和损坏密文通常只会得到无意义字节而不会报错。长度保持不变，但可用内存仍受 `to_vec` 分配约束。实现中的 `u32` 回绕和模运算避免 debug/release 溢出行为差异；`maxValue` 在调用 `myRand` 前必由 `randomInit` 设为非零。

## 并发与资源生命周期

本模块没有全局状态、锁、原子变量、线程、异步任务、通道或句柄。每个公开调用创建独立的 `sqlCrypt`，因此多个线程并发调用不会共享或竞争随机状态、置换表与 `shift`；线程安全来自状态隔离，而不是同步原语。

输入借用只持续到公开函数复制完成，返回的 `Vec<u8>` 由调用者拥有。临时 `sqlCrypt` 在函数返回时释放，其固定数组和标量无需显式清理。当前实现没有对密码、置换表或中间状态做内存清零；考虑到这是兼容算法而非秘密管理组件，代码也没有承诺防止内存取证。

## 与 Go 版本的对应关系

直接对照文件是 [`pkg/util/encrypt/crypt.go`](crypt.go)。Rust 的 `randStruct::randomInit`/`myRand`、`sqlCrypt` 的表构造、`encode`/`decode` 以及公开 `SQLDecode`/`SQLEncode` 均逐步保留 Go 的常量、循环次数、`uint32` 回绕、浮点缩放、查表和 shift 反馈顺序。

两边的表示差异主要是：Go 公开 API 在 `string` 与 `[]byte` 间转换并返回 `(string, error)`；Rust 接受字节切片并返回 `Result<Vec<u8>, Infallible>`，避免要求二进制密文是 UTF-8。Go `sqlCrypt` 还保存初始化后的 `orgRand`，但该字段在对照实现中赋值后没有被读取；Rust 没有携带这个无行为作用的快照。Go 的 `init(password, length)`、`encode(data, length)`、`decode(data, length)` 接受显式长度，Rust 直接遍历完整切片，公开调用传入完整数据，因而当前语义一致。

[`pkg/util/encrypt/crypt_test.go`](crypt_test.go) 与 `crypt_test.rs` 使用相同的十组向量，包括空输入、空密码、ASCII、中文和日文；Rust 测试还按 Go 的流程先调用 `SQLDecode` 生成密文，再调用 `SQLEncode` 验证还原。`migration_aster_unit_test.rs::migration_sql_crypt_matches_go_binary_vectors_and_round_trips_utf8` 另复核代表性的 Go 二进制向量和 UTF-8 往返。

## 扩展指南

- 若要修复兼容算法，应优先修改 `randomInit`、`myRand`、`sqlCrypt::new`、`encode` 或 `decode` 中对应的最小步骤，并先确认 Go/MySQL 期望；不得把它替换成“更安全”的算法而继续沿用同一 SQL 接口，因为这会破坏已有密文兼容性。
- 新增边界向量时，应同步更新独立的 [`pkg/util/encrypt/crypt_test.rs`](crypt_test.rs)，并与 `crypt_test.go` 或明确的 MySQL 兼容证据比对。测试逻辑不要内嵌回生产源文件。重点覆盖二进制零字节、空格/制表符密码等价性、长输入、固定密文和双向往返。
- 若公开返回类型从 `Infallible` 改为可失败错误，必须同时审查 `pkg/expression/builtin_encryption.rs::sql_decode`/`sql_encode` 的 `unwrap`、SQL 层错误/NULL 语义以及所有调用测试。
- 若尝试流式或分块接口，必须显式保存同一个 `randStruct` 和 `shift` 跨块生命周期；逐块重新调用现有公开函数会重置状态并产生不同结果。还应评估上下文复用、并发所有权及敏感状态清理。
- 性能改动应保持 `O(n)` 数据路径和固定表规模，并用固定向量证明没有改变随机值消费次序、转换截断或置换方向。

## 验证依据

- RustCodeGraph `status`：索引包含本仓库 11,467 个文件，目标目录的 `crypt.rs`、`crypt_test.rs` 及 Go 对照均在索引中。
- RustCodeGraph `files --filter pkg/util/encrypt`：确认模块入口、Rust/Go 实现和独立测试文件；`node --file pkg/util/encrypt/crypt.rs --offset 1 --limit 260` 完整核对目标文件 124 行源码与 10 个索引符号。
- RustCodeGraph `explore "pkg/util/encrypt/crypt.rs crypt encrypt decrypt"`：确认公开入口的应用调用者为 `pkg/expression/builtin_encryption.rs`，并确认内部 `randomInit -> new`、`myRand -> new/encode/decode` 及测试调用边。由于 `encode`/`decode` 是常见名称，本文未采用搜索结果中其他模块的同名边作为本文件证据。
- 已读取 [`pkg/util/encrypt/Cargo.toml`](Cargo.toml)、[`pkg/util/encrypt/lib.rs`](lib.rs)、[`pkg/expression/Cargo.toml`](../../expression/Cargo.toml) 和 [`pkg/expression/builtin_encryption.rs`](../../expression/builtin_encryption.rs)，核对 crate 归属、模块导出、依赖别名与 SQL 运行时接线。
- 已逐段对照 [`pkg/util/encrypt/crypt.go`](crypt.go)、[`pkg/util/encrypt/crypt_test.rs`](crypt_test.rs)、[`pkg/util/encrypt/crypt_test.go`](crypt_test.go) 和 [`pkg/util/encrypt/migration_aster_unit_test.rs`](migration_aster_unit_test.rs)，核对算法步骤、固定向量、空值/多语言边界和 Rust/Go 表示差异。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前使用任务指定命令验证本文恰有 11 个固定二级章节，并人工复查所有行为陈述均能回溯到上述符号、调用边或对照文件。
