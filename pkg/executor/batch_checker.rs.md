# `pkg/executor/batch_checker.rs`

## 文件定位

该文件属于 `astersql-executor` crate；crate 根 `pkg/executor/lib.rs` 以 `pub mod batch_checker` 暴露它，Cargo 边界由 `pkg/executor/Cargo.toml` 的 `[lib] path = "lib.rs"` 确定。它移植了 Go 文件 `pkg/executor/batch_checker.go` 中 INSERT、REPLACE 和 `ON DUPLICATE KEY UPDATE` 写入前的批量冲突键准备算法：把待写行转换为 record key 和可判重的 unique index key，同时为每个键预制重复键错误。

当前 Rust 模块不是完整的生产接线层。真实表元数据、分区定位、Datum、句柄、索引编码、事务读取和表达式求值均由 `BatchCheckerRuntime` trait 注入；仓库搜索未发现该 trait 的实现，也未发现文件外对 Rust 入口的调用。相对地，Go 版本已由 `insert_common.go`、`insert.go` 和 `replace.go` 调用。因此本文件的准确定位是“已移植且可复用的泛型算法边界”，不是已经替代 Go 写路径的执行器。

## 核心职责

- `getKeysNeedCheck`/`getKeysNeedCheckOneRow` 为一批待写行确定实际物理表或分区，构造主键 record key，并收集所有可写唯一索引的 distinct key。
- 为 record key 和 unique index key 生成与 Go 语义一致的重复键错误，包括 `表名.PRIMARY` 和 `表名.索引名`，并对字节、bit、binary literal 做可读格式化。
- 在 DDL add/drop/modify/change column 期间临时补齐非 public 可写列，使索引取值看到完整写入视图；结果入队前恢复原行长度。
- `getOldRow` 从事务读取已有记录，补回旧记录中缺失的 write-only/write-reorg 默认值，并按列顺序重算 public 虚拟生成列，供 REPLACE 或重复行更新使用。
- 通过 `BatchCheckerRuntime` 将算法与具体的 session/table/kv/types 实现隔离；trait 明确要求实现方提供真实操作，不存在“默认成功”的桩路径。

## 主要符号

- `DatumKind`：错误文本格式化所需的 Datum 粗分类。`Bytes` 会去掉尾部 NUL（全为 NUL 时保留一个），三种二进制类别最后都经 `printable_non_ascii_as_hex` 处理。
- `KeyValueWithDupInfo<K, E>`：把已编码的 `new_key` 与命中该键时应返回的 `dup_err` 绑定。
- `ToBeCheckedRow<Row, K, E, T>`：单行的查重工作单元，保存原行、可选 handle key、unique keys、实际表/分区和 `ignored` 标志。分区错误被 ErrCtx 接受时，`table` 为 `None` 且 `ignored = true`。
- `GeneratedIndexKey<K>`：索引生成器的一项结果；只有 `distinct = true` 的键需要冲突检查。
- `BatchCheckerRuntime`：本文件唯一的环境适配接口。关联类型覆盖 context、table、row、datum、column、index、handle、key、transaction、expression 和 error；方法分为元数据判断、行访问、句柄/键编码、DDL 列补齐、索引生成、事务读取及生成列计算。
- `getKeysNeedCheck`：批量入口。预估唯一索引数，识别 integer PK handle 或 common handle 的组成列，然后逐行调用单行入口。
- `getKeysNeedCheckOneRow`：核心算法。完成分区路由、句柄键、DDL 过渡列、部分/唯一索引筛选、临时索引键转换及结果组装。
- `buildHandleFromDatumRow`：按主键索引列顺序复制 Datum，应用前缀截断后编码 common handle。
- `dataToStrings`：把键列 Datum 转换为重复键错误中的字符串。
- `getOldRow`：读取并恢复已有记录，处理缺失非 public 列和 public 虚拟生成列。

## 执行流程

1. `getKeysNeedCheck` 遍历 `writable_indices`，只统计同时 writable 且 unique 的索引，用于 `unique_keys` 容量预分配；此计数不改变筛选结果。
2. 若 `table_pk_is_handle`，从全列中找到第一个 PK handle 列；若为 common handle，则取得 primary index，并按 `primary_index_columns` 收集组成列。普通无显式 handle 的表允许 handle 列为空。
3. 对每一行调用 `getKeysNeedCheckOneRow`。函数首先通过 `partition_for_row` 定位物理分区。普通成功继续；可被 `handle_partition_error` 接受的错误产生一个 `ignored` 项并立即返回；不可接受错误向上传播。
4. common handle 经 `buildHandleFromDatumRow` 编码；integer handle 从首个 handle 列取 `i64`；没有 handle 列则不产生 record key。存在 handle 时，用实际表/分区编码 record key，并尽可能生成 `表名.PRIMARY` 错误。字符串化失败时先尝试 `handle_data` 回退，再失败则记录日志并使用通用重复键错误。
5. 记录 `original_length`，遍历 `writable_columns`：change-state 且非 public 的列由依赖列转换后追加；其他非 public 且 offset 超出现有行长的列追加原始默认值。这些值只服务于随后的索引取值。
6. 遍历可写索引，跳过非 writable、非 unique、common handle 的 primary index，以及部分索引条件不满足者。对剩余索引获取列值并生成一个或多个键，只保留 `distinct` 键；非 public 且 backfill 适用时转换为 temporary index key；每个键配上 `表名.索引名` 重复错误。
7. 索引处理完成后把行截回 `original_length`，再以 `ignored = false`、`table = Some(实际表或分区)` 推入结果。批量入口按输入顺序保留结果顺序。
8. 独立的 `getOldRow` 用 record key 做事务点查，以 writable columns 解码原始行及实际存在的列 ID；对存储中不存在且当前为 NULL 的非 public 列补默认值，对每个 public generated column 推进表达式下标，仅为 virtual（非 stored）列求值、转换并写回。

## 数据与状态

本文件本身不持有全局或跨调用状态。批量状态由返回的 `Vec<ToBeCheckedRow<...>>` 表达；单行状态包括是否忽略、实际物理表、record key 和零到多个 unique key。输入顺序及索引生成器返回顺序均被保留。

`row` 在单行处理中暂时变长，这是重要不变量：索引值提取期间可见 DDL 过渡列，但成功路径在保存结果前必须由 `row_truncate` 恢复到进入函数时的长度。若补列或索引处理返回错误，函数直接返回 `Err`，局部拥有的行随栈释放，不会把半成品结果推入调用者的 `result`。

`getOldRow` 的 `present_column_ids` 区分“真实存储 NULL”与“旧编码中根本没有此列”，只有后一种情况才补默认值。`generated_index` 按所有 public generated columns 推进，包括 stored generated column；但只对 virtual generated column消费对应表达式进行求值，这要求调用者提供的 `generated_expressions` 与 Go 侧列顺序契约一致。

## 依赖与调用关系

RustCodeGraph 给出的文件内主链为 `getKeysNeedCheck → getKeysNeedCheckOneRow → buildHandleFromDatumRow/dataToStrings`；`getKeysNeedCheckOneRow` 还直接调用 `BatchCheckerRuntime` 的分区、列、索引和键编码能力。`getOldRow` 是另一条独立链，向下依赖事务读取、raw row 解码、默认值和 generated expression 接口。

Rust 上游目前只有 `pkg/executor/lib.rs` 的模块公开，没有发现文件外调用者或 `BatchCheckerRuntime` 实现。因此，`pkg/executor/Cargo.toml` 虽声明了 executor 所需的 table、kv、types、expression、sessionctx 等 crate，目标文件仍未直接 import 它们，而是以关联类型和 trait 方法建立可替换边界；`nextgen` feature 也没有在本文件内形成条件编译分支。

Go 生产调用链是直接证据：`insert_common.go::batchCheckAndInsert`、`insert.go::batchUpdateDupRows`、`replace.go::exec` 调用 `getKeysNeedCheck`；`insert.go::updateDupRow` 与 `insert_common.go::removeRow` 调用 `getOldRow`。RustCodeGraph 对 Go 符号也识别出相同调用边。这些 Go 调用说明算法在完整 SQL 写链中的预期位置，但不证明 Rust 版本已经接入该链。

## 错误处理与边界

所有可能失败的环境操作均返回统一的 `R::Error` 并用 `?` 原样传播，包括分区定位、类型转换、默认值、部分索引条件、索引值/键生成、common handle 编码、事务读取、行解码和生成列计算。

分区错误是唯一可转成成功结果的分支：`partition_for_row` 返回错误后，只有 `handle_partition_error` 返回 `Ok(())` 才生成 `ignored` 行；否则仍失败。trait 注释把“哪些错误可忽略”的判断留给 ErrCtx 适配层，算法不能吞掉任意分区错误。

record key 的错误文本具有降级策略：原始主键 Datum 格式化失败后尝试从 handle 解码；仍失败会记录失败并使用 `generic_duplicate_error`。unique index 的 `dataToStrings` 没有通用错误回退，失败即中止整行处理，与 Go 实现一致。

调用者/实现者必须维持若干前置条件：common handle 表应能提供 primary index 及合法列 offset；`primary_index_columns` 不能越界；行应包含 handle/dependency 所需 offset；`generated_expressions` 应覆盖按顺序出现的 public generated columns。源码使用直接索引而非显式边界错误，这些契约被破坏会触发 Rust panic，而不是 `R::Error`。此外，临时补列只在完整成功路径显式截断；这是安全的所有权行为，但若未来改为借用调用方行，必须增加作用域清理保证。

## 并发与资源生命周期

文件没有线程、异步任务、锁、channel 或静态可变状态。所有环境对象都由调用者传入：`runtime`、`context` 和 `transaction` 采用独占可变借用，编译期阻止同一调用期间并发修改；table、row、key 等由关联类型决定所有权。

批处理是顺序执行的：逐行、逐索引、逐生成键处理，遇到首个错误即停止。容量预分配减少 Vec 扩容，但没有跨行并行化。事务生命周期完全属于调用者；`getOldRow` 只在传入事务上进行一次 record-key 读取，不提交、不回滚，也不缓存结果。实际快照、一致性和锁语义由 `transaction_get` 的实现负责。

## 与 Go 版本的对应关系

Rust 的四个函数与 `pkg/executor/batch_checker.go` 的同名函数一一对应，结构和分支基本保持：handle 列发现、分区错误交给 ErrCtx、common/integer handle、重复键错误降级、DDL 过渡列补齐、partial/unique/distinct 索引筛选、temporary index key、字节错误文本格式化，以及旧行默认值与虚拟生成列恢复均被保留。

类型表达存在刻意差异：Go 直接依赖 `sessionctx.Context`、`table.Table`、`kv.Transaction`、`types.Datum` 等具体类型；Rust 用 `BatchCheckerRuntime` 的关联类型和方法抽象这些设施。Go 的指针/`nil` 分别映射为 Rust 的所有权值与 `Option`，Go 的 `[]*keyValueWithDupInfo` 映射为 `Vec<KeyValueWithDupInfo<...>>`。

需要特别注意两点现状差异。第一，Go 版本已经在写执行器中生产使用，Rust 版本没有 runtime 实现和外部调用；所以这里只能确认算法移植，不能确认端到端可运行。第二，Go 用 `extraColumns` 数量回切 slice，Rust记录完整 `original_length` 后截断，语义等价且对追加列数不敏感。Rust 对 integer handle 的错误文本统一调用 `dataToStrings`，而 Go 首次直接 `ToString`；在常规标量值上结果应相同，二进制主键格式的边缘兼容性应在接线测试中专门核对。

相关 Go 行为测试没有直接调用 batch checker 私有函数，而是经 SQL 写路径间接覆盖。例如 `pkg/executor/test/seqtest/seq_executor_test.go` 覆盖批量 INSERT/`ON DUPLICATE KEY UPDATE`，`pkg/executor/executor_failpoint_test.go` 包含带虚拟生成列和主键的写入场景。仓库未发现同名独立 Rust 测试；现有 Rust SQL 测试中的 duplicate-key 场景也不能证明本模块被调用。

## 扩展指南

- 接入真实 Rust 写路径时，应在独立生产文件实现 `BatchCheckerRuntime`，把每个方法映射到 canonical table/kv/session API；不要在本文件增加“返回成功/空集合”的默认桩。随后让 INSERT、REPLACE 和 duplicate-update 三条链显式调用这些入口。
- 新增句柄或索引编码规则时，优先修改 `buildHandleFromDatumRow`、`getKeysNeedCheckOneRow` 与 runtime 的编码方法；必须保持 record key、temporary index key、partial index 与 multi-valued key 的 distinct 语义一致。
- 修改 DDL 过渡列处理时，保持“补列仅用于索引取值、输出行恢复原长度”的不变量；相应测试应覆盖 change-state 列、缺失 write-only 默认值和错误中途返回。
- 修改错误文本时，应同时核对 `dataToStrings`、handle 回退和 `duplicate_error` 的表/索引命名。重点兼容用例包括尾部 NUL 字节、全 NUL、bit/binary literal、common handle 前缀列，以及字符串化失败后的通用错误。
- 扩展 `getOldRow` 时，必须区分缺失列和显式 NULL，并维持 generated expression 与 public generated column 的顺序契约；stored 列不应重新求值。
- 测试逻辑应放在同目录独立文件（例如新增 `pkg/executor/batch_checker_test.rs` 并从 `lib.rs` 以 `#[cfg(test)] mod batch_checker_test;` 装配），不得内嵌到生产源。至少需要 fake runtime 的函数级测试，以及真实 runtime 接线后的 INSERT/REPLACE/duplicate-update 集成测试。
- 性能风险集中在重复元数据遍历、每个索引键重复格式化 `values`、`present_column_ids.contains` 的线性查找和批量流程串行化；优化前应以行为测试锁定 Go 兼容性，不能用简化索引/DDL 分支换取性能。

## 验证依据

- Rust 源码：`pkg/executor/batch_checker.rs`，核对了 4 个数据类型/trait 与 5 个公开函数的完整实现；关键位置为 `BatchCheckerRuntime`、`getKeysNeedCheck`、`getKeysNeedCheckOneRow`、`buildHandleFromDatumRow`、`dataToStrings`、`getOldRow`。
- 模块与 crate：`pkg/executor/lib.rs` 的 `pub mod batch_checker`；`pkg/executor/Cargo.toml` 的 `astersql-executor` 包、`lib.rs` 入口、`nextgen` feature 和 executor 依赖声明。
- RustCodeGraph：`status` 显示索引包含目标文件；`files --filter pkg/executor/batch_checker.rs` 显示 64 个符号；`explore` 确认文件内调用链，并显示 Rust 入口无外部覆盖测试。精确查询同时区分了 Go 与 Rust 的同名函数。
- Go 对照：`pkg/executor/batch_checker.go`，逐分支核对同名算法；`pkg/executor/insert_common.go`、`pkg/executor/insert.go`、`pkg/executor/replace.go` 提供生产调用边。
- 测试搜索：对 `pkg/executor/**/*_test.go` 与 `*_test.rs` 搜索入口符号未发现直接函数级测试；阅读搜索结果确认 `pkg/executor/test/seqtest/seq_executor_test.go` 的批量 duplicate-update 场景及 `pkg/executor/executor_failpoint_test.go` 的生成列写入场景仅作为 Go 写路径的间接行为证据。
- 本任务是纯文档分析，依计划不运行 Cargo。结构验证要求目标文档存在，且恰好包含本页 11 个固定二级标题；此外人工复核了当前接线状态、错误边界、扩展入口和测试缺口均有源码或调用搜索依据。
