# `pkg/executor/foreign_key.rs`

## 文件定位

`foreign_key.rs` 属于 `astersql-executor` crate，并由 `pkg/executor/lib.rs` 以公开模块 `pub mod foreign_key` 暴露。它把外键 DML 处理拆成两条能力链：`FKCheckExec` 负责在子表写入时确认父键存在、在父表更新或删除时确认没有子表引用；`FKCascadeExec` 负责把父行变化转换成对子表的级联 `DELETE` 或 `UPDATE`。文件还定义了事务、编码、优化和执行所需的运行时 trait，使核心算法不直接依赖具体会话或 TiKV 类型。

当前接线状态必须与设计意图分开看：仓库内 Rust 调用搜索显示，除本文件自调用外，只有 `pkg/executor/test/fktest/foreign_key_test.rs` 直接导入本模块的 AST 与统计类型；尚未发现 Rust DML 主链构造或调用 `FKCheckExec`、`FKCascadeExec`。实际应用主链仍可在 Go 对照 `pkg/executor/foreign_key.go` 及其调用方 `builder.go`、`write.go`、`delete.go`、`insert_common.go`、`adapter.go` 中确认。因此该 Rust 文件是公开、可测试且具备完整算法骨架的移植模块，但不能仅凭当前源码宣称已接入 Rust SQL 执行主链。

## 核心职责

1. 用 `buildTblID2FKCheckExecs`、`buildFKCheckExecs`、`buildFKCheckExec` 按表和检查规格构造检查器，并把列名解析为行内偏移。
2. 用 `FKCheckExec::{insertRowNeedToCheck,updateRowNeedToCheck,deleteRowNeedToCheck}` 收集需要验证的外键值；`NULL` 值和重复值会由 `fkValueHelper` 跳过。
3. 将外键值编码为精确记录/索引键或索引前缀键，通过 `FKTransaction` 批量预取、点查或“内存缓冲优先、快照兜底”的前缀扫描确认约束。
4. 对存在性检查命中的父记录收集锁键；悲观事务延迟交给统一加锁阶段，非悲观路径调用运行时锁接口并恢复 `ForUpdateFlag`。
5. 为 `INSERT IGNORE` 一类路径提供 `checkRows`/`checkFKIgnoreErr`：约束失败不立即终止，而是标记行为忽略并追加 warning；基础设施错误仍通过 `FKResult` 返回。
6. 用 `FKCascadeExec` 聚合父行删除/更新产生的旧值与新值，按每批最多 1024 组拆分，生成带可选 `USE INDEX` 的级联语句，再交给 `FKCascadeRuntime` 优化和构造执行器。
7. 提供 `FKCheckRuntimeStats`、`FKCascadeRuntimeStats` 的展示、克隆、合并和类型标识，记录检查、锁、总耗时与处理键数。

## 主要符号

- `Datum`：本模块的简化值类型，区分 `Null`、有符号/无符号整数、字节串和字符串；`encodeDatums` 以类型标签、长度和值编码为稳定去重键。
- `ColumnInfo`、`IndexInfo`、`TableInfo`：检查与级联需要的最小元数据。`TableInfo.record_prefix` 和 `common_handle` 决定记录键与聚簇主键处理。
- `FKCheckSpec`：携带检查方向、列、目标表/索引、索引是否为主键/独占、检查存在还是不存在，以及对用户返回的约束错误。
- `FKCascadeSpec`、`FKInfo`、`FKCascadeType`、`ReferOption`：描述 ON DELETE/UPDATE 的触发方向与 `CASCADE`/`SET NULL` 等动作。
- `WithForeignKeyTrigger`：执行器侧统一访问检查器与级联器的接口；本仓库 Rust 搜索尚未找到实现者。
- `FKTransaction`：抽象事务点查、批量预取、内存/快照前缀扫描、扫描批大小和悲观事务判定。
- `FKCheckRuntime`：抽象事务获取、索引/句柄/记录键编码、锁管理、统计注册和 warning 输出。
- `FKCheckExec`：持有规格、值提取器、待点查键、待前缀查键、待锁键、逐行检查缓存和统计。
- `fkValueHelper`：按列偏移提取值，跳过含 `NULL` 的组合，并在一个执行器内去重。
- `ToBeCheckedRow`、`fkCheckKey`：分别表示可被忽略的输入行和检查键的精确/前缀形态。
- `FKCascadeRuntime`、`CascadePlan`、`CascadeExecutor`：把简化级联语句连接到优化器与执行器的抽象边界。
- `FKCascadeExec`：保存待处理值、按编码后新值有序分组的更新映射、已优化计划与统计。
- `DeleteStmt`、`UpdateStmt`、`WhereCondition`、`TableRefsClause`、`Assignment`：模块内部可测试的简化级联 AST。
- `GenCascadeDeleteAST`、`GenCascadeSetNullAST`、`GenCascadeUpdateAST`：生成单列或复合列 `IN` 条件及更新赋值的公开辅助函数。
- `FKCheckRuntimeStats`、`FKCascadeRuntimeStats`、`FKRuntimeStats`：运行时统计及同类合并协议；`Tp` 分别返回 1 和 2。

## 执行流程

检查链从构造开始。`buildFKCheckExec` 调用 `getFKColumnsOffsets` 做不区分大小写的列名匹配，把 `FKCheckSpec.table` 更新为当前表，并初始化所有集合与缓存。插入时，父表引用方向无需检查；更新时先比较新旧外键值，未变化则退出，子表方向检查新值、父表方向检查旧值；删除则检查被删除行。`addRowNeedToCheck` 经 `fetchFKValuesWithCheck` 跳过 `NULL` 和重复组合，然后由 `buildCheckKeyFromFKValue` 选择记录主键、普通索引或前缀索引路径。

`doCheck` 获取事务后，先由 `checkKeys` 对精确键执行 `BatchGet` 预取再逐键 `Get`，然后由 `checkIndexKeys` 检查前缀键。前缀检查临时把快照扫描批大小设为 2，结束后恢复为 256；`getIndexKeyValueInTable` 先查看事务内存缓冲，非空值立即命中，空值记为已删除，再扫描快照并跳过这些删除键。要求存在时，命中的索引项会转换为要锁的记录键；common handle 主索引可直接锁命中的键。要求不存在时，任何存活值都会返回规格中的约束错误。

检查完成后，没有锁键则直接结束。悲观事务通过 `AddUnchangedKeysForLock` 延迟加锁；其他事务调用 `LockKeys`，并在调用后恢复原来的 `ForUpdateFlag`。启用统计时，无论检查结果成功与否，`doCheck` 都更新总耗时和键数并注册；锁耗时只在即时锁路径记录。

逐行忽略链由 `checkRows` 驱动。它为所有未忽略且不含 `NULL` 的行构建键，只批量预取精确键；随后按行检查，并以键为单位缓存是否应忽略。重复失败键复用缓存、标记行并再次追加 warning。`checkFKIgnoreErr` 创建激活事务，用单行数组依次跑所有检查器，最终返回该行是否被忽略。

级联链由 `onDeleteRow` 或 `onUpdateRow` 收集数据。删除保存去重后的旧值；更新的 `SET NULL` 也保存旧值，而 `CASCADE` 用编码后的新值作为 `BTreeMap` 键，将多个旧值聚合到同一 `UpdatedValuesCouple`。`buildFKCascadePlan` 每次最多取 1024 组旧值：删除 CASCADE 生成 `DeleteStmt`，删除/更新 SET NULL 生成赋 `NULL` 的 `UpdateStmt`，更新 CASCADE 生成赋新值的 `UpdateStmt`。随后 `FKCascadeRuntime::Optimize` 生成计划，`buildExecutor` 再构造执行器并保留计划所有权；调用方需要重复调用直至返回 `None`，才能耗尽多批数据。

## 数据与状态

`FKCheckExec` 的三个键队列代表不同阶段：`toBeCheckedKeys` 是可点查的完整键，`toBeCheckedPrefixKeys` 是非独占索引等情况下需要扫描的前缀，`toBeLockedKeys` 是检查成功后必须保护的父记录。它们不会在 `doCheck` 内清空，因此实例应按一条执行生命周期管理，不能把一次检查后的对象无条件复用于无关语句。

`fkValueHelper.fkValuesSet` 用 `encodeDatums` 的类型化字节串去重。类型标签避免整数、字符串和字节串发生跨类型碰撞，长度前缀避免相邻值拼接歧义。任一外键列为 `Datum::Null` 时整组不参与匹配，符合 Go `hasNullValue` 路径。

`checkRowsCache: HashMap<Key, bool>` 中的布尔值表示该键是否失败并应忽略，而不是“键是否存在”。缓存只按编码键区分，没有把 `isPrefix` 放入键；同一检查器构造出的相同键语义固定，因此当前用法成立。

`FKCascadeExec.fkValues` 保存删除或 SET NULL 的旧值；`fkUpdatedValuesMap` 以新值编码为键保存“一个新值对应多组旧值”。使用 `BTreeMap` 令 Rust 的取批次顺序稳定；Go 对照使用 map，顺序未承诺。`CascadePlans` 保留已优化计划，保证构建出的执行器所依赖的计划生命周期不早于级联执行器。

统计对象是执行器内可选状态。检查统计包含 `Total`、`Check`、`Lock`、`Keys`，级联统计包含 `Total`、`Keys`；`Merge` 只接受相同枚举变体，异类统计被忽略。

## 依赖与调用关系

crate 边界由 `pkg/executor/Cargo.toml` 定义：包名为 `astersql-executor`，库入口是 `lib.rs`；本文件源码直接使用的外部 crate 只有 `astersql-errors`，其余依赖通过 `std` 集合、同步引用计数和时间类型完成。更重的事务、表编码、优化器和执行器依赖被隔离在 `FKTransaction`、`FKCheckRuntime`、`FKCascadeRuntime` 三组 trait 后面。

RustCodeGraph 对精确符号确认了本文件的内部边：`buildTblID2FKCheckExecs -> buildFKCheckExecs -> buildFKCheckExec`；`doCheck -> checkKeys/checkIndexKeys -> checkKey/checkPrefixKey`；`buildExecutor -> buildFKCascadePlan -> GenCascade*AST -> genWhereConditionAst`。精确仓库搜索没有发现本文件以外的 Rust 生产调用者；直接外部消费者是 `pkg/executor/test/fktest/foreign_key_test.rs`，其导入 AST、Datum 和统计类型。`pkg/executor/foreign_key_test.rs` 作为 crate 内独立单元测试，仅导入两种统计类型。

Go 生产调用链提供了移植意图的直接证据：`builder.go` 构造 `FKCheckExec`；`insert_common.go`、`write.go`、`delete.go` 收集插入/更新/删除行；`write.go` 与 `delete.go` 调用忽略检查；`adapter.go` 调用 `doCheck` 并反复构建/执行级联执行器。以上是 Go 当前事实，不应误写成 Rust 已完成接线。

## 错误处理与边界

所有可失败 Rust 路径统一返回 `FKResult<T> = Result<T, errors::SharedError>`。未知列、缺失索引元数据、错误的整型主键值、行偏移越界、单列/多列值宽度不匹配、空级联列集合、缺失更新值，以及与触发类型不相容的引用动作都有显式错误。事务获取、读、扫描、编码、解码、锁、优化和执行器构造错误由 `?` 原样传播。

约束违反与基础设施错误在 `checkRows` 中被有意折叠：任何 `checkKey`/`checkPrefixKey` 错误都会令该行 ignored，并追加规格中的 `failed_error` warning；预取、值提取和键构造错误则仍直接返回。此行为对应 Go 的 INSERT/DELETE IGNORE 路径，但新增错误类型时必须确认是否也应被降级为 warning，避免吞掉存储或解码故障。

前缀扫描依赖“空 value 表示事务内删除”的约定，并用内存缓冲遮蔽快照旧值。扫描批大小在正常 `Result` 返回路径恢复为 256，但如果未来在设置后引入 panic，trait 本身没有 RAII 恢复保证。`checkPrefixKeyExist` 要求 key/value 同时存在且 value 非空；普通索引还必须提供 `IndexInfo` 才能解码句柄。

级联每批上限是常量 `MAX_HANDLE_FK_VALUE_IN_ONE_CASCADE = 1024`。生成器拒绝零列和行宽不匹配；`buildFKCascadePlan` 拒绝 RESTRICT/NO ACTION 被误送入级联执行器。深度限制、权限检查、事务过大与提交重试不在本文件中实现，它们属于上层执行与会话边界，Go 集成测试覆盖这些整体行为。

## 并发与资源生命周期

`FKTransaction: Send`，两个运行时 trait 均为 `Send + Sync`，执行器通过 `Arc<dyn ...>` 共享运行时；但是 `FKCheckExec` 和 `FKCascadeExec` 持有可变队列、缓存与统计，方法需要 `&mut self`，设计上应由单个语句执行流独占，而不是跨线程并发修改。

锁生命周期由运行时决定。悲观事务只登记 unchanged keys，等待上层统一获取排他锁；非悲观路径立即调用 `LockKeys`，并保护性地保存/恢复 `ForUpdateFlag`。前缀扫描借用事务对象，Rust trait 返回拥有所有权的键值集合，因此本模块没有显式迭代器关闭动作；Go 对照则用 `defer Close` 管理内存与快照迭代器。

级联计划使用 `Box<dyn CascadePlan>`，执行器使用 `Box<dyn CascadeExecutor>`。计划在交给 `BuildExecutor` 后被推入 `CascadePlans`，防止其过早释放。`buildExecutor` 只构建而不调用 `CascadeExecutor::Execute`，执行、提交、重试和级联深度控制必须由上层负责。

## 与 Go 版本的对应关系

Rust 基本按 `pkg/executor/foreign_key.go` 的结构移植：检查器/级联器、NULL 跳过与去重、精确键和前缀键分流、mem-buffer 优先扫描、悲观事务延迟锁、1024 条分批、级联 AST、统计字符串/Clone/Merge/Tp 都有对应符号。`pkg/executor/test/fktest/foreign_key_test.go` 的 `TestForeignKeyGenerateCascadeAST`、外键检查/锁、DELETE/UPDATE CASCADE、SET NULL、大批量值与统计测试说明了 Go 行为边界；Rust 对应测试文件保留了同名 AST 和大量 SQL 场景。

关键差异包括：Rust 用简化 `Datum` 和自有 AST，而 Go 直接使用 `types.Datum`、parser AST、真实表/会话/事务；Rust 用 runtime trait 隔离这些对象。Go 更新值比较调用带排序规则的 `Datum.Compare`，Rust `updateRowNeedToCheck` 使用派生的严格 `PartialEq`，字符串排序规则等价性尚未由本模块表达。Go 的上下文支持取消并显式检查 `ctx.Done()`，Rust trait 没有取消令牌。Go 将扫描批大小恢复为 `txnsnapshot.DefaultScanBatchSize`，Rust 固定恢复为 256。Go 的级联更新 map 迭代无序，Rust `BTreeMap` 有序。

Go `GenCascadeUpdateAST` 为值表达式附带列 `FieldType`，Rust 简化 AST 只保存 `Datum`；真实类型推导依赖未来 `FKCascadeRuntime::Optimize` 适配。更重要的是，Go 生产 DML 已完整接线，而 Rust 生产调用搜索目前没有发现检查器/级联器消费者；因此 Rust 的 SQL 集成测试通过不等同于这些抽象已成为 Rust 主链唯一实现。

## 扩展指南

- 接入 Rust DML 主链时，应从 Go 的五个接点逐一对齐：builder 构造、insert 收集、write/update 收集、delete 收集、adapter 检查与循环执行级联。实现 `WithForeignKeyTrigger`、`FKCheckRuntime`、`FKTransaction`、`FKCascadeRuntime` 时避免复制本文件算法。
- 新增 Datum 类型或修改去重语义时，同步修改 `Datum`、`encodeDatums`、运行时编码实现，并增加独立测试验证跨类型、复合值和排序规则；不要把测试嵌入本源文件。
- 修改检查键选择时，重点覆盖整数主键、common handle、唯一/非唯一索引、外键仅占索引前缀以及事务内删除遮蔽快照等分支；直接测试位置应为 `pkg/executor/foreign_key_test.rs`，端到端行为位置为 `pkg/executor/test/fktest/foreign_key_test.rs`。
- 扩充级联动作时，同时更新 `ReferOption` 分派、AST 生成、优化运行时和 Go 对照语义。保持每批 1024 的性能保护，或用基准与大事务证据证明新上限安全。
- 为 `checkRows` 增加新错误时，先区分“约束失败可忽略”和“基础设施错误必须返回”；当前统一降级检查错误的行为存在兼容风险。
- 修改统计格式时同步 `formatDuration`、两种 `String`、Clone/Merge/Tp，并更新 `pkg/executor/foreign_key_test.rs` 与 fktest 的统计断言。
- 兼容性风险主要是排序规则、错误文本、warning 数量、锁时序与 AST 类型信息；性能风险主要是前缀扫描、缓存增长、待锁键增长和级联批大小；正确性风险主要是 NULL、事务内删除、复合键宽度及多批级联是否被上层完整耗尽。

## 验证依据

- Rust 源码：`pkg/executor/foreign_key.rs`，已通过 RustCodeGraph `node --file` 阅读全部 1195 行；主要证据为 `FKCheckExec`、`FKCascadeExec`、三个 runtime trait、`fkValueHelper`、`GenCascade*AST` 和统计实现。
- crate 与模块：`pkg/executor/Cargo.toml` 确认包名、`lib.rs` 入口、`astersql-errors` 依赖与 `nextgen` feature；`pkg/executor/lib.rs` 确认 `pub mod foreign_key` 及独立 `foreign_key_test`。
- RustCodeGraph：`status` 显示索引包含 7032 个 Rust 文件和 4415 个 Go 文件；`query FKCheckExec/FKCascadeExec/buildFKCheckExec/checkFKIgnoreErr/GenCascadeUpdateAST` 定位 Rust/Go 对应符号；`node --file` 用于读取 Rust、Go 与测试源码。图的 `callers/callees` 命令未返回可用精确边，因此调用边又用限定 Rust/Go 文件的 `rg` 搜索核验，并在本文明确区分 Rust 自调用、测试消费者和 Go 生产调用者。
- Go 对照：`pkg/executor/foreign_key.go` 全部 1017 行；生产入口证据来自 `pkg/executor/builder.go`、`write.go`、`delete.go`、`insert_common.go`、`adapter.go` 的精确调用搜索。
- 测试：`pkg/executor/foreign_key_test.rs` 覆盖统计持续时间精度；`pkg/executor/test/fktest/foreign_key_test.rs` 直接覆盖简化 AST、SQL 外键行为、并发锁与级联；Go 对照为 `pkg/executor/test/fktest/foreign_key_test.go`，覆盖索引形态、NULL、检查与锁、CASCADE/SET NULL、1024 分批相关大数据场景、统计、权限、重试与错误边界。
- 本任务是纯文档分析，按计划未运行 Cargo，也未修改 Rust、Go、Cargo 或总计划。交付前执行任务指定的 11 章节结构检查，并人工复核本文没有把 Go 主链或未来 runtime 接线误写为当前 Rust 已支持能力。
