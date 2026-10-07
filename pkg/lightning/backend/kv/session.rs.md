# [`pkg/lightning/backend/kv/session.rs`](session.rs)

## 文件定位

本文件位于 `astersql-lightning-backend-kv` crate 内；`pkg/lightning/backend/kv/Cargo.toml` 将该 crate 的入口指定为 `lib.rs`，后者通过 `mod session` 和 `pub use session::*` 把这里的类型与函数导出。它不是通用 TiDB Session 或真实 TiKV 事务实现，而是 Lightning SQL→KV 编码路径使用的轻量会话：把编码过程中产生的记录键、记录值和索引键暂存在内存，并向编码逻辑提供表达式上下文与表变更上下文。

上游入口主要有两个：`base.rs::NewBaseKVEncoder` 为写入路径创建 `Session`，`kv2sql.rs::NewTableKVDecoder` 为解码器创建相同的求值上下文。写入主链是 `BaseKVEncoder::AddRecord` → `Session::Txn` → `transaction::Set` → `MemBuf::Set`；一行完成后，`BaseKVEncoder::Record2KV` 调用 `Session::TakeKvPairs` 取走结果。`sql2kv.rs` 的 `Encoder::Close` 实现最终调用 `Session::Close`。

## 核心职责

1. `NewSession` 从 `encode::SessionOptions` 建立 `litExprContext` 和 `litTableMutateContext`，为生成列求值、SQL mode、时区和行编码选项提供会话级状态。
2. `transaction`、`kvUnionStore` 和 `invalidIterator` 提供经过裁剪的事务/联合存储外形；真正受支持的核心操作只有向 `MemBuf` 追加 KV。
3. `MemBuf` 管理当前连续字节缓冲、最多 20 个可复用缓冲的共享池、已生成的 `Pairs` 以及键值总字节数。
4. `Session::TakeKvPairs` 按行批次移交累计结果并重置计数，`Session::Close` 主动释放当前缓冲和池内缓冲。
5. `Session::SetUserVarVal` 与 `UnsetUserVar` 把用户变量生命周期转发给表达式上下文。

该文件明确不负责网络、持久化、MVCC、快照读取、提交或 2PC；这些能力在此轻量事务中要么返回固定值，要么报告不支持。

## 主要符号

- `maxAvailableBufSize: usize = 20`：可复用缓冲池的硬上限。池满时 `MemBuf::Recycle` 淘汰最早元素。
- `MIB`：1 MiB 的最小分配粒度，仅在模块内部使用。
- `invalidIterator`：`Valid` 恒为 `false`、`Close` 为空操作的范围迭代占位。
- `BytesBuf { buf, idx, cap }`：预分配的连续字节区。私有 `add` 复制输入并推进 `idx`；`destroy` 清空并收缩底层 `Vec`，同时把游标与容量归零。
- `newBytesBuf(size)`：创建长度和容量语义均为 `size` 的全零缓冲。
- `MemBuf { buf, availableBufs, kvPairs, size }`：追加式内存写缓冲。`availableBufs` 使用 `Arc<Mutex<Vec<BytesBuf>>>`；`kvPairs` 是对外交付的结果；`size` 是自上次提取以来键和值的累计字节数。
- `MemBuf::AllocateBuf`：寻找容量足够的池内缓冲，否则新建。目标容量为 `max(1 MiB, next_power_of_two(max(requested, 1)) * 2)`，乘法使用饱和运算。
- `MemBuf::Set`：确保剩余空间足够，必要时回收旧缓冲、分配新缓冲，然后把键和值分别复制进缓冲并将独立的 `KvPair` 加入结果。
- `kvUnionStore`：只实际保存一个 `MemBuf`；索引名和表信息缓存接口均为占位。
- `transaction`：只支持 `Set` 和取得 `MemBuf` 的轻量事务。`Len` 返回 KV 条数，不是字节数。
- `Session { txn, exprCtx, tblCtx }`：编码会话的所有者。三个字段均为私有，只通过访问方法暴露。
- `NewSession(options)`：公开构造入口，过滤系统变量后依次调用 `newLitExprContext`、`newLitTableMutateContext`；任一上下文创建失败即返回错误。
- `Session::TakeKvPairs`：用同容量空 `Vec` 替换当前 `Pairs`，把 `size` 清零并返回旧结果。
- `Session::Close`：销毁当前缓冲，并在持锁期间排空、销毁池内全部缓冲。

## 执行流程

典型的一行编码流程如下。

1. `NewBaseKVEncoder` 把 `EncodingConfig::SessionOptions` 交给 `NewSession`。后者只保留白名单 `KNOWN` 中的系统变量，先建立 `litExprContext`，再基于它建立 `litTableMutateContext`。
2. `BaseKVEncoder::AddRecord` 编码记录键和值，并经 `Session::Txn().Set(...)` 写入；每个索引键也沿相同路径写入。
3. `transaction::Set` 转发到 `MemBuf::Set`。若没有当前缓冲，或 `cap - idx` 小于本次键值总长度，旧缓冲先进入复用池，再按分配公式取得足够大的缓冲。
4. `BytesBuf::add` 依次拷贝 key 和 value；随后构造的 `verification::KvPair` 持有这两个片段的 `Vec<u8>` 副本。`MemBuf::size` 增加 `key.len() + value.len()`。
5. `BaseKVEncoder::Record2KV` 在一行及其索引全部完成后调用 `TakeKvPairs`。旧 `Pairs` 返回给调用者，新 `Pairs` 复用原向量容量；累计字节数归零，但当前 `BytesBuf` 及缓冲池仍保留供后续行使用。
6. 编码器关闭时，`tableKVEncoder::Close` 调用 `Session::Close`，释放当前缓冲和池中缓冲，并把编码器标为已关闭。

解码入口 `NewTableKVDecoder` 也调用 `NewSession`，但主要利用其中的表达式/表上下文，而不是通过本文件提交事务。

## 数据与状态

`BytesBuf` 的不变量是 `0 <= idx <= cap`，并且正常分配后 `cap == buf.len()`。`add` 本身不做容量检查，安全性依赖调用方 `MemBuf::Set` 预先确认当前剩余空间至少等于 key 与 value 的总长度。回收时 `idx` 重置为 0，`cap` 恢复为底层 `Vec` 长度。

缓冲池以 `Vec` 保存：`AllocateBuf` 选择第一个 `cap >= 目标容量` 的元素，通过交换到下标 0 后删除；`Recycle` 在达到 20 项时删除并销毁下标 0，再把回收项追加到末尾。因此它是有界复用池，但不是按大小排序的最佳适配器。

`MemBuf::kvPairs` 与 `size` 描述当前尚未被 `TakeKvPairs` 取走的批次。`Len` 是 KV 对数量，`Size` 是所有键和值的字节和；两者在 `TakeKvPairs` 后分别随新空列表和 `size = 0` 重置。当前 Rust 实现的 `KvPair` 拥有复制后的 key/value，因此返回的 `Pairs` 不借用 `BytesBuf`，缓冲可独立复用。

`Session` 内的 `exprCtx` 保存 SQL mode 派生的错误等级、时间戳、时区、受支持的系统变量和用户变量；`tblCtx` 保存行编码、校验和及 mutation 相关配置。其具体解析和约束在直接依赖 `context.rs::newLitExprContext` 与 `newLitTableMutateContext` 中实现。

## 依赖与调用关系

crate 边界由 `pkg/lightning/backend/kv/Cargo.toml` 定义。本文件直接使用：

- `encode::{Datum, SessionOptions}`：会话输入与用户变量值类型；对应 path 依赖 `../encode`。
- `verification::KvPair`：编码结果中的单个 KV；对应 path 依赖 `../../verification`。
- crate 内 `Pairs`、`litExprContext`、`litTableMutateContext` 及两个上下文构造函数；`Pairs` 由同 crate 的 SQL/KV 接线使用。
- 标准库 `HashMap`、`Arc`、`Mutex`：系统变量过滤与缓冲池同步。

RustCodeGraph 对目标文件报告 58 个符号，并显示其被 `base.rs`、`canonical.rs`、`kv2sql.rs`、`session_internal_test.rs` 等 8 个文件使用。精确调用证据包括：`base.rs::NewBaseKVEncoder` → `NewSession`，`kv2sql.rs::NewTableKVDecoder` → `NewSession`，`base.rs::BaseKVEncoder::Record2KV` → `TakeKvPairs`，`base.rs::BaseKVEncoder::AddRecord` → `transaction::Set`，以及 `sql2kv.rs::Encoder::Close` → `Session::Close`。

## 错误处理与边界

- `NewSession` 仅把 `KNOWN` 白名单内的系统变量传给上下文构造器；未知或未列入的变量被静默忽略。白名单内的非法数值、时区或模式由 `context.rs` 返回字符串错误，并经 `?` 传播。
- `MemBuf::Set` 的签名可返回错误，但当前路径在成功分配后只追加数据，正常逻辑恒为 `Ok(())`。`Mutex::lock().unwrap()` 表示锁中毒会 panic，而不是转成 `Result`。
- 极端 `requested` 会经过 `next_power_of_two` 和饱和乘法；实际 `Vec` 分配仍可能因平台容量限制或内存耗尽而失败，未转换为业务错误。
- `Delete` 明确返回 `"unsupported operation"`；`GetFlags` 与 `transaction::Get` 返回 `"key not exist"`；`Iter` 返回永远无效的迭代器。
- `kvUnionStore::GetIndexName` 会 panic；缓存索引名、缓存表信息、staging、flags 更新、`Discard`、`Reset` 等方法是空实现。调用方不得把这些接口的存在理解为对应语义已实现。
- `Flush` 固定返回 0，`MayFlush` 固定成功，`IsPipelined` 固定为 `false`，`GetTableInfo` 固定为 `None`。本文件没有持久化或读取已写数据的事务语义；即使先 `Set`，`transaction::Get` 仍返回不存在。只有 `MemBuf::GetLocal` 会逆序扫描当前 `kvPairs`，体现同 key 后写覆盖先写的本地查询语义。

## 并发与资源生命周期

只有复用池 `availableBufs` 通过 `Arc<Mutex<_>>` 保护；`MemBuf::buf`、`kvPairs`、`size` 以及 `Session` 上的可变入口依赖 `&mut self` 串行访问。代码没有启动线程、异步任务或通道，也没有让完整 `Session` 成为并发事务对象。

分配生命周期为“当前缓冲 → 空间不足时回收池 → 后续分配复用 → `Close` 销毁”。池最大 20 项，限制了闲置缓冲数量，但不限制单个缓冲容量；一次很大的键值可能留下同样大的可复用缓冲，直至被淘汰或关闭。`TakeKvPairs` 只转移结果并复用 `Pairs` 向量容量，不关闭会话，也不清空当前字节缓冲。

`Close` 没有实现 `Drop`，必须由上层显式调用。由于 Rust 的 `Vec` 在对象析构时也会释放内存，遗漏 `Close` 不会造成永久泄漏，但会推迟大容量缓冲的释放；显式 `destroy` 还会立即 `shrink_to_fit`。`Close` 排空池，因此重复调用在当前实现下是安全的空操作。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/lightning/backend/kv/session.go`，独立 Go 测试是 `session_internal_test.go`。Rust 保留了 Go 的主要结构和命名：`invalidIterator`、`BytesBuf`、`MemBuf`、`kvUnionStore`、`transaction`、`Session`，以及 1 MiB 起步、两倍 2 次幂容量、20 项池上限、只追加事务和按行 `TakeKvPairs` 的总体流程。

需要注意以下当前差异：

- Go 的 `BytesBuf::add` 返回底层手工分配缓冲的切片，`Pairs` 通过 `BytesBuf`/`MemBuf` 维持并回收该内存；Rust 的 `add` 返回 `to_vec()` 副本，`KvPair` 与缓冲生命周期解耦。因此 Rust 保留了分配池策略，但产生额外复制，资源归属与 Go 不完全相同。
- Go `NewSession` 借助完整系统变量注册表过滤未知、只读及非法变量并记录日志；Rust 使用固定 `KNOWN` 白名单且没有 logger。比如 Rust 测试中的 `lc_time_names` 因不在白名单而被忽略；不能据此推断所有 Go 可写系统变量都已移植。
- Go 通过嵌入 TiDB 的 `kv.Transaction`/`kv.MemBuffer` 接口满足更宽的类型契约；Rust 使用固有方法模拟所需子集，没有对应 trait 实现。空操作与固定错误是有意裁剪边界。
- Go `GetLocal` 委托通用 `kv.GetValue`；Rust 直接逆序扫描当前列表。对同 key 的追加结果保持后写优先，但复杂 flags/staging 语义没有移植。
- Go 通过 `manual.New`/`manual.Free` 管理缓冲；Rust 使用 `Vec<u8>`，`destroy` 明确清空收缩，最终释放仍受 Rust 所有权保证。

Rust 的 `session_internal_test.rs` 基本对应 Go `session_internal_test.go` 的缓冲容量序列、池上限及会话状态检查；额外的 `session_test.rs::transaction_get_remains_unsupported_after_set` 固化了裁剪事务“不因 Set 而支持 Get”的边界。

## 扩展指南

- 新增系统变量支持时，先确认 `context.rs` 是否实际消费并验证该变量，再同步 `NewSession::KNOWN`；同时在 `session_internal_test.rs` 增加有效、无效和应忽略值的案例，并核对 Go 注册表语义。仅扩白名单而不实现解析不会带来可观察行为。
- 修改缓冲增长或回收策略时，集中调整 `AllocateBuf`/`Recycle`，保持 `idx <= cap` 和池上限不变量；同步两项分配回收测试，并评估大行内存峰值、额外复制和池中大缓冲驻留风险。
- 若要消除 Rust 的 key/value 复制，必须重新设计 `Pairs` 与 `BytesBuf` 的所有权关系，不能只让 `add` 返回借用切片；需覆盖 `TakeKvPairs` 后继续编码、回收缓冲和关闭会话时旧结果仍有效的生命周期测试。
- 若新增读取、删除、flags、staging、表/索引缓存或提交语义，应在 `transaction`/`kvUnionStore`/`MemBuf` 对应占位符处实现，并放在独立测试文件中；不要用 `Flush = 0` 或空方法冒充成功实现。还要检查 `pkg/kv` 的真实契约，而不是只对齐方法名。
- 会话级上下文行为应优先在 `context.rs` 扩展；本文件只负责构造、持有和转发。新增用户变量行为时同步检查名称小写规范化。
- 关闭流程的扩展应经 `sql2kv.rs::Encoder::Close` 接线，并验证错误路径或重复关闭；当前没有 `Drop` 自动清理协议。

相关 Rust 测试必须继续独立保存在 `session_internal_test.rs`、`session_test.rs` 等测试文件中，不应内嵌到生产源文件。

## 验证依据

- RustCodeGraph 索引状态：11467 个文件、307296 个节点、1848419 条边；`files --filter pkg/lightning/backend/kv` 确认目标 crate 的 Rust/Go 对照文件均已索引。
- RustCodeGraph 源码与结构查询：`node --file pkg/lightning/backend/kv/session.rs --offset 1 --limit 500`（完整 349 行、58 个符号、8 个使用文件）；另读取 `base.rs` 155–274、`kv2sql.rs` 320–362、`sql2kv.rs` 100–159、`context.rs` 1–230 的索引片段以核对上下游调用和上下文语义。
- RustCodeGraph 调用结果明确列出：`NewSession` 的 Rust 调用者包括 `NewBaseKVEncoder`、`NewTableKVDecoder` 和内部测试；`AllocateBuf` 的调用者包括 `MemBuf::Set` 及两项分配测试；`TakeKvPairs` 的调用者包括自身实现与 `TestSessionInternalState`。由于常见符号名会混入其他模块，本说明仅采纳带目标路径的边。
- crate/模块证据：`pkg/lightning/backend/kv/Cargo.toml`、`pkg/lightning/backend/kv/lib.rs`。
- Go 对照证据：`pkg/lightning/backend/kv/session.go`、`pkg/lightning/backend/kv/session_internal_test.go`。
- Rust 测试证据：`pkg/lightning/backend/kv/session_internal_test.rs`、`pkg/lightning/backend/kv/session_test.rs`。
- 人工核对结论：该文件存在是为了以最小 Session/事务表面支撑 Lightning 编解码；运行时只把编码结果累计在内存并显式移交；安全扩展的关键是保持缓冲所有权、裁剪接口边界和上下文的 Go 兼容性。
