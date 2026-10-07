# `pkg/executor/write.rs`

## 文件定位

`write.rs` 是 `astersql-executor` crate 中的 DML 写路径公共算法模块。crate 根由 [`pkg/executor/Cargo.toml`](Cargo.toml) 的 `[lib] path = "lib.rs"` 指定，[`pkg/executor/lib.rs`](lib.rs) 通过 `pub mod write` 公开本模块，并只在 `cfg(test)` 下挂载独立测试 [`pkg/executor/write_test.rs`](write_test.rs)。文件对应 Go 实现 [`pkg/executor/write.go`](write.go)，主要抽取 UPDATE/`ON DUPLICATE KEY UPDATE` 单行写回过程中可共享的比较、自动值回填、外键/分区校验和统计顺序。

当前 Rust 模块是“泛型算法 + 运行时边界”，不是已经接入具体 session/table/transaction 的完整生产实现：`WriteRuntime` 把所有 TiDB/AsterSQL 实体能力抽象为关联类型与方法。仓库搜索只找到 `write_test.rs` 对该 trait 的实现和对 Rust `updateRecord` 的直接调用；RustCodeGraph 虽给出若干文件级使用者，但精确符号搜索没有找到生产 runtime 或生产调用边。因此本文件目前可验证为公开、可单测的写行协议，不能据此声称 Rust SQL UPDATE/INSERT 已经通过它落到存储。

## 核心职责

- `WriteRuntime` 定义列元数据、Datum 比较/转换、赋值求值、事务模式、表写入、外键、分区及 statement 统计的依赖边界。
- `updateRecord` 按 Go 版顺序编排一行更新：比较非生成列、处理无变化行、刷新 `ON UPDATE CURRENT_TIMESTAMP`、求值赋值/生成列、处理 NULL/外键/交换分区、选择 replace 或 update、最后执行外键动作并记账。
- `addUnchangedKeysForLockByRow` 为悲观事务的未变化键加锁提供统一门控；具体 row key/unique index key 生成由 runtime 实现。
- `rebaseAutoRandomValue` 提取 AUTO_RANDOM 的增量位并回填 allocator 水位。
- `DataTooLongError`/`resetErrDataTooLong` 构造带列名与行号的 MySQL 风格错误文本。
- `checkRowForExchangePartition` 是交换分区落点检查的薄入口。

## 主要符号

- `lockRowKey = 1`、`lockUniqueKeys = 2`：传给未变化键锁定逻辑的位标志，可组合为 `3`。二者公开，但采用与 Go 一致的非大写命名，文件用 `#![allow(non_snake_case, non_upper_case_globals)]` 保留迁移命名。
- `WriteRuntime`：本文件唯一 trait。它有 `Context`、`Datum`、`Column`、`Assignment`、`Handle`、`DuplicateKeyMode`、`ForeignKeyCheck`、`ForeignKeyCascade`、`Error` 九个关联类型。方法按职责分为列属性/比较、AUTO_INCREMENT/AUTO_RANDOM、未变化键锁与 statement 计数、当前时间和赋值缓冲、NULL/FK/交换分区校验、replace/update 存储写回以及更新后的 FK 检查/级联。trait 没有默认生产实现。
- `updateRecord<R, H>(...) -> Result<(bool, bool), R::Error>`：核心公开函数。第一个返回值表示真正完成写入，第二个表示 `IGNORE` 因外键检查跳过本行；闭包 `H` 接收赋值项、原始值和可选求值/转换错误，决定错误是返回、降级还是修正。
- `addUnchangedKeysForLockByRow`：仅当 `in_pessimistic_transaction()` 且 `key_set != 0` 时委托 runtime 收集锁键，否则返回 `0`。
- `rebaseAutoRandomValue`：表无 AUTO_RANDOM 或记录 id 为负时不操作；否则以 `auto_random_incremental_mask` 清除非增量位，再调用 `rebase_auto_random`。
- `DataTooLongError { column_name, row_index }`：可克隆、比较并实现 `Display`/`Error` 的值对象；显示文本固定为 `Data too long for column '<name>' at row <index>`。
- `resetErrDataTooLong`：构造上述错误，不接收也不保存原始错误链。
- `checkRowForExchangePartition`：直接转发到 `WriteRuntime::check_exchange_partition_row`，不自行查 infoschema 或解析分区定义。

文件没有条件编译项、异步函数、宏或其他类型实现。

## 执行流程

`updateRecord` 的输入行长度必须与 `runtime.columns()` 一致，`modified` 也必须逐列对应；函数一开始用三个 `assert_eq!` 强制该不变量，违反时会 panic。

1. 第一遍遍历列时跳过所有生成列。对 `ON UPDATE` 列记录它是否未被显式修改；随后用 `compare_binary` 比较新旧 Datum，并覆盖调用方传入的 `modified[index]`。不同值会把 `changed` 置真；自增列回填 AUTO_INCREMENT，整型主键 handle 同时标记 `handle_changed` 并尝试回填 AUTO_RANDOM，common handle 列也标记 handle 变化。
2. 若第一遍完全无变化，函数增加 touched 行；开启 `CLIENT_FOUND_ROWS` 时也增加 affected 行。随后请求锁 row key，并按 `lock_unchanged_keys` 决定是否同时锁唯一键；无论锁到多少键都返回 `(false, false)`，不会进入赋值、外键或存储写回阶段。
3. 已有普通列变化时，所有需要自动刷新的 `ON UPDATE CURRENT_TIMESTAMP` 列由 `current_timestamp` 填值，并同步到可选求值缓冲的 `index + offset`。这类列若被 runtime 标为 pk-is-handle，返回内部一致性错误；若属于 common handle，则触发 replace 路径。
4. 顺序处理 `assignments`。延迟错误优先返回；目标列下标按 `assignment_column_index - offset` 计算。表达式求值失败时用 `zero_datum` 作为传给错误处理器的占位，转换成功才写入 `new_data`；求值缓冲始终写入当前 `new_data`。`error_handler` 成功后重新计算该列的 `modified`、自增水位和 handle 变化。
5. 对旧行中的虚拟生成列执行遗留坏 NULL 检查：旧值为 NULL 且列为 NOT NULL 或禁止 NULL 插入时直接返回 `bad_null_error`。之后，`ignore_error` 模式先调用 FK ignore 检查；若其返回跳过，则立即返回 `(false, true)`，此时不会触碰存储和计数。
6. 每列调用 `handle_bad_null`，再按 `exchange_partition_check_required` 决定是否检查交换分区落点。通过后才增加 touched 行。
7. `handle_changed` 为真时调用 `replace_record_with_staging`，语义是 staged remove-old/add-new；否则调用 `update_record`。仅当既非显式事务、又非 FK trigger、也没有 FK cascade 时，`skip_untouched_indices` 才为真。普通 update 成功后，如配置要求，再收集未变化的唯一键。两类写错误都先经过 `handle_partition_error` 映射。
8. 非 `IGNORE` 模式逐项执行 `foreign_key_update_check`；级联动作不受该条件限制，随后逐项执行。成功结束时 affected 增加 `1`，`ON DUPLICATE KEY UPDATE` 增加 `2`；updated/copied 各增加 `1`，返回 `(true, false)`。

## 数据与状态

本文件不拥有 session、事务或表对象。`old_data` 和 `handle` 按值传入后只借用；`new_data`、`modified` 在函数内部拥有并修改，成功或失败都不返回最终行，持久化与观察由 runtime 方法完成。`columns()` 返回一次快照，之后所有索引操作都假设它与两行及 `modified` 等长。

关键状态变量是 `changed`、`handle_changed` 和 `on_update_needs_modification`。`changed` 在第一遍或赋值重算中只会从 false 变 true，不会因后续赋值恢复旧值而复位；这与 Go 版共享“曾检测到变化即继续写路径”的控制语义。`modified` 则保存每列最后一次比较结果，供底层 update 决定索引维护。`handle_changed` 同样单调为真，用于选择 staged replace。

计数顺序也是外部可见状态：无变化行一定 touched，可选 affected；FK-ignore 跳过发生在 touched 增加之前；真正写成功后才增加 affected/updated/copied。`DataTooLongError` 只是不可变错误数据，没有全局状态。AUTO_RANDOM 水位和未变化锁键集合都由 runtime 持有。

## 依赖与调用关系

直接 Rust 依赖只有 `std::fmt`；`Cargo.toml` 中 executor 的 sessionctx、table、kv、expression、types 等依赖没有被本文件直接导入，而是预期由未来的 `WriteRuntime` 实现使用。crate 唯一 feature `nextgen` 转发到 `astersql-dxf-importinto/nextgen`，本文件没有 feature 分支。

上游装配是 `lib.rs -> pub mod write`。精确仓库搜索确认 Rust 上游只有 `write_test.rs -> updateRecord`；`update.rs` 自己定义另一套 `UpdateRuntime`，目前没有调用这里的 `updateRecord`。RustCodeGraph 对 `updateRecord` 的 callees 验证了它调用本文件的 runtime 方法以及 `addUnchangedKeysForLockByRow`、`rebaseAutoRandomValue`、`checkRowForExchangePartition`；对生产 callers 没有给出可验证的 Rust 调用边。因此生产接线明确标记为未实现/未验证。

Go 上游则已接线：[`pkg/executor/update.go`](update.go) 和 [`pkg/executor/insert.go`](insert.go) 调用 Go `updateRecord`；[`pkg/executor/insert_common.go`](insert_common.go) 还直接调用数据过长错误重写、交换分区检查和未变化键锁函数。Go 下游进一步到 expression 求值、session statement context、事务 mem-buffer staging、table `RemoveRecord`/`AddRecord`/`UpdateRecord`、FK executor、infoschema 与 tablecodec。这些是 Rust trait 设计的对照依据，不是 Rust 已执行这些调用的证据。

## 错误处理与边界

除输入长度断言外，算法型失败统一使用 `R::Error` 并以 `?` 或显式返回传播。比较、allocator rebase、时间生成、赋值、NULL 处理、FK、分区、锁键和存储写都可中止流程。replace/update 错误额外通过 `handle_partition_error` 转译；其他错误不经过该钩子。

赋值求值/类型转换是特殊边界：它们先收集为 `Option<R::Error>`，再交给 `error_handler`，从而支持 Go 中 `IGNORE`/warning 等策略。延迟错误不经过此闭包而直接返回。目标下标使用无检查的 `usize` 减法和向量索引；错误的 `offset`、assignment index 或 runtime 列布局会 panic，调用方必须保证布局契约。

`ON UPDATE` pk-is-handle 被视为不可能状态并返回 runtime 构造的错误，而非 panic；Rust 独立测试专门覆盖这一点。`DataTooLongError` 不保留 Go 注释所说的原始 `types.ErrDataTooLong` 包装链，只有最终列名/行号文本。交换分区函数本身也不区分“表不存在”“不是分区表”“约束失败”，这些必须由 runtime 的错误类型表达。

## 并发与资源生命周期

所有可变操作都要求 `&mut R` 和 `&mut Context`，本文件没有线程、异步任务、锁、通道或 `Send`/`Sync` 约束；安全 Rust 下单次调用按上述顺序串行执行。是否跨 worker 共享 session、allocator、统计或 FK 执行器，以及如何同步，完全属于 runtime 实现责任。

最重要的资源生命周期位于 trait 之后：handle 变化时 `replace_record_with_staging` 必须保证 staging 在成功时 release、失败时 cleanup；Rust 函数只要求这一原子语义，自己不持有 staging guard。未变化锁键只登记到悲观事务上下文，实际加锁/提交/回滚也不在本文件发生。AUTO_INCREMENT/AUTO_RANDOM rebase 可能在最终写入前发生；这与 Go allocator 的非回收水位语义一致，但 Rust 能否保持该语义取决于 runtime。

外键检查和级联以调用方传入的可变 slice 顺序执行，没有并行化。任一项失败立即停止后续项；存储写已经发生在检查/级联之前，因此具体事务层必须提供与 Go 一致的语句级回滚能力。

## 与 Go 版本的对应关系

Rust `updateRecord` 的阶段顺序、两个返回布尔值、`CLIENT_FOUND_ROWS`、`ON DUPLICATE` affected=2、二进制比较、ON UPDATE、生成列赋值、坏 NULL、FK ignore、交换分区、handle replace、未触及索引优化及 FK 后处理，都直接对应 `write.go` 的同名函数。四个辅助入口也保留 Go 命名和核心分支。

但抽象后的语义并非全部由 Rust 文件自身实现：

- Go 直接使用 `sessionctx.Context`、`table.Table`、`kv.Handle`、`types.Datum` 和 `expression.Assignment`；Rust 全部由关联类型替代。
- Go `updateRecord` 创建 tracing region、取得 statement/session variables、事务和悲观 lazy-check mode，并接收 memory tracker；Rust 没有 tracing、memory 参数或事务获取逻辑。
- Go handle 变化路径显式创建 mem-buffer staging，先 remove 再 add，并在成功/失败时 release/cleanup；Rust只调用 `replace_record_with_staging`。
- Go `addUnchangedKeysForLockByRow` 解析分区物理 ID，编码 row key，遍历 public unique index，并处理 global-index V1 的 `PartitionHandle` 与 DDL 新表 ID；Rust辅助函数只做悲观事务/key-set 门控，全部编码细节属于 runtime。
- Go `checkRowForExchangePartition` 从最新 infoschema 找交换目标分区表，检查分区定义并按全局开关检查约束；Rust只是 runtime 委托。
- Go `resetErrDataTooLong` 返回 TiDB 类型化错误；Rust返回独立 `DataTooLongError`，没有错误码和 cause。
- Go 生产调用来自 update/insert；当前 Rust 没有生产 `WriteRuntime`，只有 `write_test.rs` 的最小测试实现。

Go 测试提供迁移目标而非 Rust 覆盖证明：[`pkg/executor/update_test.go`](update_test.go) 覆盖悲观事务未变化键锁、global unique index/NULL 回归和 ON UPDATE + 生成列索引一致性；[`pkg/executor/executor_pkg_test.go`](executor_pkg_test.go) 直接核验分区/global index 的锁键编码。Rust 测试目前只证明 ON UPDATE pk-is-handle 返回错误而不是 panic。

## 扩展指南

要让该模块进入 Rust 生产写链，首要工作是在具体 executor/session 适配层实现 `WriteRuntime`，并从 Rust `update.rs`/insert 路径实际调用 `updateRecord`。实现时应逐项对齐 Go：binary collation、column offset、生成列求值缓冲、allocator rebase、事务 staging、lazy duplicate-key mode、global index/partition handle 编码、statement error context、FK 语句回滚和统计。不能用仅返回成功的桩方法宣称接线完成。

修改比较或赋值顺序时，必须保持 ON UPDATE 与生成列依赖关系，并验证 `changed`、`modified`、`handle_changed` 三者在“值先变后恢复”、多表 offset 和 common handle 场景的行为。修改未变化键逻辑时，应优先在 runtime 层实现 Go 的分区/global-index细节，不把 tablecodec 依赖硬塞回泛型算法。修改错误模型时要决定是否引入错误码/cause，以免只匹配文本。

Rust 测试必须继续放在独立 [`pkg/executor/write_test.rs`](write_test.rs)，不要写回生产文件。建议补齐：完全无变化及 `CLIENT_FOUND_ROWS`、悲观/非悲观锁位组合、AUTO_INCREMENT/AUTO_RANDOM 正负 ID 与掩码、赋值求值/转换/error-handler 分支、FK ignore、坏 NULL、交换分区、replace/update 错误映射、`skip_untouched_indices` 的所有事务/FK组合、affected/updated/copied 计数及 `DataTooLongError`。生产接线后还需移植 Go 的 SQL 级回归，尤其是 global unique index NULL、ON UPDATE 生成列索引和分区交换。兼容风险集中在 MySQL 行计数、warning/IGNORE、错误码与锁键；性能风险集中在每行 `columns()`/向量分配、Datum clone、未触及索引策略和 staging。

## 验证依据

- RustCodeGraph：`status` 确认仓库已有索引；`node --file pkg/executor/write.rs --offset 1 --limit 500` 读取目标文件完整 496 行，并报告文件级使用者；`query` 定位 `WriteRuntime`、`addUnchangedKeysForLockByRow`、`rebaseAutoRandomValue`、`resetErrDataTooLong`、`checkRowForExchangePartition` 的 Rust/Go定义；`callees pkg/executor/write.rs:updateRecord` 验证 runtime 调用和三个辅助函数调用边。由于精确 callers 没有给出生产 Rust 边，本文没有推断生产接线。
- 已读 Rust 文件：[`pkg/executor/write.rs`](write.rs)、[`pkg/executor/write_test.rs`](write_test.rs)、[`pkg/executor/lib.rs`](lib.rs)、[`pkg/executor/Cargo.toml`](Cargo.toml)，并搜索 `pkg/executor` 下所有 Rust `WriteRuntime` 实现和 `updateRecord` 引用；结果只有独立测试实现/调用。
- 已读 Go 对照：[`pkg/executor/write.go`](write.go) 全文件，以及直接调用点 [`pkg/executor/update.go`](update.go)、[`pkg/executor/insert.go`](insert.go)、[`pkg/executor/insert_common.go`](insert_common.go) 的符号搜索结果。
- 已读相关测试：Rust [`pkg/executor/write_test.rs`](write_test.rs)；Go [`pkg/executor/update_test.go`](update_test.go) 的未变化锁与 ON UPDATE/生成列用例，以及 [`pkg/executor/executor_pkg_test.go`](executor_pkg_test.go) 对 `addUnchangedKeysForLockByRow` 的直接断言。
- 人工复核：文档区分了 trait 契约、Rust 已执行逻辑和仅由 Go 证明的迁移目标；未把 RustCodeGraph 粗粒度文件引用当作生产调用；测试建议均指向独立测试文件。
