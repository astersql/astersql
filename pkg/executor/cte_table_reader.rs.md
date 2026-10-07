# `pkg/executor/cte_table_reader.rs`

## 文件定位

本文件是 `astersql-executor` crate 中的 CTE（Common Table Expression，公用表表达式）迭代输入读取器实现，由 [`pkg/executor/lib.rs`](./lib.rs) 以 `pub mod cte_table_reader` 导出。它把 [`astersql_util_cteutil::Storage`](../util/cteutil/storage.rs) 中按 chunk 保存的某一轮递归输入，转换为执行器上游逐次拉取的 `Chunk`。

需要区分“文件内已实现的语义”和“当前主链接线”：本文件定义了公开的 `CTEReaderBase` 与 `CTETableReaderExec<B>`，但仓库搜索未发现 `CTEReaderBase` 的实现或 `CTETableReaderExec` 的 Rust 构造点。当前 [`builder.rs`](./builder.rs) 的 `buildCTETableReader` 委托给 `build_cte_table_reader_executor`，使用的是 `Arc<dyn CteStorage>`/`ExecutorBox` 抽象，而不是本文件的 `Box<dyn Storage>` 类型。因此本模块目前是已导出、可复用但尚未接入完整 Rust 构建主链的迁移实现，不能仅凭 Go 主链推断它已经承接 SQL 请求。

## 核心职责

- `CTETableReaderExec::Open` 和 `Close` 在委托基础执行器生命周期前复位读取状态。
- `CTETableReaderExec::Next` 每次先清空调用方提供的结果 chunk，再至多返回存储中的一个 chunk；没有剩余数据时以空 chunk 和 `Ok(())` 表示本轮耗尽。
- 以存储的迭代号 `Storage::GetIter()` 识别递归 CTE 是否进入新一轮。新轮次开始时把 `chk_idx` 归零，以便从新一轮输入的第一个 chunk 重新读取。
- 返回前通过 `CopyConstructSel` 复制存储 chunk，并以 `SwapColumns` 交给请求 chunk，避免上层算子改变共享 CTE 存储。
- 拒绝读取器的 `cur_iter` 大于存储迭代号的倒退状态，防止在生产者状态异常时静默读取错误轮次。

## 主要符号

- `pub trait CTEReaderBase`：迁移期的最小执行器生命周期边界，只要求 `open(&mut self)` 和 `close(&mut self)` 返回 `cteutil::errors::Error`。它没有 `Next`、schema 或上下文接口，因而不是 Go `exec.Executor` 的完整 Rust 等价物。
- `pub struct CTETableReaderExec<B: CTEReaderBase>`：读取器状态对象。泛型 `B` 保存基础执行器，`iter_in_tbl: Box<dyn Storage>` 保存递归轮次输入，`chk_idx` 是下一个 chunk 下标，`cur_iter` 是读取器已观察到的迭代号。四个字段当前均为 `pub`。
- `Open<C>(&mut self, _ctx: C)`：接受但不使用任意上下文类型；先调用私有状态逻辑 `reset`，再调用 `base_executor.open()`，并原样传播错误。
- `Next<C>(&mut self, _ctx: C, req: &mut Chunk)`：核心拉取入口。上下文同样未使用；执行迭代同步、边界检查、chunk 获取、复制和游标推进。
- `Close(&mut self)`：先 `reset`，再调用 `base_executor.close()`，并原样传播错误。
- `reset(&mut self)`：把 `chk_idx` 和 `cur_iter` 都设为 `0`。它是公开方法，但只修改本地游标，不清空或关闭共享存储。
- 文件没有模块级常量、枚举、条件编译项或内部测试模块；`#![allow(non_snake_case)]` 用于保留与 Go 的 `Open`/`Next`/`Close` 命名对齐。

## 执行流程

1. 构造方必须先提供一个实现 `CTEReaderBase` 的基础执行器和一个已经可读的 `Storage`。本文件本身不创建、打开或填充存储。
2. `Open` 调用 `reset`，令下一次读取从本地初始状态开始，然后打开基础执行器。若基础执行器打开失败，读取器仍保持已复位状态。
3. `Next` 首先执行 `req.Reset()`；即使后续返回错误，调用方也不会继续看到上一次调用遗留的数据。
4. 读取一次 `storage_iter = iter_in_tbl.GetIter()`。若它与 `cur_iter` 不同：
   - `cur_iter > storage_iter` 时立即返回包含两者数值的 `invalid iteration` 错误；
   - 否则认为生产者推进到了新一轮，把 `chk_idx` 设为 `0`，并把 `cur_iter` 更新为 `storage_iter`。
5. 若 `chk_idx < NumChunks()`，调用 `GetChunk(chk_idx)`。成功后复制选择态 chunk，把副本的列与 `req` 交换，最后将 `chk_idx` 加一。`GetChunk` 失败时不推进游标，因此调用方可在处理错误后决定是否重试。
6. 若下标已到末尾，直接返回空的 `req` 和 `Ok(())`；调用方据此结束当前轮次的拉取循环。
7. `Close` 再次复位本地状态并关闭基础执行器；它不调用 `Storage::DerefAndClose`，存储所有权和引用生命周期属于外层 CTE 生产者/构建器。

## 数据与状态

`chk_idx` 与 `cur_iter` 组成读取游标。`chk_idx` 只在成功交付一个 chunk 后递增；`cur_iter` 只在看到更大的存储迭代号后更新。其关键不变量是：同一迭代号内顺序读取 `[0, NumChunks())`，迭代号前进时重新从 `0` 读取，迭代号后退则报错。

`iter_in_tbl` 是 trait object，文件只依赖 `GetIter`、`NumChunks` 与 `GetChunk` 三个只读操作。存储接口在 [`pkg/util/cteutil/storage.rs`](../util/cteutil/storage.rs) 中说明写侧显式加锁、完成填充后允许多个读取者访问不可变 chunk；`GetChunk` 在存储未打开/无效时返回错误。该读取器不检查 `Done()` 或存储内的 `Error()`，生产者与调度方必须保证调用时机。

输出数据不是存储对象本身：`GetChunk` 取得 chunk 后又执行 `CopyConstructSel`，随后 `SwapColumns` 把副本列移入 `req`。这保留了共享中间结果不被上层原地修改的隔离边界，但每个非空 `Next` 都承担一次 chunk 复制成本。

## 依赖与调用关系

- crate 边界：[`pkg/executor/Cargo.toml`](./Cargo.toml) 声明 crate 名为 `astersql-executor`，并以路径依赖引入 `astersql-util-chunk` 与 `astersql-util-cteutil`；本文件只直接使用这两个 crate。
- 模块入口：[`pkg/executor/lib.rs`](./lib.rs) 公开导出 `cte_table_reader`。
- 文件内调用边：RustCodeGraph 确认 `Open -> reset`、`Close -> reset`；`Next` 下游调用 `Chunk::Reset`、`Storage::{GetIter, NumChunks, GetChunk}`、`Chunk::CopyConstructSel` 和 `Chunk::SwapColumns`。
- 设计上的生产者：[`pkg/executor/cte.rs`](./cte.rs) 描述 `CTEExec`/producer 写入 `iterInTbl`，递归分支经 table reader 消费本轮输入并生成下一轮输出；该文件中的 Rust producer 使用另一套后端抽象，不能据此声称已直接调用本类型。
- 当前 Rust 构建入口：[`pkg/executor/builder.rs`](./builder.rs) 的 `buildCTETableReader` 校验同一 storage ID 的 CTE 存储已建立，再委托 `build_cte_table_reader_executor`。仓库内没有到本文件类型的构造或 trait 实现边，接线仍未验证。
- Go 已接线入口：[`pkg/executor/builder.go`](./builder.go) 的 `buildCTETableReader` 从 `CTEStorages.IterInTbl` 取存储并直接构造 Go `CTETableReaderExec`。

## 错误处理与边界

- `Open`/`Close` 不包装基础执行器错误；调用方收到 `CTEReaderBase` 的原始 `errors::Error`。
- `Next` 会传播 `Storage::GetChunk` 的错误。由于游标只在成功复制后递增，失败不会跳过 chunk。
- `cur_iter > storage_iter` 被视为非法倒退，错误消息同时包含读取器和存储迭代号，便于定位生产/消费状态错位。
- `cur_iter < storage_iter` 无论跨越一轮还是多轮都会直接同步到最新轮次；本文件不检测中间轮次是否被跳过。
- `chk_idx >= NumChunks()` 不是错误，而是返回空 chunk。接口没有单独的 EOF 类型。
- `NumChunks()` 与随后的 `GetChunk()` 之间没有在本文件内加锁。如果写侧并发改变存储，正确性依赖 `Storage` 文档规定的“填充完成后只读”协议。
- `Open` 先复位再打开，`Close` 先复位再关闭；即使底层生命周期调用失败，本地游标也已经归零。

## 并发与资源生命周期

本类型使用 `&mut self` 执行生命周期和拉取方法，因此单个读取器实例的游标更新要求独占可变访问；文件自身不创建线程、任务、通道、锁或事务。`Box<dyn Storage>` 也没有在这里包裹 `Arc`，读取器对 trait object 拥有唯一的 Box 所有权，但底层具体实现是否共享资源由实现方决定。

读取器不负责 `Storage::OpenAndRef`、`DerefAndClose`、`Reopen`、`Lock` 或 `Unlock`。外层必须先建立有效存储，并在所有读取器结束后统一释放资源。关闭读取器只关闭 `base_executor`，不会释放 CTE 存储。存储的 Rust 回归测试 [`pkg/util/cteutil/storage_test.rs`](../util/cteutil/storage_test.rs) 与 [`migration_aster_unit_test.rs`](../util/cteutil/migration_aster_unit_test.rs) 覆盖打开前读取失败、chunk 往返、`Reopen` 清理、迭代元数据和显式锁语义；它们验证下游存储契约，不等价于本读取器的独立测试。

## 与 Go 版本的对应关系

直接对照文件是 [`pkg/executor/cte_table_reader.go`](./cte_table_reader.go)。Rust 的字段和流程逐项对应 Go 的 `BaseExecutor`、`iterInTbl`、`chkIdx`、`curIter` 以及 `Open`、`Next`、`Close`、`reset`：两者都在新迭代复位 chunk 下标、拒绝迭代倒退、复制 chunk 防止上层修改共享存储，并以空 chunk 表示耗尽。

当前差异主要来自迁移接线而不是核心算法：

- Go 类型直接嵌入 `exec.BaseExecutor` 并实现完整 `exec.Executor`；Rust 用最小 `CTEReaderBase` trait 泛化基础生命周期，且没有完整 executor trait 的静态约束。
- Go 接受 `context.Context`；Rust 的 `Open<C>`/`Next<C>` 接受任意但未使用的上下文，`Close` 无上下文。
- Go builder 已直接构造该执行器；Rust builder 当前委托另一组依赖 trait，尚未找到本类型的构造点。
- Go `Storage.GetChunk` 的结果随后 `CopyConstructSel`；Rust `Storage::GetChunk` 已返回一个 `Chunk` 值，但仍保持额外复制步骤以对齐 Go 的隔离语义。

Go 的递归 CTE 行为回归集中在 [`pkg/executor/test/cte/cte_test.go`](./test/cte/cte_test.go)，覆盖多类 `WITH RECURSIVE` SQL 语义；仓库内未找到专门实例化本 Rust 类型的独立测试，因此这些 Go 测试只能证明原实现及整体 SQL 行为，不能证明 Rust 模块已接线或已被执行。

## 扩展指南

- 接入 Rust 主链时，优先在 `builder.rs` 的 `build_cte_table_reader_executor` 实现侧建立明确适配层：统一当前 `CteStorage` 与本文件 `Storage` 的类型边界，并让基础执行器实现 `CTEReaderBase`。不要并行保留两套不可互操作的 CTE reader 状态模型。
- 若扩展迭代协议，应首先修改/审查 `Next` 中 `cur_iter` 比较分支，并同步检查 CTE producer 设置迭代号的位置。尤其要明确是否允许跳轮、何时读取 `Done/Error`、写侧何时停止改变 `NumChunks`。
- 若优化复制成本，必须保留“上层不能修改共享 `iter_in_tbl`”的不变量；借用、引用计数或 copy-on-write 方案需要同时验证选择向量、列缓冲所有权和并发只读期。
- 若增加错误上下文，保持 `GetChunk` 失败时不推进 `chk_idx`，并保留迭代倒退错误中的两个迭代号。
- 为本文件新增行为测试时，应创建同目录独立测试文件（例如 `cte_table_reader_test.rs`）并在 `lib.rs` 以 `#[cfg(test)]` 引入，不要把测试内嵌回生产源文件。至少覆盖生命周期复位、同轮顺序读取、迭代推进后从 chunk 0 重读、倒退报错、耗尽返回空 chunk、`GetChunk` 错误不推进以及副本隔离。
- 完整接线后还应同步 Go 的 [`pkg/executor/test/cte/cte_test.go`](./test/cte/cte_test.go) 所表达的 SQL 级兼容预期，并新增 Rust 构建器测试证明物理 CTE table plan 实际构造并调用本读取器。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件；`files --filter pkg/executor/cte_table_reader.rs` 确认目标文件已索引并识别 9 个符号。
- RustCodeGraph `explore "cte_table_reader.rs CteTableReaderExecutor"`：读取目标文件全貌，并报告 `Open`、`Close` 到 `reset` 的调用边；`query CTETableReaderExec` 同时定位 Go/Rust 两个同名结构体，`query CTEReaderBase` 定位本文件 trait。精确 `callers/callees` 命令在本次限定时间内未返回，因此没有把缺失的图结果当作已验证调用关系。
- 直接阅读：[`pkg/executor/cte_table_reader.rs`](./cte_table_reader.rs)、[`pkg/executor/lib.rs`](./lib.rs)、[`pkg/executor/Cargo.toml`](./Cargo.toml)、[`pkg/executor/builder.rs`](./builder.rs)、[`pkg/executor/cte.rs`](./cte.rs)、[`pkg/util/cteutil/storage.rs`](../util/cteutil/storage.rs)。
- Go 对照与整体回归：[`pkg/executor/cte_table_reader.go`](./cte_table_reader.go)、[`pkg/executor/builder.go`](./builder.go)、[`pkg/executor/cte.go`](./cte.go)、[`pkg/executor/test/cte/cte_test.go`](./test/cte/cte_test.go)。
- Rust 下游存储测试：[`pkg/util/cteutil/storage_test.rs`](../util/cteutil/storage_test.rs)、[`pkg/util/cteutil/migration_aster_unit_test.rs`](../util/cteutil/migration_aster_unit_test.rs)。仓库搜索未发现 `CTEReaderBase` 实现、本 Rust `CTETableReaderExec` 构造点或专属 Rust 测试。
- 本任务只新增文档，按任务约束未运行 Cargo；交付结构以固定 11 个二级标题的 shell 校验为准。
