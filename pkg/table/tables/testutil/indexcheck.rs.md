# `pkg/table/tables/testutil/indexcheck.rs`

## 文件定位

该文件属于 `astersql-table-tables-testutil` crate，是表模块测试辅助包中的索引 KV 数量检查适配层。crate 入口 `pkg/table/tables/testutil/lib.rs` 通过 `pub mod indexcheck` 声明模块，并以 `pub use indexcheck::*` 将本文件的公共项提升到 crate 根；根工作区又通过 `facade_table_tables_testutil` 暴露该 crate（`Cargo.toml`）。

当前 Rust 实现没有直接连接 `Domain`、会话事务或 KV 快照，而是把这些能力抽象为 `IndexCheckRuntime` 和 `SnapshotIterator`，由调用者注入。因此它当前可确认的使用面是 `pkg/table/tables/testutil/indexcheck_test.rs` 中的内存单元测试；全库限定路径搜索未发现其他 Rust 调用者。它用于保留 Go 测试工具 `pkg/table/tables/testutil/indexcheck.go::CheckIndexKVCount` 的扫描与清理语义，但尚不是对真实 TiDB/AsterSQL 运行时的完整适配。

## 核心职责

- `CheckIndexKVCount` 固定在 `test` 数据库中查找目标表和索引对应的抽象元数据。
- 从 `IndexMeta::minimum_key` 创建快照迭代器，连续统计能解码为目标 `index_id` 的键。
- 遇到不同索引 ID、不能解码为索引 ID 的键或迭代器失效时结束连续前缀扫描，避免把相邻键空间计入目标索引。
- 无论扫描成功、扫描报错还是计数不符，进入扫描闭包后都会尝试关闭迭代器，并在事务已经开始后尝试提交。
- 将运行时错误和数量不一致统一表示为 `IndexCheckError`，供独立测试以 `Result` 方式断言，而不是像 Go 版本那样直接调用 `testing.T`/`require`。

## 主要符号

- `IndexCheckError(pub String)`：公开的字符串错误包装；派生 `Clone`、`Debug`、`Eq`、`PartialEq`，并实现 `Display` 与 `std::error::Error`。当前没有错误类别或源错误链。
- `IndexMeta { table_id, index_id, minimum_key }`：运行时返回的最小索引元数据。扫描逻辑读取 `index_id` 和 `minimum_key`；`table_id` 是公开字段，但当前函数不使用它，主要用于保留实现真实运行时适配所需的表身份。
- `SnapshotIterator`：对象安全的公开迭代器 trait。`valid` 和 `key` 读取当前位置，`next` 前进，`close` 显式释放资源。`snapshot_iter` 以 `Box<dyn SnapshotIterator>` 返回它。
- `IndexCheckRuntime`：公开的依赖注入边界。`index_meta` 负责名称解析和起始键准备，`begin`/`commit` 管理事务，`snapshot_iter` 打开扫描，`decode_index_id` 判断键所属索引。
- `CheckIndexKVCount(runtime, table_name, index_name, expected)`：唯一公开业务入口。名称沿用 Go 导出函数，crate 根的 `#![allow(non_snake_case)]` 明确允许该命名。

文件没有模块级常量、枚举、条件编译项或内部私有辅助函数；所有行为集中在上述入口和两个 trait 契约中。

## 执行流程

1. 调用 `runtime.index_meta("test", table_name, index_name)`。若失败，立即返回，尚未开始事务。
2. 调用 `runtime.begin()`。若失败，立即返回，也不会调用 `commit`。
3. 在外层扫描闭包中用 `meta.minimum_key` 调用 `snapshot_iter`。这一调用若失败，闭包记录该错误，随后仍执行第 8 步的提交。
4. 初始化局部计数器 `count = 0`，只在函数调用期间存在。
5. 当 `iterator.valid()` 为真时，读取 `iterator.key()` 并调用 `decode_index_id`。
6. 只有解码结果为 `Some(meta.index_id)` 时才递增计数并调用 `iterator.next()`；不同 ID 或 `None` 都直接终止循环，边界键本身不会前进或计数。解码及前进错误由 `?` 记录为扫描错误。
7. 循环正常或异常结束后调用 `iterator.close()`，但显式忽略关闭结果。随后传播迭代错误；若扫描无错但 `count != expected`，构造包含表名、索引名、实际值和期望值的错误。
8. 扫描闭包返回后无条件调用 `runtime.commit()`，再以 `scan_result.and(commit_result)` 合并结果：扫描失败优先；只有扫描成功时才返回提交错误。

`pkg/table/tables/testutil/indexcheck_test.rs` 的调用序列断言验证了正常单键路径为 `index_meta -> begin -> snapshot_iter -> decode -> next -> close -> commit`，并验证相邻索引边界停止时不会调用该边界键之后的 `next`。

## 数据与状态

`IndexMeta` 是由运行时生成、按值返回的快照式描述；`minimum_key` 拥有其字节缓冲区，传入 `snapshot_iter` 时只借用切片。`index_id` 是连续扫描的身份不变量：计数仅在解码 ID 与它相等时增长。`table_id` 当前不参与比较或扫描。

`CheckIndexKVCount` 自身不保存全局状态。可变状态位于调用方提供的 `&mut dyn IndexCheckRuntime`、堆分配的迭代器 trait object 和局部 `count` 中。测试运行时用 `Rc<RefCell<Vec<String>>>` 记录调用顺序、用 `HashMap<Vec<u8>, Option<i64>>` 注入解码结果；这些是测试实现细节，不是生产接口的并发保证。

名称契约中数据库固定为字符串 `"test"`，表名与索引名原样转交。期望数量使用 `usize`，与 Go 版本的 `int` 类型不同；接口没有处理负数期望值的语义。

## 依赖与调用关系

RustCodeGraph 对 `pkg/table/tables/testutil/indexcheck.rs::CheckIndexKVCount` 的精确节点显示九条直接下游调用边：`IndexCheckRuntime::{index_meta, begin, commit, snapshot_iter, decode_index_id}` 与 `SnapshotIterator::{valid, key, next, close}`。本文件自身的唯一语言依赖是 `std::fmt`。

上游模块关系为 `pkg/table/tables/testutil/lib.rs -> indexcheck`，且公共项被再导出。当前可检索到的实际 Rust 调用者仅是 `pkg/table/tables/testutil/indexcheck_test.rs` 中四个测试；没有找到实现真实 `Domain`、事务管理器或 KV snapshot 的 `IndexCheckRuntime`。因此不能把 Cargo 中声明的依赖误写成已经存在的真实调用链。

`pkg/table/tables/testutil/Cargo.toml` 将 crate 映射到 Go 包 `pkg/table/tables/testutil`，并声明 `astersql-domain`、parser AST、`astersql-sessiontxn`、`astersql-table`、`astersql-tablecodec`、`astersql-testkit`、codec 与 collate 等路径依赖。这些与 Go 原实现使用的子系统相符，但当前 `indexcheck.rs` 尚未直接导入它们；它们更像 crate 移植边界和未来真实适配所需依赖，而不是本文件现有执行边。

## 错误处理与边界

- `index_meta` 失败：直接返回，不调用 `begin`、`close` 或 `commit`。
- `begin` 失败：直接返回，不调用 `commit`。
- `snapshot_iter` 失败：返回该扫描错误，但仍调用 `commit`；没有迭代器可关闭。
- `decode_index_id` 或 `next` 失败：保存原错误，随后仍调用 `close` 和 `commit`；因 `scan_result.and(commit_result)`，扫描错误覆盖同时发生的提交错误。
- `close` 失败：错误被忽略，保持 Go `defer iter.Close()` 未检查返回值的行为。测试 `check_index_kv_count_ignores_iterator_close_error_like_go_defer` 明确锁定该约定。
- 数量不符：先关闭、再提交，最终返回形如 `index orders.by_customer contains 1 KV pairs, expected 2` 的错误；测试 `check_index_kv_count_reports_count_mismatch_after_close_and_commit` 验证顺序和文本。
- 扫描成功但提交失败：返回提交错误；测试 `check_index_kv_count_returns_commit_error_after_a_successful_scan` 覆盖此分支。
- `decode_index_id` 返回 `None` 或不同 ID：视为键空间边界并正常停止，而不是错误。`check_index_kv_count_stops_at_the_next_index_prefix` 覆盖不同 ID。

当前测试没有单独覆盖 `index_meta`、`begin`、`snapshot_iter`、`decode_index_id` 或 `next` 的失败，也没有覆盖扫描错误与提交错误同时发生时的优先级；这些行为可由源码控制流确认，但扩展时宜补充回归测试。

## 并发与资源生命周期

函数是同步的，没有线程、异步任务、锁或通道。`&mut dyn IndexCheckRuntime` 保证单次调用期间对运行时的独占可变借用，但 trait 未要求 `Send` 或 `Sync`，所以接口本身不承诺跨线程使用。

事务生命周期从 `begin` 成功开始，到函数末尾的 `commit` 尝试结束。实现不提供显式回滚，也不区分只读事务的成功/失败清理；这是对 Go 工具 `defer tk.MustExec("COMMIT")` 的刻意对齐。迭代器生命周期位于扫描闭包内：成功构造后，无论循环如何退出都会调用一次显式 `close`，随后 trait object 离开作用域并被丢弃。关闭错误不改变返回结果。

接口依赖实现方保证 `valid() == true` 时 `key()` 可安全读取，并保证 `next()` 推进或返回错误；本文件不能防止一个永不失效且不推进的恶意/错误实现造成无限循环。

## 与 Go 版本的对应关系

Go 对照文件是 `pkg/table/tables/testutil/indexcheck.go`，同名函数的核心语义一致：固定查找 `test` 库、生成目标索引最小键、开启事务、从快照起始键顺序扫描、仅统计相同索引 ID、遇到非目标索引停止、关闭迭代器、提交并核对数量。

主要差异如下：

- Go 版本直接通过 `domain.Domain` 查表和索引、`tablecodec.GenIndexKey` 生成起始键、`sessiontxn` 取 statement read timestamp 对应的 snapshot；Rust 将所有这些动作合并进 `IndexCheckRuntime`，当前没有真实运行时实现。
- Go 版本在循环中先用 `tablecodec.IsIndexKey` 拒绝非索引键，再用 `DecodeIndexKey` 读取索引 ID。Rust 的 `decode_index_id -> Result<Option<i64>, _>` 把“是否为索引键”和“解码 ID”合为一个契约，其中 `None` 表示正常边界。
- Go 版本通过 `require` 立即终止测试；Rust 返回 `Result<(), IndexCheckError>`，让调用者决定如何断言。
- Go 的 `defer` 以逆序执行：迭代器关闭发生在提交之前。Rust 显式保持 `close -> commit` 顺序，并同样忽略关闭返回值。
- Go 在索引名未匹配时仍会继续解引用 `idx`，依赖测试前置条件；Rust 把“索引存在且最小键可生成”纳入 `index_meta` 的 `Result` 契约。
- Go 使用 `int` 期望值，Rust 使用 `usize`；Rust 因而不接受负数。

这些差异说明当前文件是可测的语义骨架，而非 Go 运行时接线的一比一完成版。

## 扩展指南

要接入真实运行时，应优先新增独立适配实现文件，让适配器实现 `IndexCheckRuntime`，并让快照包装器实现 `SnapshotIterator`；不要把 Domain、会话和 KV 细节塞入 `CheckIndexKVCount` 的扫描算法。适配时需要逐项对齐 Go 的 `TableByName`、大小写归一化索引查找、新排序规则 encoder、`GenIndexKey`、statement read timestamp snapshot、`IsIndexKey` 与 `DecodeIndexKey`。Cargo 已声明相关 crate，但实际 API 与生命周期必须由代码验证，不能仅凭依赖名推断。

修改扫描边界时应集中检查 `CheckIndexKVCount` 的 `decode_index_id` 分支；修改事务清理或错误优先级时应检查外层 `scan_result.and(commit_result)`；修改资源清理时应保持迭代器只关闭一次且先于提交。若决定传播关闭错误或失败时回滚，这会偏离 Go 行为，需要明确兼容性理由和新的优先级测试。

测试必须继续放在独立文件 `pkg/table/tables/testutil/indexcheck_test.rs`，不要内嵌到源文件。建议补充元数据、begin、snapshot、decode、next 失败，以及扫描错误与提交错误并发出现的表格化用例；真实适配完成后还需增加能经过真实 tablecodec 与 snapshot 的集成测试。性能方面应保持流式计数和遇边界立即停止，避免收集所有键或扫描越过索引前缀。

## 验证依据

- RustCodeGraph `status`：索引包含目标 Rust、Go 和测试文件；`files --filter pkg/table/tables/testutil` 列出 `indexcheck.rs`、`indexcheck_test.rs`、`indexcheck.go` 与 `lib.rs`。
- RustCodeGraph `node --file pkg/table/tables/testutil/indexcheck.rs`：核对 103 行完整源码、公共符号、控制流及文件使用关系。
- RustCodeGraph `query/node CheckIndexKVCount`：区分 Go/Rust 两个同名定义，并确认 Rust 入口到两个 trait 的九条直接调用边。
- RustCodeGraph `node --file pkg/table/tables/testutil/indexcheck_test.rs`：核对四个独立测试覆盖的调用顺序、相邻索引边界、关闭错误、数量错误与提交错误。
- RustCodeGraph `node --file pkg/table/tables/testutil/lib.rs`：核对模块声明、公共再导出和独立测试文件挂载。
- RustCodeGraph `node --file pkg/table/tables/testutil/indexcheck.go`：核对 Domain/InfoSchema、索引最小键、事务 snapshot、键类型与索引 ID 边界以及 defer 清理语义。
- `pkg/table/tables/testutil/Cargo.toml`、根 `Cargo.toml` 和 `pkg/lib.rs`：核对 crate 名、Go 包映射、依赖声明、工作区成员和 facade 再导出。
- `rg` 限定 Rust 源码搜索：确认本文件公共 trait/类型及 `CheckIndexKVCount` 当前仅由同目录独立测试使用；RustCodeGraph 的全库调用方命令曾超时，因此该结论同时以限定文本搜索交叉验证。

本任务是纯文档分析，没有运行 Cargo 或代码测试。结构验证应确认本文恰有计划规定的十一个二级标题。
