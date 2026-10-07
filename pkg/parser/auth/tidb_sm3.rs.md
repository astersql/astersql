# `pkg/parser/auth/tidb_sm3.rs`

## 文件定位

本文件属于 `astersql-parser-auth` crate；crate 根 `pkg/parser/auth/lib.rs` 以 `pub mod tidb_sm3` 导出它，并在兼容迁移路径 `parser::auth` 下再次聚合导出。它实现 SM3（256 位摘要、512 位分组），既是 `tidb_sm3_password` 认证派生流程使用的底层哈希函数，也是表达式层 SM3 SQL 函数的摘要后端。

直接上游有三类：`pkg/parser/auth/caching_sha2.rs` 的 `CheckHashingPassword` 和 `NewHashPassword` 把 `Sm3Hash` 作为函数参数传给 `hashCrypt`；`pkg/expression/builtin_encryption.rs::sm3_hash` 计算标量结果；`pkg/expression/builtin_encryption_vec.rs::sm3_vec` 逐行计算向量结果。该文件自身只负责原始摘要，不解析认证字符串、不生成盐，也不把摘要编码成十六进制。

## 核心职责

- 维护可增量写入的 SM3 状态：八个 32 位链接字、累计消息位数和不足 64 字节的尾部缓冲（`sm3`）。
- 完成 SM3 的消息扩展、64 轮压缩和分组间链接（`sm3::update`，以及 `ff0`/`ff1`/`gg0`/`gg1`/`p0`/`p1`/`leftRotate`）。
- 按规范追加 `0x80`、零字节和 64 位大端原消息长度（`sm3::pad`）。
- 提供与 Go 实现形状接近的 `BlockSize`、`Size`、`Reset`、`Write`、`Sum`、`NewSM3`，以及一次性入口 `Sm3Hash`。

职责边界很重要：认证格式错误、迭代次数和随机盐由 `caching_sha2.rs` 处理；十六进制输出由表达式层的 `hex::encode` 处理。本文件不包含条件编译项，也不依赖 crate `Cargo.toml` 中列出的第三方密码学库来实现 SM3。

## 主要符号

- `pub struct sm3`：公开具体类型，但五个字段均私有。`digest: [u32; 8]` 是当前链接状态，`length: u64` 记录已写入消息的位数，`unhandleMsg: Vec<u8>` 保存尾段；`blockSize` 和 `size` 分别固定为 64、32。
- `ff0`、`gg0`：前 16 轮使用的异或布尔函数；`ff1`、`gg1`：后 48 轮使用的多数函数和条件选择函数。
- `p0`、`p1`：分别用于压缩结果和消息扩展的线性置换；`leftRotate` 用 `u32::rotate_left` 实现循环左移。
- `sm3::pad(&self) -> Vec<u8>`：克隆当前尾段并产生最终填充分组，不直接改变对象。
- `sm3::update(&self, msg: &[u8]) -> [u32; 8]`：从当前 `digest` 出发压缩所有完整分组，返回新状态；它不直接写回 `self`。
- `sm3::BlockSize`、`sm3::Size`：分别返回 64 和 32。命名保留 Go API 风格，因此 crate 根用 `#![allow(non_snake_case)]` 接纳这些名称。
- `sm3::Reset(&mut self)`：恢复 SM3 初始向量，清空长度和尾段，并设置尺寸常量。
- `sm3::Write(&mut self, input: &[u8]) -> Result<usize, Infallible>`：累计输入、压缩完整块、保存余数；成功值恒为输入字节数，错误类型表明该实现不会返回错误。
- `sm3::Sum(&mut self, input: &[u8]) -> Vec<u8>`：先将 `input` 写入当前状态，再基于填充后的临时块生成并仅返回 32 字节摘要。
- `NewSM3() -> sm3`：构造具体 Rust 类型并调用 `Reset`；与 Go 返回 `hash.Hash` 接口值不同。
- `Sm3Hash(data: &[u8]) -> Vec<u8>`：公开的一次性便捷入口，执行 `NewSM3`、`Write(data)`、`Sum(&[])`。

## 执行流程

一次性调用从 `Sm3Hash` 开始：创建并重置状态，将全部输入交给 `Write`，再以空切片调用 `Sum`。流式调用者则可自行 `NewSM3`，多次调用 `Write`，最后调用 `Sum(&[])`；`migration_aster_unit_test.rs::sm3_vectors_and_streaming_match_go` 用 `"a"`、`"bc"` 两次写入证明结果与一次性 `"abc"` 相同。

`Write` 先以回绕算术把 `input.len() * 8` 累加到 `length`，再把旧尾段与新输入拼接。`nblocks = msg.len() / 64` 决定完整分组数；`update` 只处理这些完整块，随后 `Write` 把 `nblocks * 64` 之后的字节保存回 `unhandleMsg`。因此任意拆分方式都落到相同的 64 字节分组边界。

每个分组在 `update` 中先按大端序读取 16 个 32 位字，再扩展为 `w[0..68]`，并生成 `w1[i] = w[i] ^ w[i + 4]`。第 0 至 15 轮使用 `ff0`、`gg0` 和常数 `0x79cc4519`；第 16 至 63 轮使用 `ff1`、`gg1` 和常数 `0x7a879d8a`。每轮更新八个工作寄存器，所有模 2^32 加法均显式使用 `wrapping_add`。分组结束时工作寄存器与进入该分组的链接状态逐字异或，成为下一分组的起点。

`Sum` 先调用 `Write(input)`，随后 `pad` 在尾段后追加 `0x80`，补零至块内偏移 56，并追加累计位长的 8 字节大端表示；这会生成一个或两个完整最终分组。`update` 对填充分组计算临时摘要，八个状态字再按大端序串联为固定 32 字节结果。最终摘要未写回 `digest`，但传给 `Sum` 的非空 `input` 已由前置 `Write` 永久计入对象状态。

## 数据与状态

`digest` 的初值由 `Reset` 固定为 SM3 规范的八个初始字；压缩期间另有栈上数组 `w: [u32; 68]`、`w1: [u32; 64]` 和八个工作寄存器。每处理一个完整块，`Write` 才把 `update` 的返回值提交到 `self.digest`。`unhandleMsg` 始终应短于 64 字节，`length` 则包含完整块与尾段的全部已写入位数。

`pad` 克隆尾段，因此最终填充不会污染 `unhandleMsg`。`Sum(&[])` 在通常用法下不会改变对象；`Sum(non_empty)` 是例外，它先改变 `length`、可能改变 `digest` 和尾段。`Reset` 是复用同一对象的明确生命周期边界。

长度累加采用 `u64::wrapping_add`，字节转位数也采用回绕乘法；极端累计输入超过 `u64` 位长时按模 2^64 编码，与无符号整数回绕意图一致。内部拼接与填充会分配 `Vec<u8>`：每次 `Write` 克隆旧尾段并复制新输入，`pad` 再克隆尾段；核心压缩数组本身位于栈上。

## 依赖与调用关系

下游仅使用 Rust 标准库能力：`Vec`、切片转换、`u32::from_be_bytes`/`to_be_bytes`、`u64::to_be_bytes`、循环左移、回绕算术和 `std::convert::Infallible`。`pkg/parser/auth/Cargo.toml` 定义该 crate 的根为 `lib.rs`，没有为本文件设置 feature 或条件依赖；文件也没有直接 `use` 外部 crate。

主要内部调用边为：`Sm3Hash -> NewSM3 -> Reset`，然后 `Sm3Hash -> Write -> update`，最后 `Sm3Hash -> Sum -> Write/pad/update`。压缩函数 `update` 调用消息置换和布尔辅助函数；`p0`、`p1` 继续调用 `leftRotate`。

主要上游调用边为：

- `caching_sha2.rs::{CheckHashingPassword, NewHashPassword} -> hashCrypt(..., Sm3Hash)`，服务 `AuthTiDBSM3Password` 的校验和生成；
- `builtin_encryption.rs::sm3_hash -> Sm3Hash -> hex::encode`，形成标量 SQL 可见十六进制值；
- `builtin_encryption_vec.rs::sm3_vec -> Sm3Hash -> hex::encode`，对非 NULL 行生成向量结果并保留 NULL；
- `tidb_sm3_test.rs` 与 `migration_aster_unit_test.rs` 直接调用 `Sm3Hash`/`NewSM3` 验证固定向量、流式行为和认证组合。

RustCodeGraph 的精确 `callees Sm3Hash` 也识别到 Rust 实现调用 `Write`、`Sum`、`NewSM3`；由于 `callers Sm3Hash` 对 Go/Rust 同名符号查询未稳定返回，以上跨文件上游边同时由限定目录的引用搜索和对应源码节点复核。

## 错误处理与边界

该摘要核心没有业务错误分支。`Write` 返回 `Result<usize, Infallible>`，并总是返回 `Ok(input.len())`；`Sm3Hash` 和 `Sum` 因此忽略其结果是类型安全的。`update` 中四字节切片转数组使用 `unwrap`，但循环前保证仅在 `msg.len() >= 64` 时读取一个完整块，且索引只覆盖该块的前 64 字节，所以在该不变量下不会因短切片失败。

输入边界包括空消息、恰好跨越 56 字节填充阈值、恰好 64 字节和多块消息。`pad` 的模循环会为尾段长度大于等于 56 的情况自然增加第二个最终块。现有固定向量覆盖 3 字节和 64 字节输入，流式测试覆盖跨多次 `Write` 的合并；空输入及 55/56/63/64/65 字节的系统化边界未在指定测试中直接展示，扩展时应补充。

必须特别保留 `Sum` 的现状：虽然 Go 注释声称把摘要追加到输入且不改变状态，实际 Go 代码会先 `Write(in)`，随后只返回新切出的摘要区域。Rust 测试 `sum_with_input_returns_only_digest_and_updates_state` 明确锁定了“只返回 32 字节且输入进入状态”的行为，不能按通用 `hash.Hash::Sum` 直觉擅自改写。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道、文件、套接字或事务；一次摘要的资源生命周期完全由一个 `sm3` 值及其 `Vec` 缓冲承担。对象构造后经若干次可变 `Write`/`Sum` 使用，必要时 `Reset` 复用，离开作用域时由 Rust 自动释放缓冲。

状态方法需要 `&mut self`，Rust 借用规则可阻止无同步的同一实例并发修改；文件本身没有提供跨线程共享包装。并发调用应为每个请求或行创建独立实例（一次性 `Sm3Hash` 天然如此），或由调用者在共享实例外加同步。只读辅助计算没有全局可变状态，因此多个独立摘要实例互不干扰。

性能上，压缩过程没有堆分配，但 `Write` 每次都会克隆尾段并把输入复制进新 `Vec`，向量表达式也会为每行创建摘要和十六进制结果。若优化缓冲复用，必须维持分块、累计位长、尾段不变量和 Go 对齐结果，并用独立基准或大输入测试证明收益。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/parser/auth/tidb_sm3.go`。Rust 保留了同名 `sm3` 状态字段、六个辅助函数、`pad`/`update`、五个类 `hash.Hash` 方法、`NewSM3` 和 `Sm3Hash`；初始向量、消息扩展公式、轮常数、寄存器轮转、分组链接和大端输出均逐项对应。Rust 用 `wrapping_add` 明确表达 Go `uint32` 的自然回绕，避免调试构建的整数溢出检查改变算法。

API 层存在两处明确差异。Go `NewSM3() hash.Hash` 返回接口对象，Rust `NewSM3() -> sm3` 返回具体类型，源码注释也标明尚未以等价 trait object 抽象。Go `Write` 的错误值恒为 `nil`，Rust 用 `Infallible` 表达不可能失败。Go 的 `Sum(in)` 通过切片容量操作最终只返回摘要区域；Rust 直接新建容量为 32 的向量，保持可观察返回值一致。

`pkg/parser/auth/tidb_sm3_test.go::TestSM3` 与 Rust `tidb_sm3_test.rs::test_sm3` 使用相同的 `abc` 和 64 字节长文本向量。Rust 额外的 `sum_with_input_returns_only_digest_and_updates_state` 固化 Go 实现的非常规 `Sum` 行为；`migration_aster_unit_test.rs::sm3_vectors_and_streaming_match_go` 额外验证分块写入。密码正确、错误及格式异常测试虽然位于同名测试文件中，但相关错误来自 `caching_sha2.rs` 的认证格式层，不应归因于本摘要核心。

## 扩展指南

若修改压缩算法、常数、填充或状态布局，优先修改 `sm3::update`、`sm3::pad`、`Reset`，并同步独立测试文件 `pkg/parser/auth/tidb_sm3_test.rs`；不得把 Rust 测试内嵌进生产源文件。至少保留 Go 的两个固定向量，并新增空输入、55/56/63/64/65 字节、多块输入以及不同 `Write` 切分方式的等价性测试。

若引入统一摘要 trait，应从 `NewSM3` 的返回类型及 `Write`/`Sum` 契约接入，同时检查 `caching_sha2.rs::hashCrypt` 的函数参数类型。尤其不能在没有迁移调用者和测试的情况下把 `Sum(input)` 改成标准“追加但不写入状态”语义；这会破坏已经记录的 Go 兼容行为。

若优化分配，可让 `Write` 先填满现有尾段、直接压缩输入中的完整块、只复制最终余数；验证必须证明 `length` 仍按全部输入累计、`digest` 只提交完整块、`unhandleMsg.len() < 64`。若改变 SQL 可见编码或 NULL 行为，应在 `pkg/expression/builtin_encryption.rs`、`builtin_encryption_vec.rs` 及其独立测试中处理，而不是把十六进制或 SQL 语义塞进本文件。

安全与兼容风险集中在字节序、模 2^32 加法、填充长度和 API 状态语义；性能风险集中在短块多次写入时的复制。任何行为变化都应同时与 `tidb_sm3.go` 逐项比较，并确认认证派生和标量/向量表达式两个上游表面。

## 验证依据

- Rust 实现：`pkg/parser/auth/tidb_sm3.rs` 的 `sm3`、辅助函数、`sm3::{pad, update, BlockSize, Size, Reset, Write, Sum}`、`NewSM3`、`Sm3Hash`。
- crate 边界：`pkg/parser/auth/Cargo.toml` 的 `[lib] path = "lib.rs"` 和依赖声明；`pkg/parser/auth/lib.rs` 的 `pub mod tidb_sm3`、兼容再导出及独立测试模块装配。
- Go 对照：`pkg/parser/auth/tidb_sm3.go` 的同名状态、压缩流程和公开入口；`pkg/parser/auth/tidb_sm3_test.go` 的固定向量与认证组合用例。
- Rust 测试：`pkg/parser/auth/tidb_sm3_test.rs::test_sm3`、`sum_with_input_returns_only_digest_and_updates_state`；`pkg/parser/auth/migration_aster_unit_test.rs::sm3_vectors_and_streaming_match_go` 与 `hashing_password_vectors_and_errors_match_go`。
- 上游源码：`pkg/parser/auth/caching_sha2.rs::{CheckHashingPassword, NewHashPassword}`，`pkg/expression/builtin_encryption.rs::sm3_hash`，`pkg/expression/builtin_encryption_vec.rs::sm3_vec`。
- RustCodeGraph：`status` 显示索引包含 11,467 个文件；`files --filter pkg/parser/auth` 列出 16 个 Go/Rust 文件；文件节点读取确认目标 18 个符号；`query Sm3Hash`/`query NewSM3` 区分 Go 与 Rust 定义；`callees Sm3Hash` 确认 Rust 边 `Sm3Hash -> Write/Sum/NewSM3`。调用者命令对同名定义未稳定产出，因此跨文件调用者另由限定引用搜索与源码节点交叉验证。
- 本任务为纯文档分析，按计划不运行 Cargo；交付前以任务规定命令验证文档存在且恰有十一个固定二级标题。
