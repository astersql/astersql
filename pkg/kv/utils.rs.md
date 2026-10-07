# `pkg/kv/utils.rs`

## 文件定位

本文件属于 `astersql-kv` crate。其 crate 边界由 [`pkg/kv/Cargo.toml`](Cargo.toml) 定义，库入口是 [`pkg/kv/lib.rs`](lib.rs)；入口用 `#[path = "utils.rs"] mod utils_impl` 装入本文件，再通过 `pub use utils_impl::*` 把五个函数从 crate 根公开。因此，上层可以直接调用 `astersql_kv::IncInt64`、`GetInt64`、`WalkMemBuffer`、`IsUserKS` 和 `IsSystemKS`，而不需要知道私有模块名 `utils_impl`。

它位于 KV 抽象层的便捷操作面：前三个函数建立在 [`pkg/kv/kv.rs`](kv.rs) 的 `Retriever`、`Mutator`、`RetrieverMutator` 和 `Iterator` trait 之上，用统一接口处理十进制整数值和缓冲区遍历；后两个函数建立在 `Storage::GetKeyspace`、`kerneltype` 与 `keyspace` 上，为 SQL/server 生命周期代码区分 classic、nextgen 用户 keyspace 和 SYSTEM keyspace。本文件没有条件编译项，但 [`pkg/kv/Cargo.toml`](Cargo.toml) 的 `nextgen` feature 会同时启用 `kerneltype/nextgen` 与 `keyspace/nextgen`，从而改变 keyspace 判定结果。

## 核心职责

- `IncInt64`：读取某个 key 的字节值，按 UTF-8 十进制有符号 64 位整数解析，加上 `step` 后以十进制字节写回；键不存在时直接写入 `step`。
- `GetInt64`：读取并解析由上述约定保存的整数；键不存在返回 `0`，使调用者无需把“尚未初始化”当作错误处理。
- `WalkMemBuffer`：从空 key 开始、无上界地正向遍历 `Retriever`，逐项调用可失败回调，并保证取得迭代器后在所有正常/错误退出路径上调用 `Close`。
- `IsUserKS` / `IsSystemKS`：只在 nextgen 内核下按 `Storage::GetKeyspace` 区分非 SYSTEM 与 SYSTEM；classic 模式下两者都返回 `false`。
- 保持与 [`pkg/kv/utils.go`](utils.go) 的接口意图和边界语义一致，同时适配 Rust 的 trait object、`Result`、所有权和显式溢出行为。

## 主要符号

- `pub fn IncInt64(rm: &mut dyn RetrieverMutator, key: &Key, step: i64) -> Result<i64, Error>`：需要可变的读写组合接口。它用 `Context::todo()` 发起 `Get`，通过 `IsErrNotFound` 单独识别缺失键；已有值经 `std::str::from_utf8` 和 `parse::<i64>()` 两阶段解析，随后用 `wrapping_add` 加法并调用 `Set`。返回值是成功写回后的整数。
- `pub fn GetInt64(ctx: &Context, retriever: &dyn Retriever, key: &Key) -> Result<i64, Error>`：保留调用者传入的上下文，只要求只读 `Retriever`。缺失键返回 `Ok(0)`，其他读取错误原样传播，编码或整数格式错误转换为 crate 的共享错误。
- `pub fn WalkMemBuffer<F>(mem_buf: &dyn Retriever, callback: F) -> Result<(), Error>`，其中 `F: FnMut(&Key, &[u8]) -> Result<(), Error>`：允许回调携带并更新自身状态。迭代器由 `Iter(Key::default(), None)` 创建；每轮依次取 `Key`、`Value`，调用回调，再推进 `Next`。
- `pub fn IsUserKS(store: &dyn Storage) -> bool`：`kerneltype::IsNextGen() && store.GetKeyspace() != keyspace::System`。
- `pub fn IsSystemKS(store: &dyn Storage) -> bool`：`kerneltype::IsNextGen() && store.GetKeyspace() == keyspace::System`。

本文件没有模块级常量、类型、trait、`impl` 或 `cfg` 条目；所有行为都集中在这五个公开函数中。

## 执行流程

`IncInt64` 的流程如下：

1. 克隆 `key`，以占位上下文和空 `GetOption` 切片调用 `RetrieverMutator::Get`。
2. 若成功，取 `ValueEntry.Value`；若错误是 `ErrNotExist` 类错误，则把 `step.to_string()` 写入并直接返回 `step`；其他读取错误直接返回。
3. 对已有字节先做 UTF-8 校验，再按十进制解析为 `i64`。任一步失败都停止，且不会执行写回。
4. 用 `wrapping_add(step)` 计算新值，把新值格式化为十进制字节并调用 `Set`；写入成功才返回新值。

`GetInt64` 使用相同的读取与解析约定，但不写入：缺失键走零值分支，其他错误传播，已有值解析成功后返回整数。

`WalkMemBuffer` 先创建全范围迭代器。成功创建后，它在一个内部闭包中循环：仅当 `Valid()` 为真时获取当前键和值；回调成功后才执行 `Next()`。回调或 `Next` 的第一个错误成为循环结果。闭包结束后无条件调用一次 `Close()`，最后返回之前保存的结果；因此清理不会覆盖真正的遍历错误。创建迭代器本身失败时函数立即返回，此时没有资源需要关闭。

keyspace 函数利用 `&&` 短路：classic 模式下不会调用 `GetKeyspace`，直接返回 `false`；nextgen 模式下才比较存储报告的 keyspace 字符串与 `keyspace::System`。

## 数据与状态

整数以 `ValueEntry.Value` 中的 ASCII/UTF-8 十进制文本保存，而不是定长二进制。负值和负步长由 `i64` 解析及格式化自然支持；前导正负号等具体接受范围由 Rust `str::parse::<i64>()` 决定。缺失键在读路径上等价于数值零，但 `GetInt64` 不创建该键；`IncInt64` 对缺失键写入 `step`，而不是先物化零再做第二次读写。

`IncInt64` 的 `wrapping_add` 明确规定越过 `i64::MAX`/`i64::MIN` 时按二补码回绕，不 panic。现有 Rust/Go 测试只直接覆盖跨越 `u32::MAX` 后仍可继续增长；真正的 `i64` 极值回绕由实现明确给出，但当前独立测试没有覆盖。

`WalkMemBuffer` 本身不保存集合。每轮从迭代器取得拥有所有权的 `Key` 和 `Vec<u8>`，再把临时借用传给回调；回调若要在本轮之后保留数据，必须像 [`pkg/kv/mpp_2_aster_unit_test.rs`](mpp_2_aster_unit_test.rs) 那样克隆 key/value。遍历顺序、快照一致性及遍历期间可否修改底层存储由具体 `Retriever`/`Iterator` 实现决定，本文件不额外保证。

## 依赖与调用关系

直接下游依赖均来自 crate 根或 [`pkg/kv/kv.rs`](kv.rs)：`Context`、`Error`、`IsErrNotFound`、`Key`、`Retriever`、`RetrieverMutator`、`Storage`，以及错误构造模块 `errors`。`IsUserKS`/`IsSystemKS` 还依赖 Cargo 中声明的本地包 `kerneltype` 和 `keyspace`。没有网络、磁盘或 TiKV RPC 直接调用；实际读写和迭代由传入 trait object 的后端实现完成。

已核实的 Rust 上游调用包括：

- [`pkg/structure/string.rs`](../structure/string.rs) 的 `TxStructure::Inc` 编码结构化字符串 key 后调用 `kv::IncInt64`。
- [`pkg/session/runtime/crossks_session_pool.rs`](../session/runtime/crossks_session_pool.rs)、[`pkg/session/runtime/system_session.rs`](../session/runtime/system_session.rs) 和 [`pkg/session/runtime/session.rs`](../session/runtime/session.rs) 用 `IncInt64` 分配连续的全局 ID；调用者根据返回的最后一个 ID 构造区间。
- [`pkg/meta/reader.rs`](../meta/reader.rs) 与 [`pkg/ddl/persistent_masking_actions.rs`](../ddl/persistent_masking_actions.rs) 也通过 crate 根调用 `IncInt64`，把该工具用于元数据计数/标识更新。
- [`pkg/session/runtime/session.rs`](../session/runtime/session.rs) 在用户 keyspace 启动路径中调用 `IsUserKS`，决定是否等待 SYSTEM store 的 bootstrap 版本；[`cmd/tidb-server/main.rs`](../../cmd/tidb-server/main.rs) 在关闭流程中用它决定是否额外关闭 SYSTEM storage。
- 当前生产 Rust 检索未发现 `WalkMemBuffer` 的跨文件生产调用；其直接调用证据来自独立迁移回归测试 [`pkg/kv/mpp_2_aster_unit_test.rs`](mpp_2_aster_unit_test.rs)。这说明 API 已公开且行为已测试，不能据此推断生产路径一定使用它。

RustCodeGraph 精确节点确认五个函数均定义于本文件。调用图对常见名称 `GetInt64` 会混入大量行/Datum 同名方法，且部分精确 callers/callees 查询长时间无输出；因此上述跨文件边又用限定 Rust 源码检索与调用点上下文核实，没有把不完整图结果解释为“无调用者”。

## 错误处理与边界

`IncInt64` 和 `GetInt64` 只把 `IsErrNotFound` 识别的错误转换为正常值；权限、取消、后端、锁冲突等其他读取错误保持原错误返回。无效 UTF-8 与超出 `i64` 范围或非十进制整数的文本，经 `errors::New(error.to_string())` 转换后返回。该转换保留可读错误文本，但不保留原标准库错误类型供下游向下转型。

`IncInt64` 在解析失败前不会写入；如果最终 `Set` 失败，则错误返回，不能声称值已更新。它是 `Get` 后 `Set` 的复合操作，不提供比较交换或内部锁，函数自身不保证多个并发调用者之间的原子增量。要求唯一 ID 或计数正确性的调用者必须依赖事务隔离、预先加锁和冲突重试；例如 cross-keyspace 分配路径在调用前显式锁住 global ID key。

`WalkMemBuffer` 的错误优先级是：`Iter` 创建错误立即返回；之后回调错误或 `Next` 错误返回第一个遇到的错误。当前 Rust `Iterator::Close` 签名返回 `()`，所以不存在可传播的关闭错误；这一点与 Go 版本用 `defer iter.Close()` 且不检查返回值的可观察行为一致。回调报错后不会执行该轮的 `Next`，但仍会关闭迭代器。

keyspace 判定只做内核模式与字符串相等比较，不校验空 keyspace 或未知名称。在 nextgen 中，任何不等于 `keyspace::System` 的值（包括空字符串）都会被 `IsUserKS` 视为用户 keyspace；这是当前表达式的事实，新增存储实现必须保证 `GetKeyspace` 的返回契约正确。

## 并发与资源生命周期

本文件没有全局可变状态、锁、原子变量、线程、异步任务或通道。`GetInt64` 和两个 keyspace 判定只借用输入；`IncInt64` 通过 `&mut dyn RetrieverMutator` 在单次调用期间取得独占 Rust 借用，但这只阻止同一安全 Rust 借用域内的并发访问，不把后端的跨事务读改写变成原子操作。

`WalkMemBuffer` 独占拥有返回的 `Box<dyn Iterator>`，并在取得迭代器后的所有闭包退出路径上调用一次 `Close`。回调是 `FnMut`，可以维护局部累积状态；其借用只持续到函数返回。迭代器中潜在的游标、快照或缓冲资源由具体实现和 `Close` 释放，本文件不控制其内部生命周期。

整数写入的事务提交、回滚、锁释放和重试均在调用者或存储实现中完成。`IncInt64` 只修改传入的 `RetrieverMutator` 当前视图，不提交事务，也不确认值已经持久化。

## 与 Go 版本的对应关系

直接对照为 [`pkg/kv/utils.go`](utils.go)，测试对照为 [`pkg/kv/utils_test.go`](utils_test.go)。五个 Rust 函数逐一保留 Go 的名称与职责：缺失整数键的初始化/零值语义、十进制文本编码、全范围正向遍历、回调/推进错误短路，以及 nextgen SYSTEM/用户 keyspace 判定表达式均一致。

语言层差异如下：

- Go 的 `RetrieverMutator` 接口按值传递，Rust 用 `&mut dyn RetrieverMutator` 表达读后写；Go `context.TODO()` 对应 Rust `Context::todo()`，Go 可变 `GetOption` 在这里是空切片 `&[]`。
- Go `strconv.ParseInt(..., 10, 64)` 同时处理字节到字符串和整数解析；Rust 显式分为 UTF-8 校验与 `parse::<i64>()`。两者对普通十进制持久值一致，但错误类型和某些格式细节不应在未测试时假定完全同构。
- Go 的有符号整数溢出结果按二补码实现语义运行；Rust 调试构建普通 `+` 可能 panic，因此源码显式采用 `wrapping_add` 固定回绕行为。
- Go 的 `Iter(nil, nil)` 用 nil 表达无界；Rust 接口要求起点 `Key`，因此用 `Key::default()`（空字节 key）和 `None` 上界表达同一全范围意图。
- Go `defer iter.Close()` 对应 Rust 先保存循环结果、再显式 `Close`。当前 Rust trait 的 `Close` 无返回值，注释所说“忽略 Close 错误”是对 Go 语义的说明，不代表 Rust 侧存在可检查的关闭错误。

[`pkg/kv/utils_test.rs`](utils_test.rs) 覆盖缺失初始化、连续增量、非法整数、跨 `u32::MAX`、读回值以及 classic/nextgen 的用户/SYSTEM 判定；[`pkg/kv/mpp_2_aster_unit_test.rs`](mpp_2_aster_unit_test.rs) 额外覆盖 `WalkMemBuffer` 遍历两个条目。Go 测试覆盖整数与 keyspace 分支，但其本地 `mockMap::Iter` 返回 nil，因此没有直接测试 `WalkMemBuffer`。

## 扩展指南

修改整数格式或错误语义时，应同时审查 `IncInt64` 与 `GetInt64`，确保写入格式必定能被读取，并同步对照 [`pkg/kv/utils.go`](utils.go)。新增边界测试必须放在独立的 [`pkg/kv/utils_test.rs`](utils_test.rs) 或其他独立 `*_test.rs` 文件，不要内嵌进生产源码；建议补充 `i64::MAX + 1`、`i64::MIN - 1`、负步长、无效 UTF-8、`Get`/`Set` 注入错误，以及写失败不返回成功值。

若要把 `IncInt64` 用于并发计数，不能只修改算术表达式；应先确定后端事务锁与重试协议，必要时在更高层为 key 加锁，或引入后端原子接口。改变为饱和/检查溢出会影响 Go 对齐和现有持久值语义，必须同步实现与兼容性测试。

扩展 `WalkMemBuffer` 时应保持“取得迭代器后必定关闭”的不变量，并为 `Iter`、回调、`Next` 的失败分别添加可观察清理测试。如果要允许回调修改同一 mem buffer，必须先核对具体迭代器的失效与锁规则，不能从 `Retriever` trait 推断安全性。改变起止边界时应优先新增独立函数或显式参数，避免悄然改变当前全范围 API。

新增 keyspace 类别时，需要同步检查 `kerneltype`、`keyspace::System`、`Storage::GetKeyspace` 以及所有 `IsUserKS`/`IsSystemKS` 调用者。当前“非 SYSTEM 即用户”的二分法可能无法表达第三类保留空间。这里的主要正确性风险是误分类后走错 bootstrap、系统会话池或关闭流程；兼容性风险是改变 classic 下恒 false 的约定；性能风险总体较低，但 `IncInt64` 每次都进行分配、文本解析与格式化，热路径改动应避免额外往返和复制。

## 验证依据

本说明使用了以下直接证据：

- RustCodeGraph `status`：本地索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；任务无前置依赖，目标文件已被索引。
- RustCodeGraph `query`：分别定位 `IncInt64`、`GetInt64`、`WalkMemBuffer`、`IsUserKS`、`IsSystemKS` 的 Rust 与 Go 定义；精确 `node pkg/kv/utils.rs::<符号>` 核对了五个函数的签名和源码。`node` 还报告 `WalkMemBuffer` 被 `mpp_2_integer_helpers_and_walk_match_go` 调用。部分精确 callers/callees 查询长时间无输出，未用其缺失结果推断调用关系。
- 目标源码：[`pkg/kv/utils.rs`](utils.rs)，核对全部五个公开函数，确认没有其他常量、类型、实现或条件编译项。
- crate 与接口边界：[`pkg/kv/Cargo.toml`](Cargo.toml)、[`pkg/kv/lib.rs`](lib.rs)、[`pkg/kv/kv.rs`](kv.rs)、[`pkg/kv/error.rs`](error.rs) 和 [`pkg/kv/key.rs`](key.rs)。
- Go 对照：[`pkg/kv/utils.go`](utils.go)；独立 Go 测试：[`pkg/kv/utils_test.go`](utils_test.go)。
- 独立 Rust 测试：[`pkg/kv/utils_test.rs`](utils_test.rs) 与 [`pkg/kv/mpp_2_aster_unit_test.rs`](mpp_2_aster_unit_test.rs)。
- 上游调用点：[`pkg/structure/string.rs`](../structure/string.rs)、[`pkg/session/runtime/crossks_session_pool.rs`](../session/runtime/crossks_session_pool.rs)、[`pkg/session/runtime/system_session.rs`](../session/runtime/system_session.rs)、[`pkg/session/runtime/session.rs`](../session/runtime/session.rs)、[`pkg/meta/reader.rs`](../meta/reader.rs)、[`pkg/ddl/persistent_masking_actions.rs`](../ddl/persistent_masking_actions.rs) 和 [`cmd/tidb-server/main.rs`](../../cmd/tidb-server/main.rs)。

本任务只新增说明文档，按计划不运行 Cargo。交付前使用任务指定命令验证文档存在且恰有 11 个固定二级标题，并人工复核以上结论均可回溯到所列符号、接口、调用点或测试。
