# `pkg/executor/insert_common.rs`

## 文件定位

`pkg/executor/insert_common.rs` 是 `astersql-executor` crate 中 INSERT/REPLACE 公共语义的 Rust 移植文件，由 [`pkg/executor/lib.rs`](lib.rs) 以 `pub mod insert_common` 导出。它把 Go 版 [`pkg/executor/insert_common.go`](insert_common.go) 中依赖会话、表、事务和表达式包的操作压缩到 `InsertBackend` trait 后面，并在 `InsertValues<B>` 上实现行构造、默认值、自增/自随机 ID、重复键预检、写表和运行时统计等算法。

必须区分“已定义”和“已接线”：仓库搜索没有找到 `InsertBackend` 的具体实现，也没有找到 Rust `insertRows`、`insertRowsFromSelect` 或 `batchCheckAndInsert` 的生产调用。因此这些泛型算法目前是可编译的移植骨架，不是 Rust INSERT 执行器的现行入口。当前有直接生产调用的是 `is_terminal_auto_id_error`（[`pkg/executor/insert.rs`](insert.rs)）和 `evaluate_embedding_inputs`（[`pkg/session/runtime/dml.rs`](../session/runtime/dml.rs)）；错误补全小边界由独立 Rust 测试使用。

crate 边界由 [`pkg/executor/Cargo.toml`](Cargo.toml) 定义：包名为 `astersql-executor`，入口是 `lib.rs`，`nextgen` feature 只透传给 `astersql-dxf-importinto/nextgen`，本文件本身没有条件编译项。源码直接依赖标准库，以及工作区 crate `astersql-meta-autoid` 和 `astersql-expression`；其余数据库能力通过 `InsertBackend` 注入。

## 核心职责

文件承担五组职责。

1. `InsertErrorKind`、`DmlErrorCause`、`CompletedDmlError`、`CompleteInsertErrorForColumn`、`CompleteLoadErrorForColumn` 以及泛型版 `completeInsertErr`/`completeLoadErr` 将底层转换错误补齐列名和行号，并保留 cause。
2. `InsertBackend` 定义 INSERT 算法需要的会话、列、表达式、Datum、内存、事务、外键、ID 分配和统计能力；`InsertValues<B>` 保存一次语句跨行共享的状态。
3. `evalRow`/`fastEvalRow`/`getRow`、`fillRow`、`fillColValue` 和默认值缓存把 VALUES、INSERT…SELECT 或 LOAD DATA 的输入组装为目标表行，并计算普通生成列。
4. `lazyAdjustAutoIncrementDatum`、`adjustAutoIncrementDatum`、`adjustAutoRandomDatum` 和隐式 row-id 方法管理显式值、分配、rebase、重试重放及 `lastInsertID`。
5. `batchCheckAndInsert`、`removeRow`、`addRecordWithAutoIDHint` 实现重复键预检、INSERT IGNORE/REPLACE 分支、外键动作和计数；`InsertRuntimeStat` 汇总耗时。独立的 `evaluate_embedding_inputs` 为会话层的 EMBED_TEXT 生成列提供有界并发计算。

## 主要符号

- `InsertErrorKind`：文件内稳定的错误分类，覆盖过长、溢出、截断、错误值、无默认值、约束冲突和未找到等分支。
- `DmlErrorCause` / `CompletedDmlError`：轻量错误类型；后者的 `source()` 返回原始 cause。`CompleteInsertErrorForColumn` 使用一基行号，`CompleteLoadErrorForColumn` 保留调用方传入的 LOAD 行号并把 `DataTooLong` 改类为 `Truncated`。
- `FieldKind` / `DupKeyCheckMode`：分别抽象列类型类别，以及写表时跳过或执行重复键检查的模式。
- `DuplicateKey<E>` / `ToBeCheckedRow<D,T,E>`：批检阶段的键、预构造冲突错误、目标表、行数据和忽略状态。
- `InsertBackend`：包含关联类型和能力方法的后端边界。方法按职责可分为错误/SQL 模式、表列元数据、表达式与 Datum、警告和默认值、内存与 SELECT 子执行器、事务批次、ID 分配与重试、重复键/外键/写表、运行时统计。
- `InsertValues<B>`：语句状态容器。关键字段包括 `rowCount`/`curBatchCnt`/`maxRowsInBatch`、`lastInsertID`、`Table`、请求列 `Columns`、VALUES 表达式 `Lists`、生成列表达式 `GenExprs`、解析后的 `insertColumns`、默认值与求值缓冲、批量/引用列/额外 handle 标志、内存跟踪器、外键检查/级联及 `ignoreErr`。
- `insertCommon<B>`：要求派生执行器返回其 `InsertValues` 并实现批量 `exec`。`InsertValues` 自身的默认 `exec` 会 panic，明确要求派生类型覆写；但当前仓库没有发现后端实现或派生生产接线。
- 行构造方法：`initInsertColumns`、`initEvalBuffer`、`evalRow`、`fastEvalRow`、`getRow`、`getColDefaultValue`、`fillColValue`、`fillRow`、私有 `rewrite_warnings`。
- ID 方法：`isAutoNull`、`lazyAdjustAutoIncrementDatum`、`adjustAutoIncrementDatum`、`adjustAutoRandomDatum`、`allocAutoRandomID`、`rebaseAutoRandomID`、`adjustImplicitRowID`、`rebaseImplicitRowID`，以及自由函数 `findAutoIncrementColumn`、`setDatumAutoIDAndCast`、`getAutoRecordID`。
- 冲突与写入方法：`collectRuntimeStatsEnabled`、`handleDuplicateKey`、`batchCheckAndInsert`、`removeRow`、`equalDatumsAsBinary`、`addRecord`、`addRecordWithAutoIDHint`。
- 主路径函数：`insertRows` 处理 VALUES/SET，`insertRowsFromSelect` 处理 SELECT 输入。两者均依赖 `insertCommon::exec` 完成真正写入。
- `CreateSession` / `CloseSession`：`OnceLock<RwLock<Option<Arc<...>>>>` 形式的全局可注入辅助会话钩子；本文件只声明，不读取或调用。
- `InsertRuntimeStat<B>`：格式化、克隆、合并基本耗时、快照 RPC 和分配器统计，`Tp` 由后端提供类型标识。
- `evaluate_embedding_inputs`：接收已准备好的 EMBED_TEXT 参数，最多启动 `min(inputs.len(), 800)` 个 scoped 线程，保持结果顺序并把单项错误留在对应位置。

## 执行流程

VALUES/SET 的设计流程从 `insertRows` 开始：先把 `lazyFillAutoID` 置为 true，根据 `allAssignmentsAreConstant` 选择 `fastEvalRow` 或 `evalRow`；每行经过表达式求值、cast、警告改写和 `fillRow`。达到允许的非事务批大小时，先估算并登记内存，再由 `lazyAdjustAutoIncrementDatum` 对连续空自增值批量分配 ID，调用派生执行器的 `exec`，释放内存记账，最后 `doBatchInsert` 提交语句并在当前语句中开启新事务。循环后对尾批重复分配、写入与释放。

INSERT…SELECT 的设计流程由 `insertRowsFromSelect` 驱动：创建子执行器 chunk，先使事务写吞吐 SLI 失效；逐 chunk 拉取 Datum 行，以 `getRow` cast 和补齐目标行，同时保存 SELECT 多出的列。达到批边界时把额外列发布给后端、调用 `exec` 并切换事务；每个 chunk 尾部也刷新剩余行，并对 chunk、目标行和额外列做对称的内存记账。

单行构造集中在 `fillRow`。它补入显式 `_tidb_rowid` 列，跳过生成列，依次调用 `fillColValue` 处理自增、auto-random、显式 row-id 或默认值，再做 bad-null 与 exchange-partition 检查；最后在可变行缓冲上按 `GenExprs` 顺序求值生成列、cast、改写警告并再次检查 null。数组生成列求值失败走 `completeError` 的函数索引错误边界，其他生成列表达式错误交给截断策略。

重复键路径由 `batchCheckAndInsert` 先生成待检键并取得事务，再做外键批检和非临时表唯一索引预取。它先检查 handle key，再检查每个 unique key；临时索引键未命中时转换为正式索引键重查。INSERT IGNORE 追加警告并可登记悲观事务锁键；REPLACE 取冲突 handle 后由 `removeRow` 比较新旧行，完全相同时只维护计数和锁键，否则删除旧记录、触发外键 remove 动作并更新 deleted/affected 计数。无冲突行以 `DupKeyCheckMode::Skip` 写入，因为预检已完成。

生产中的 EMBED_TEXT 路径位于 [`pkg/session/runtime/dml.rs`](../session/runtime/dml.rs)：会话层负责从行和列构造 `EmbedTextArgs`、取得 provider 与取消信号，再调用 `evaluate_embedding_inputs`。该函数用原子索引分配任务；每个 worker 在处理输入（包括 NULL 或参数错误）前检查取消，结果按原索引写入 Mutex 保护的数组。任一取消使整批返回顶层错误；参数/provider 错误只占据单个结果，不取消同批其他任务。

## 数据与状态

`InsertValues` 是语句级可变状态而不是单行对象。`rowCount` 驱动用户可见行号和批边界；`lastInsertID` 只在首次隐式分配时设置，成功写表后由 `addRecordWithAutoIDHint` 发布；重试 ID 队列通过后端读取和追加，以便重放时复用相同 ID。`insertColumns` 保存用户可写列，完整表列仍由后端按需取得；`evalBufferTypes`/`evalBuffer` 保证引用前列或生成列时能看到已计算的值。

`colDefaultVals` 只在多 VALUES 行时惰性分配，且只缓存非表达式默认值；默认表达式每行重新求值。`has_value` 与 Datum 本身分离，因而能区分未提供、NULL 和显式零，这对 `NO_AUTO_VALUE_ON_ZERO`、默认值以及 auto-id 分配是关键不变量。`lazyFillAutoID` 让 VALUES 路径先留 NULL，再对连续空值批分配；显式非零值会 rebase 分配器。

重复键数据使用 `ToBeCheckedRow` 保存所属表，避免分区/目标表信息在预取后丢失。`ignored` 行不会进入后续检查。运行时统计把 `CheckInsertTime`、`Prefetch` 和 `FKCheckTime` 分开累计，快照与 allocator 统计由后端负责深拷贝/合并，基本统计仅在目标为空时从另一份补入。

全局 `CreateSession` 和 `CloseSession` 的值由 `OnceLock` 只初始化锁容器一次，内部 `RwLock<Option<Arc<...>>>` 允许随后替换回调；本文件没有管理注册或调用生命周期。embedding 结果数组、首次取消原因都由局部 Mutex 所有，scoped 线程退出后才被取出，不产生脱离调用栈的后台任务。

## 依赖与调用关系

RustCodeGraph 对该文件报告 8 个文件级引用，但精确 `callers` 查询没有返回这些泛型入口的调用边；源码引用搜索给出了更窄且可验证的实际关系：

- [`pkg/executor/lib.rs`](lib.rs) 公开模块，并仅在测试配置下装入 `insert_common_test`。
- [`pkg/executor/insert.rs`](insert.rs) 的 `InsertExecutor::Next` 调用 `is_terminal_auto_id_error`，防止 RPC 重试耗尽类分配错误被普通 INSERT IGNORE/错误转换吞掉。
- [`pkg/session/runtime/dml.rs`](../session/runtime/dml.rs) 调用 `evaluate_embedding_inputs`，然后把有序结果写回对应行和生成列。
- [`pkg/executor/benchmark_test.rs`](benchmark_test.rs) 使用 `CompleteInsertErrorForColumn`、`CompleteLoadErrorForColumn`、`DmlErrorCause` 和 `InsertErrorKind` 验证窄错误格式化边界。
- [`pkg/executor/insert_common_test.rs`](insert_common_test.rs) 直接覆盖 cast 错误选择、终止性 auto-id cause 链和 embedding 并发行为。

算法内部的主要下游边为：`insertRows -> evalRow|fastEvalRow -> fillRow -> fillColValue`，随后 `lazyAdjustAutoIncrementDatum -> setDatumAutoIDAndCast`，最后 `insertCommon::exec`；SELECT 路径为 `insertRowsFromSelect -> executor_next_rows -> getRow -> fillRow -> exec`；冲突路径为 `batchCheckAndInsert -> foreign_key_check_rows/prefetch_unique_indices/transaction_get -> handleDuplicateKey -> removeRow` 或传入的 `add_record` 回调。

`Cargo.toml` 的直接相关依赖是 `astersql-meta-autoid`（`is_terminal_auto_id_error` 识别 `AutoIdError::RpcRetryLimit`）和 `astersql-expression`（`EmbedTextArgs`）。表、事务、Datum、执行器 chunk 等没有固定为具体 crate 类型，而由 `InsertBackend` 关联类型隔离。

## 错误处理与边界

`handleErr` 首先沿 error source 链识别终止性的 auto-id RPC 重试耗尽；该错误无论 SQL mode 或 IGNORE 都直接返回。其他错误按 INSERT 或 LOAD 路径补齐，再交给后端语句错误上下文决定报错还是降级为 warning。Timestamp DST transition 有专门分支：转换为 wrong-insert-value；严格模式且非 ignore 时返回，否则追加 warning。

`getRow` 保留 Go 的一个细节：cast 错误若被语句上下文接受则继续；若拒绝，LOAD 返回补全后的错误，非 LOAD 返回原始 cast 错误。这由 `resolve_get_row_cast_error` 明确编码。`rewrite_warnings` 只截取本列求值之后新增的 warnings，逐项补齐列/行上下文后再放回，避免改写之前的警告。

自增/自随机边界包括：显式非零 ID 必须 rebase；禁止显式 auto-random 时立即失败；auto-random incremental 超出 mask 返回读取失败；负的显式 auto-random/row-id 不 rebase；cast 后读回值小于原 ID 时通常报 `auto_increment_read_failed`，只有 on-duplicate 截断可作 warning 的兼容模式允许继续。`getAutoRecordID` 只接受 float/double/integer 类别，插入时浮点取 round，非插入取截断转换。

重复键预检只把 not-found 当作无冲突，其他事务读错误原样传播。旧行取不到会记录失败并转换为更明确的 old-row-not-found；二进制 Datum 比较错误加 trace。写入时 check constraint violation 在非 LOAD 场景追加 warning 并跳过该行，LOAD 不重复追加；其他写入错误终止批次。

`insertCommon for InsertValues` 的默认 `exec` 会 panic，因此不得直接把裸 `InsertValues` 传入主路径。若 `evalBuffer` 未初始化，`evalRow` 也会 panic；调用者必须先执行初始化。`evaluate_embedding_inputs` 中 worker panic 或 Mutex poisoning 也会 panic，目前没有把 panic 转成业务错误。

## 并发与资源生命周期

常规 INSERT 泛型算法本身是同步的，但后端要求 `Send + Sync + 'static`，并通过 `Arc<B>` 共享。批量路径的事务生命周期是“累积一批 -> `exec` -> statement commit -> statement 内新事务”；显式事务时禁用该跨事务批处理。内存跟踪以正负 delta 成对更新，VALUES 路径跟踪待写行，SELECT 路径同时跟踪 chunk、目标行和额外列；任何中途 `?` 返回前并非所有局部正 delta 都显式回收，实际安全性依赖执行器关闭/后端 tracker 的上层清理语义，当前文件本身不提供 RAII guard。

重复键路径复用同一事务，先预取唯一索引再逐键读取；临时表跳过预取。悲观事务且开启 `lock_unchanged_keys` 时，忽略的重复键或完全相同的 REPLACE 行会登记后续锁定键，防止“未修改”分支失去并发保护。外键检查发生在唯一键扫描前，真正插入后的单行外键登记只在没有跳过 duplicate check 时执行。

`evaluate_embedding_inputs` 每次调用创建 scoped threads，worker 数上限为 800 且不超过输入数。`AtomicUsize` 用 Relaxed 顺序只负责发号，结果可见性由 Mutex 和 scope join 保证。取消是协作式的：worker 只在领取任务后、调用 provider 前检查；已进入 provider 的任务不会被强制终止。多个 worker 同时看到取消时只保存首个取得锁并写入的错误文本；scope 结束后整批返回该错误。

`InsertRuntimeStat::Merge` 是普通可变方法，没有内部同步；调用方必须串行合并。全局会话钩子的读写同步由 `RwLock` 提供，但本文件未定义谁初始化、替换或消费它们。

## 与 Go 版本的对应关系

Rust 的主要名称和分支与 [`pkg/executor/insert_common.go`](insert_common.go) 基本一一对应：`InsertValues`、`insertCommon`、两条 `insertRows*` 主路径、行求值/默认值/生成列、自增与 auto-random、重复键批检、REPLACE 删除、写表和 `InsertRuntimeStat` 均保留了 Go 的控制流。`resolve_get_row_cast_error` 对齐 Go `getRow` 在 LOAD 与普通 INSERT 中选择补全错误或原始错误的差异；`is_terminal_auto_id_error` 对齐 Go `autoid.IsRPCRetryLimitError` 的不可忽略行为。

Rust 通过 `InsertBackend` 将 Go 中 `sessionctx.Context`、`table.Table`、`kv.Transaction`、`types.Datum`、chunk 和统计类型抽象为关联类型。这是移植结构差异，不应被理解为已有完整适配：当前没有发现 trait 实现，所以大部分算法还没有连接 Rust 生产执行器。Go 的同名代码仍有真实执行入口及具体依赖。

存在一项重要功能差异：Go 的 `insertRows` 和 `insertRowsFromSelect` 在每批 `exec` 前调用 `fillEmbedTextValues`，并由同文件完成生成列发现、参数求值、部署模式校验、provider 调用和结果写回；Rust 的两条泛型主路径没有调用 `evaluate_embedding_inputs`。Rust 只把 provider 并发核心留在本文件，参数准备与结果写回已迁到 `pkg/session/runtime/dml.rs`，由会话运行时直接调用。现有 Go 测试 [`pkg/executor/executor_pkg_test.go`](executor_pkg_test.go) 覆盖完整 fill 流程；Rust 独立测试只覆盖并发核心，所以不能宣称两边接线位置完全等价。

另有表现层差异：Rust 额外提供 `DmlErrorCause`/`CompletedDmlError` 作为窄测试/调用边界；泛型 `completeInsertErr` 才更接近 Go 对具体错误类型的改写。Rust `CreateSession`/`CloseSession` 使用可选、加锁、引用计数回调，而 Go 是由 session 包直接赋值的函数变量。Go 文件末尾还有 `recordWriteCPUWork`，Rust 本文件没有对应符号；这属于当前文件迁移差异，是否在其他 Rust 模块实现未在本任务中扩展调查。

## 扩展指南

若扩展行转换或默认值行为，应修改 `fillColValue`/`fillRow`/`getColDefaultValue`，保持 `has_value` 与 Datum 状态分离，并把新错误同时接入 `handleErr` 和 `rewrite_warnings`。新增生成列类型时要检查普通生成列表达式分支、数组/函数索引错误包装，以及 Go `fillRow` 对应逻辑；测试应放在独立的 [`pkg/executor/insert_common_test.rs`](insert_common_test.rs)，不要内嵌到生产文件。

若扩展 ID 语义，应分别审查即时与惰性自增路径、重试 ID 队列、rebase、mask、显式零和 `lastInsertID`，不能只改 `adjustAutoIncrementDatum`。终止性 allocator 错误必须继续穿透 IGNORE；对应 Rust 回归测试应覆盖 cause 链，Go 语义则核对 `insert_common.go` 和相关 executor 测试。

若要真正启用泛型 INSERT 主路径，最小必要工作不是在本文件继续堆算法，而是提供并验证具体 `InsertBackend` 实现及 `insertCommon::exec` 派生类型，再从 Rust INSERT 执行器接入。接线时应特别验证批事务、内存 tracker 的错误退出回收、SELECT extra columns、外键、临时索引以及 runtime stats；在接线完成前不得把这些方法当作生产覆盖证据。

扩展 EMBED_TEXT 时，应保持“会话层准备参数/写回，`evaluate_embedding_inputs` 只做 provider 调度”的现有边界。若改变并发上限、取消优先级或 fail-fast 策略，应同步修改 `insert_common_test.rs` 的顺序、NULL、参数错误、provider 错误、取消与 800 上限用例，并对照 Go [`pkg/executor/executor_pkg_test.go`](executor_pkg_test.go) 的完整行级行为。

新增或修改公开错误格式时，同时检查 [`pkg/executor/benchmark_test.rs`](benchmark_test.rs) 中 `Complete*ErrorForColumn` 用例。修改统计格式需同步 Go [`pkg/executor/insert_test.go`](insert_test.go) 的 `TestInsertRuntimeStat` 意图，并为 Rust 增加独立测试；当前 Rust 独立测试尚未直接覆盖 `InsertRuntimeStat`。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；目标文件可由 `node --file pkg/executor/insert_common.rs` 完整读取，报告 8 个文件级使用者。
- RustCodeGraph `query`：确认 Rust/Go 的 `insertRows`、`insertRowsFromSelect`、`batchCheckAndInsert`、`fillRow`、`lazyAdjustAutoIncrementDatum`，以及 Rust `evaluate_embedding_inputs`、`InsertValues`、`InsertBackend` 的定义位置。`callees insertRows` 和 `callees batchCheckAndInsert` 核对了行求值、批事务、内存、事务读、外键、预取和冲突处理下游边；精确 qualified-name `callers` 没有返回结果，因此实际上游又用源码引用搜索核验。
- 已读源与装配文件：[`pkg/executor/insert_common.rs`](insert_common.rs)、[`pkg/executor/lib.rs`](lib.rs)、[`pkg/executor/Cargo.toml`](Cargo.toml)。目标包不存在 `pkg/executor/doc.go`。
- 已读直接生产调用：[`pkg/executor/insert.rs`](insert.rs) 的终止性 auto-id 错误分支，以及 [`pkg/session/runtime/dml.rs`](../session/runtime/dml.rs) 的 embedding 参数准备、provider 调用和结果写回。
- 已读 Go 对照：[`pkg/executor/insert_common.go`](insert_common.go)，重点核对两条插入主路径、错误处理、`getRow`、embedding fill、自增/auto-random、重复键/REPLACE、写表和运行时统计。
- 已读测试：[`pkg/executor/insert_common_test.rs`](insert_common_test.rs)；[`pkg/executor/benchmark_test.rs`](benchmark_test.rs) 中错误补全用例；[`pkg/executor/executor_pkg_test.go`](executor_pkg_test.go) 中 EMBED_TEXT 完整流程；[`pkg/executor/insert_test.go`](insert_test.go) 中运行时统计测试位置。
- 仓库引用搜索没有发现 `impl InsertBackend`，也没有发现 Rust 泛型 `insertRows`/`insertRowsFromSelect`/`batchCheckAndInsert` 的生产调用；因此文档将其标为未接线移植框架，而没有用 Go 的现行行为代替 Rust 事实。
- 本任务是纯文档分析，按计划不运行 Cargo。结构验收命令与 Ready 文档检查结果在任务完成时记录于最终交付。
