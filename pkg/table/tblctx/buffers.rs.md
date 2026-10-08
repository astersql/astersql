# `pkg/table/tblctx/buffers.rs`

## 文件定位

本文件属于 Cargo crate `astersql-table-tblctx`（`pkg/table/tblctx/Cargo.toml`），由 `pkg/table/tblctx/lib.rs` 的 `mod buffers; pub use buffers::*;` 纳入并公开。它位于表数据变更上下文层：上接 `MutateContext::GetMutateBuffers`，下接 `tablecodec` 行编码、`kv::MemBuffer` 写入和会话级 `variable::WriteStmtBufs`。Rust 会话适配器 `pkg/table/tblsession/table.rs` 中的 `NewMutateContext` 从 session 取出 `WriteStmtBufs` 后调用 `tblctx::NewMutateBuffers`，因此缓冲对象的生命周期绑定于该表突变上下文。

Go 对照实现是 `pkg/table/tblctx/buffers.go`。Go 的完整 DML 消费位置可见 `pkg/table/tables/tables.go` 的 `TableCommon.updateRecord`、`TableCommon.addRecord` 和 `addIndices`；当前 Rust 搜索只确认了构造、trait 暴露及测试中的直接调用，未找到 Rust 生产代码直接调用 `WriteMemBufferEncoded`、`EncodeBinlogRowData` 或 `GetRowToCheck`。因此，本文件已经提供缓冲与编码能力，但不能据此断言 Rust 的完整 AddRecord/UpdateRecord 主链已经接通。

## 核心职责

- `EncodeRowBuffer` 成对累积列 ID 与 `Datum`，复用 `WriteStmtBufs.RowValBuf` 完成新/旧行格式编码，并选择 `MemBuffer::Set` 或 `SetWithFlags` 写入事务内存缓冲。
- `CheckRowBuffer` 累积约束检查所需的列值，并生成拥有自身 chunk 底层存储的 `CheckedRow`，避免返回悬垂的临时行视图。
- `MutateBuffers` 聚合编码缓冲、检查缓冲和共享的语句级缓冲；每次 getter 调用先清空逻辑长度、按请求容量扩容，但保留已分配容量以服务后续行。
- `ensureCapacityAndReset` 模拟 Go `make`/切片重切片的容量规则：容量不足时重新分配，否则复用现有 allocation 并调整长度。

该文件只管理暂存、编码和写入边界，不负责选择待编码列、执行 SQL 约束本身、生成 record key、开启/提交事务或管理 staging；这些职责在上层表变更代码中完成。

## 主要符号

- `EncodeRowBuffer { colIDs, row, writeStmtBufs }`：`colIDs[i]` 与 `row[i]` 必须表示同一列。`Reset(capacity)` 清零两者长度并保证最低容量；`AddColVal(colID, val)` 同步追加一对值。
- `EncodeRowBuffer::WriteMemBufferEncoded(cfg, loc, ec, memBuffer, key, handle, flags)`：按 `RowEncodingConfig` 可选创建 `rowcodec::RawChecksum`，取出并复用 `RowValBuf`，调用 `tablecodec::EncodeRow`，成功后缓存编码结果并写入 `MemBuffer`。
- `EncodeRowBuffer::EncodeBinlogRowData(loc, ec)`：调用 `tablecodec::EncodeOldRow`，从空字节缓冲开始生成独立结果，不修改 `RowValBuf`。
- `handleEncodingError(ec, message)`：把底层编码错误文本包装成共享错误，再交给 `errctx::Context::HandleError`；若上下文吞掉错误，则仍回退返回原错误。
- `CheckRowBuffer { rowToCheck }`：以 `AddColVal` 累积待检查值，以 `Reset` 清空并预留容量。
- `CheckedRow { inner }`：拥有 `chunk::mutrow::MutRow`，当前只公开 `Len` 和 `GetInt64` 两个读取接口。
- `CheckRowBuffer::GetRowToCheck()`：克隆 `rowToCheck` 并通过 `MutRowFromDatums` 构建 `CheckedRow`。
- `MutateBuffers { stmtBufs, encodeRow, checkRow }`：以 `Rc<RefCell<WriteStmtBufs>>` 让聚合器和编码缓冲共享同一个会话缓冲。
- `NewMutateBuffers(stmtBufs)`：执行 `intest::AssertNotNil`，把传入值装入 `Rc<RefCell<_>>`，初始化两个可复用子缓冲。
- `GetEncodeRowBufferWithCap` / `GetCheckRowBufferWithCap`：先 `Reset` 再返回同一内部实例的可变引用；调用方不得期待旧内容保留。
- `GetWriteStmtBufs()`：返回动态可变借用 `RefMut<WriteStmtBufs>`，借用规则由 `RefCell` 在运行时检查。
- `ensureCapacityAndReset<T: Default>(slice, size, optCap)`：公开的泛型容量辅助函数；新增元素以 `T::default()` 填充。

文件没有模块级常量、trait、枚举、条件编译项或异步函数。

## 执行流程

编码写入流程如下：

1. 上层通过 `GetEncodeRowBufferWithCap` 取得内部唯一编码缓冲；该调用清空旧列并按本行列数预留容量。
2. 上层逐列调用 `AddColVal`，保持列 ID 与值的顺序和数量一致。
3. `WriteMemBufferEncoded` 在启用行级校验和时把传入 `handle` 放入 `RawChecksum`；未启用时不使用 handle。
4. 函数可变借用共享 `WriteStmtBufs`，把 `AddRowValues` 的长度调整为 `row.len() * 2`。当前该字段是 `Vec<String>`，只保留与 Go 可见的长度/容量约定；真正的旧格式 `Datum` 暂存由 `tablecodec::EncodeRow` 在传入 `None` 后自行分配。
5. 函数用 `mem::take` 暂时移出 `RowValBuf`，克隆当前 `row` 和 `colIDs`，调用 `tablecodec::EncodeRow`。`RowEncoder.Enable` 为真时走新行格式，否则由 `tablecodec` 回退到 `EncodeOldRow`。
6. 编码成功后把结果的 clone 存回 `RowValBuf`，释放 `RefCell` 借用；随后空 flags 调 `MemBuffer::Set`，非空 flags 调 `SetWithFlags`。最终返回的是内存缓冲写入结果。

约束检查流程较短：通过 `GetCheckRowBufferWithCap` 清空缓冲，逐列 `AddColVal`，再调用 `GetRowToCheck` 得到独立拥有 chunk 的 `CheckedRow`。Go 主链在 `TableCommon.updateRecord` 中把该 row 交给约束检查；当前 Rust 生产调用未在仓库搜索中确认。

binlog 流程独立调用 `EncodeBinlogRowData`：它强制使用旧行格式、从空缓冲编码，因此返回值可缓存或修改而不应与 `RowValBuf`/`IndexKeyBuf` 共享底层字节存储；Rust 和 Go 测试均验证了这一点。

## 数据与状态

`EncodeRowBuffer` 有三个关键不变量：`colIDs.len() == row.len()`；二者索引一一对应；`writeStmtBufs` 必须与所属 `MutateBuffers.stmtBufs` 指向同一 `Rc`。前两个不变量由正确配对调用 `AddColVal` 维持，类型系统并未阻止调用方直接修改公开字段；若数量不等，`tablecodec::EncodeRow`/`EncodeOldRow` 会返回计数不匹配错误。

`Reset(capacity)` 只清空逻辑内容，不保证最终 capacity 精确等于参数：已有容量足够时继续保留更大的 allocation；不足时新建至少为指定 capacity 的 vector。`ensureCapacityAndReset` 的 `size` 决定长度，`optCap` 首元素仅决定分配门槛和新 allocation 的目标容量。若进入分配分支且 `capacity < size`，或现有 allocation 连 `size` 都容不下，函数以断言 panic，模拟 Go 对非法 make/切片操作的失败语义。

`WriteStmtBufs` 的定义位于 `pkg/sessionctx/variable/session.rs`，包含 `RowValBuf: Vec<u8>`、`AddRowValues: Vec<String>`、`IndexValsBuf: Vec<String>` 与 `IndexKeyBuf: Vec<u8>`。本文件只直接改动前两者；`GetWriteStmtBufs` 让同一 DML 上下文中的其他阶段复用 index 相关缓冲。

## 依赖与调用关系

直接上游与装配关系：

- `pkg/table/tblctx/lib.rs` 声明并再导出本模块，同时提供精简的 `kv::MemBuffer` trait、时区别名和依赖 crate 再导出。
- `pkg/table/tblctx/table.rs` 的 `MutateContext::GetMutateBuffers` 把本类型纳入表突变上下文接口。
- `pkg/table/tblsession/table.rs::NewMutateContext` 是已确认的 Rust 生产构造入口：`TakeWriteStmtBufs -> NewMutateBuffers`；其 trait 实现返回同一个内部 `MutateBuffers`。
- Rust 直接行为调用主要见 `pkg/table/tblctx/buffers_test.rs` 与 `migration_aster_unit_test.rs`。仓库 `rg` 未发现 Rust 生产路径直接调用本文件的具体编码/检查方法。
- RustCodeGraph 对同名 Go API 给出的调用边显示：`TableCommon.updateRecord`、`TableCommon.addRecord` 调 `WriteMemBufferEncoded`，`TableCommon.updateRecord` 调 `GetRowToCheck`；这与 `pkg/table/tables/tables.go` 源码一致，是 Go 对照主链证据，不是 Rust 已接线证据。

直接下游：

- `tablecodec::EncodeRow` / `EncodeOldRow`：校验列和值数量，展平 `Datum`，编码新/旧行格式。
- `rowcodec::RawChecksum` 和 `RowEncodingConfig.RowEncoder`：控制 checksum 和格式选择。
- `errctx::Context::HandleError` 与 `errors::New`：处理编码错误策略。
- `kv::MemBuffer::{Set, SetWithFlags}`：提交编码后的 key/value 及可选标志。
- `chunk::mutrow::MutRowFromDatums`：构建检查行的拥有型存储。
- `Rc`/`RefCell`：在单线程所有权模型下共享并动态借用 `WriteStmtBufs`。

`Cargo.toml` 对本文件最直接的 crate 依赖包括 `astersql-util-chunk`、`astersql-errctx`、`astersql-util-intest`、`astersql-tablecodec` 和 `astersql-sessionctx-variable`；它们经 `lib.rs` 统一再导出。本 crate 没有为该模块声明 feature。

## 错误处理与边界

- 编码错误先经 `handleEncodingError` 交给 `errctx`。值得注意的是，该辅助函数即使 `HandleError` 返回 `None`（按策略转为警告/忽略），也会用 `unwrap_or(error)` 返回原错误；因此当前 Rust 实现不会像 Go `err = ec.HandleError(err)` 那样在错误被上下文吞掉后继续写入。这是可见语义差异，修改前需要专门的错误策略测试。
- `RowEncodingConfig.RowEncoder` 为 `None` 时会以 `expect("RowEncodingConfig.RowEncoder is nil")` panic。`tblsession::GetRowEncodingConfig` 当前总是构造 `Some`，但公开 API 的其他调用方仍须遵守该前置条件。
- `RefCell::borrow_mut` 在存在重叠的可变或不可变借用时 panic。尤其持有 `GetWriteStmtBufs` 返回的 `RefMut` 时，不得再调用需要借用相同 `stmtBufs` 的编码方法。
- `MemBuffer::Set`/`SetWithFlags` 的错误原样返回；此时编码已经成功，且 `RowValBuf` 已更新，但写入未成功。事务回滚或 staging 清理由上层负责。
- `CheckedRow::GetInt64(column)` 未在本文件做下标或 Datum 类型检查；越界/类型行为由 chunk/Datum 实现决定。
- `row.len().saturating_mul(2)` 避免 usize 乘法溢出，但极端长度仍可能因分配失败而 panic/abort；正常 DML 行宽远低于此边界。

## 并发与资源生命周期

本文件没有线程、异步任务、锁、通道或事务所有权。`Rc<RefCell<_>>` 明确是单线程共享模型，`MutateBuffers` 因 `Rc` 不能跨线程安全发送；调用方应把它限制在所属 session/语句执行线程中。

`MutateBuffers` 拥有两个长期存在的子缓冲，每次 getter 返回其可变引用并立即重置。Rust 的 `&mut self` 借用可在编译期阻止同时持有两个由同一 getter 链产生的冲突可变借用，但借用结束后下一次 getter 会覆盖先前逻辑内容。`CheckedRow` 通过 clone Datum 并拥有 `MutRow`，所以在原 `CheckRowBuffer` reset 后仍有独立生命周期；代价是构建时复制。

编码期间对 `WriteStmtBufs` 的动态借用在调用 `MemBuffer` 前显式 `drop`，避免把 `RefCell` 借用跨越外部写入。`RowValBuf` 被移出后若 `EncodeRow` 返回错误，函数会提前返回，而原缓冲不会写回（`stmtBufs.RowValBuf` 留为 `Vec::new()`）；这是当前资源状态转换，扩展错误路径时应决定是否需要恢复旧 allocation。

## 与 Go 版本的对应关系

总体结构与 `pkg/table/tblctx/buffers.go` 一一对应：三个 buffer 类型、两类 getter、容量复用函数、编码/写入分支和 binlog 旧格式均保留。`pkg/table/tblctx/buffers_test.rs` 也复刻 `buffers_test.go` 的新旧格式、checksum、flags、容量保留、共享缓冲和独立 binlog 字节等意图。

重要差异如下：

- Go 持有 `*WriteStmtBufs`，Rust 构造函数取得 `WriteStmtBufs` 所有权并用 `Rc<RefCell<_>>` 共享；Rust `tblsession` 通过 `TakeWriteStmtBufs` 配合这一所有权设计。
- Go 把真实 `[]Datum AddRowValues` 传给 `EncodeRow` 复用；当前 Rust 集成类型把 `AddRowValues` 定义为 `Vec<String>`，本文件只维护两倍列数的长度/容量，并向 `EncodeRow` 传 `None`，后者另行分配 `Vec<Datum>`。行为结果可对齐，但该部分的分配优化尚未等价。
- Go `GetRowToCheck` 返回借用底层切片的 `chunk.Row`；Rust 返回拥有 clone 后数据的 `CheckedRow`，生命周期更明确但有复制成本，且读取 API 目前只有 `Len`/`GetInt64`。
- Rust 编码调用克隆 `row`、`colIDs` 和最终 encoded bytes，以适配当前按值接口；Go 主要传递切片。这会增加每行分配/复制风险。
- Go 的 `ec.HandleError` 可以把某些编码错误降为警告并返回 nil；当前 Rust `handleEncodingError` 始终产生 `Err`，如“错误处理与边界”所述。
- Go 完整 AddRecord/UpdateRecord 路径已直接使用这些 API；Rust 当前只确认 session 构造和 trait 暴露，具体生产调用尚未检出。

## 扩展指南

- 新增编码选项时，优先扩展 `RowEncodingConfig` 和 `WriteMemBufferEncoded` 的配置分支，并同步 `pkg/table/tblctx/buffers_test.rs`；同时检查 `pkg/table/tblsession/table.rs::GetRowEncodingConfig` 是否需要提供新值。
- 若要消除 Go/Rust 的旧行 scratch-buffer 差异，应先统一 `WriteStmtBufs.AddRowValues` 的实际类型与 `tablecodec::EncodeRow` 参数，再验证容量复用和结果一致性；不要只删除当前长度维护逻辑。
- 若要扩展约束读取类型，在 `CheckedRow` 上增加与 chunk 对应的窄接口，并在独立测试文件中覆盖类型、空值和越界边界；不要把测试嵌入本生产文件。
- 若把具体编码/检查方法接入 Rust AddRecord/UpdateRecord，需保持“取得缓冲—填充—消费完成后才可再次 getter”的顺序，并补集成级回归，确认 flags、checksum、约束检查及 staging 错误清理。
- 性能修改应重点测量四处复制/分配：`row.clone()`、`colIDs.clone()`、`encoded.clone()`、`GetRowToCheck` 的 Datum clone；优化时不得破坏 binlog 返回值不共享内部 byte buffer 的契约。
- 错误策略修改必须覆盖 `errctx` 可忽略/转警告和严格返回两类上下文，并明确编码失败后是否恢复 `RowValBuf`。
- `ensureCapacityAndReset` 是公开函数；改变断言、默认填充或 optCap 规则前，应同步 Rust `TestEnsureCapacityAndReset` 与 Go `TestEnsureCapacityAndReset` 的语义矩阵。

## 验证依据

- RustCodeGraph 状态：本仓库索引可用，目标目录 9 个文件均被索引；`buffers.rs` 显示 24 个符号。读取了目标文件全部 233 行，并查询了 `NewMutateBuffers`、`WriteMemBufferEncoded`、`ensureCapacityAndReset`、`GetRowToCheck` 的节点/调用边。
- 源码与装配：`pkg/table/tblctx/buffers.rs`、`pkg/table/tblctx/lib.rs`、`pkg/table/tblctx/table.rs`、`pkg/table/tblctx/Cargo.toml`、`pkg/table/tblsession/table.rs`。
- 下游实现：`pkg/sessionctx/variable/session.rs::WriteStmtBufs`、`pkg/tablecodec/tablecodec.rs::{EncodeRow, EncodeOldRow}`、`pkg/errctx/context.rs::Context::HandleError`。
- Rust 测试：`pkg/table/tblctx/buffers_test.rs` 和 `pkg/table/tblctx/migration_aster_unit_test.rs`。覆盖新/旧格式、checksum、flags、容量复用、共享 `WriteStmtBufs`、检查行读取和 binlog buffer 独立性。
- Go 对照：`pkg/table/tblctx/buffers.go`、`pkg/table/tblctx/buffers_test.go`，以及 `pkg/table/tables/tables.go` 中的 `TableCommon.updateRecord`、`TableCommon.addRecord`、`addIndices`。
- 全仓 Rust 文本搜索确认：生产侧存在 `tblsession` 构造和 `MutateContext` 暴露；本文件具体编码/检查方法的直接 Rust 调用仅在相关测试中检出。RustCodeGraph 的同名方法调用边主要落到 Go 实现，文档已按语言区分。
- 本任务是纯文档分析，按计划不运行 Cargo；最终只执行固定 11 章节的结构校验，并人工检查未把 Go 主链当成 Rust 已接线事实。
