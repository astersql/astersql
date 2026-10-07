# `pkg/bindinfo/utils.rs`

源码：[utils.rs](utils.rs)。本说明只描述当前 Rust 实现；Go 对照用于解释移植来源，不代表 Rust 已具备 Go 侧全部接线。

## 文件定位

`utils.rs` 是 `astersql-bindinfo` crate 的共享工具与适配边界。模块由 [lib.rs](lib.rs) 以私有 `mod utils` 装配后通过 `pub use utils::*` 重新导出，因此本文件中的 `pub` 项是 crate 对外 API。它不负责绑定匹配本身，而是在绑定缓存、持久化存储、SQL 执行上下文和 Hint 解析之间提供公共数据类型与薄封装。

该 crate 的边界由 [Cargo.toml](Cargo.toml) 定义：库入口是 `lib.rs`，直接依赖 `astersql-parser`、`astersql-util-hint`、`astersql-util-parser`、`serde` 和 `serde_json`，并以 `package.metadata.porting.go-package = "pkg/bindinfo"` 标明 Go 对照包。本文件自身直接使用 crate 内的 `Binding`、`BindingTime`、`Statement`、`prepareHints` 等符号，以及标准库的 `Arc`、`Duration` 和 serde 派生。

## 核心职责

1. 以 `BindingStore` 隔离绑定持久化，以 `BindingSqlContext` 隔离内部 SQL 执行、计划摘要计算和绑定校验，以 `DestroyableSessionPool` 规定会话获取、归还和销毁语义。
2. 通过 `callWithSCtx` 管理可选悲观事务及会话生命周期；通过 `exec`/`execRows` 保留与 Go 工具层相近的调用形状。
3. 通过 `GenerateBindingSQL` 恢复默认库名并把计划 Hint 注入 DELETE、UPDATE、SELECT、CTE 或 INSERT/REPLACE 的查询部分。
4. 通过 `readBindingsFromStorage`、`newBindingFromStorage`、`validateAndPrepareBinding` 和 `getBindingPlanDigest` 连接存储行、内存绑定、Hint 准备与计划摘要适配器。
5. 通过 `updateBindingUsageInfoToStorage` 及其内部函数节流、分批持久化最近使用时间；`addLockForBinds` 和 `saveBindingUsage` 提供写入所需的锁定与存储调用。

## 主要符号

- `UpdateBindingUsageInfoBatchSize: usize = 100`：每个使用信息批次的最大绑定数；Rust 中是不可变常量。
- `MaxWriteInterval: Duration = 6h`：同一绑定两次写回之间的最小节流间隔；Rust 中也是不可变常量。
- `BindingStore: Send + Sync`：持久化端口，覆盖增量读取、替换、逻辑删除、状态修改、删除记录 GC 和使用时间保存。批量方法的原子性由实现方保证；trait 本身不创建事务。
- `BindingSqlContext: BindingValidator`：SQL/优化器端口。`execute` 返回 `Vec<BindingRow>`，`plan_digest` 计算指定 schema 下的计划摘要；继承 `BindingValidator` 使同一上下文可交给 `prepareHints`。
- `DestroyableSessionPool: Send + Sync`：会话池端口。`release` 只用于成功路径，`destroy` 用于任何失败路径。
- `SqlValue`：参数化 SQL 的值类型，支持 `Null`、字符串、两种整数和 `BindingTime`，并可序列化。
- `BindingRow`：`mysql.bind_info` 行的 Rust 传输结构，包含原始/绑定 SQL、默认库、状态、时间、字符集/排序规则、来源及 SQL/计划摘要。
- `callWithSCtx<T, F>`：获取一次会话并执行 `FnOnce`；可用 `BEGIN PESSIMISTIC`、`COMMIT`、`ROLLBACK` 包裹闭包。
- `GenerateBindingSQL` 与私有 `keyword_position`：前者生成绑定 SQL；后者按词边界及括号深度定位应注入 Hint 的关键字。
- `updateBindingUsageInfoToStorage`、`shouldUpdateBinding`、`updateBindingUsageInfoToStorageInternal`：分别负责分批、节流判断和单批写回。
- `addLockForBinds`、`saveBindingUsage`：分别逐 SQL digest 执行 `SELECT ... FOR UPDATE`，以及委托存储保存使用时间。
- `newBindingFromStorage`：把 `BindingRow` 转为共享的 `Arc<Binding>`。
- `getBindingPlanDigest`：以 best-effort 语义调用上下文；失败返回空串。
- `validateAndPrepareBinding`：把校验和 Hint 解析委托给 `prepareHints`。

## 执行流程

事务会话流程如下：`callWithSCtx` 先 `acquire`；若要求事务则执行 `BEGIN PESSIMISTIC`。开始失败会立即 `destroy`。随后执行闭包：闭包成功时尝试 `COMMIT`，闭包失败时尽力 `ROLLBACK`，且回滚错误不会覆盖原错误。最终结果成功才 `release`，闭包错误或提交错误都 `destroy`。不包事务时，闭包结果直接决定归还或销毁。

绑定 SQL 生成流程如下：`GenerateBindingSQL` 克隆 `Statement`，先剥离可能的 `EXPLAIN ` 前缀，再调用 `RestoreDBForBinding` 恢复带默认库名的规范 SQL。它依据原 SQL 开头区分 DELETE、UPDATE、SELECT、WITH、INSERT 和 REPLACE；普通 DML 在首个目标关键字后注入 `/*+ ... */`，CTE 与 INSERT/REPLACE 只在括号深度为零的 SELECT 后注入。`INSERT ... VALUES` 没有 SELECT 时返回恢复后的 SQL；无法恢复或不支持的语句返回空串。

使用信息流程如下：`bindingCacheUpdater::UpdateBindingUsageInfoToStorage`（[binding_cache.rs](binding_cache.rs)）取得缓存内全部绑定并调用本文件的批量函数。外层按 100 条切片；内层读取每条绑定的 `LastUsedAt`/`LastSavedAt`，只有从未保存，或最近使用晚于最近保存且从最近保存到当前已满六小时的绑定才进入 `to_write`。它逐条调用 `saveBindingUsage`；整批全部成功后，才用同一个当前时间更新这些绑定的 `LastSavedAt`。任一保存失败会立即返回，且不会推进该批任何内存保存时间。

存储行转换时，`newBindingFromStorage` 小写化默认数据库名，把历史状态 `using` 兼容转换为 `enabled`，复制其余持久化字段，并用 `Binding::default()` 初始化 Hint、表名和使用统计等非行字段。

## 数据与状态

`BindingRow` 是无内部同步的拥有型行快照；`newBindingFromStorage` 把它消费并构造 `Arc<Binding>`。真正的使用统计位于 [binding.rs](binding.rs) 的 `bindingInfoUsageInfo`：`LastUsedAt` 与 `LastSavedAt` 都是 `Arc<Mutex<Option<BindingTime>>>`，因此共享绑定可在不取得 `&mut Binding` 的情况下更新两个时间戳。两个锁彼此独立，读取的时间对不是事务快照；本文件仅做节流选择，最终一致性取决于后续再次回写。

`BindingTime` 是 Unix epoch 微秒的 `i64` 包装。`shouldUpdateBinding` 用饱和减法规避时钟差值下溢，用 `i64::try_from(Duration::as_micros())` 处理转换溢出。`UpdateBindingUsageInfoBatchSize` 与 `MaxWriteInterval` 是编译期常量，不像 Go 对照中的包级变量那样能被测试或运行时修改。

`callWithSCtx` 独占从池中取得的 `Box<dyn BindingSqlContext>`，直至显式归还或销毁。`SqlValue` 与 `BindingRow` 的 serde 派生允许适配器跨序列化边界传递，但本文件没有执行序列化。

## 依赖与调用关系

已验证的 Rust 主链是：`bindingCacheUpdater::UpdateBindingUsageInfoToStorage` → `updateBindingUsageInfoToStorage` → `updateBindingUsageInfoToStorageInternal` → `shouldUpdateBinding` / `saveBindingUsage` → `BindingStore::save_usage`。`BindingStore` 还被 [binding_cache.rs](binding_cache.rs)、[binding_handle.rs](binding_handle.rs) 和 [binding_operator.rs](binding_operator.rs) 持有或使用，是当前绑定缓存与操作器共享的持久化边界。

`GenerateBindingSQL` → `RestoreDBForBinding`（[binding.rs](binding.rs)）→ parser/util-parser 的恢复逻辑，并在本文件内调用 `keyword_position`。当前 RustCodeGraph 未给出它的生产调用者；可见直接调用来自 [main_test.rs](main_test.rs) 和 [utils_test.rs](utils_test.rs)。因此它是已导出的可用能力，但不能据此声称已接入 Rust 规划主链。

`validateAndPrepareBinding` → `prepareHints`，后者捕获 panic、解析 Hint、收集表名并执行绑定校验。`getBindingPlanDigest` → `BindingSqlContext::plan_digest`。`readBindingsFromStorage` 仅委托 `BindingStore::read_bindings_since`。当前仓库搜索未发现这三个包装函数的 Rust 生产调用者；相应底层能力可能由其他模块直接使用。

`callWithSCtx`、`exec`/`execRows` 和 `addLockForBinds` 在本文件内部形成通用会话/SQL 工具，但当前 Rust 使用信息写回采用 `BindingStore`，不会经过 `callWithSCtx` 或 `addLockForBinds`。这与 Go 的事务型写回路径不同，扩展时不能假设二者已经串联。

## 错误处理与边界

- `callWithSCtx` 传播获取、BEGIN、闭包和 COMMIT 错误；ROLLBACK 是 best-effort，失败会被丢弃以保留原始闭包错误。COMMIT 失败会销毁会话。
- `GenerateBindingSQL` 以空串表示恢复失败或语句类型不支持。关键字搜索区分字母数字词边界但不理解 SQL 引号、注释或转义；正确性依赖 `RestoreDBForBinding` 产生的规范 SQL。匹配本身区分大小写，而调用处传入大写关键字。
- `keyword_position` 用 `saturating_sub` 容忍不平衡右括号，但它只是轻量扫描器，不是 parser。对复杂 SQL 的新支持应先补独立测试，不能只扩充字符串分支。
- `shouldUpdateBinding(None, None)` 返回 false；有最近使用但无最近保存返回 true；已有保存时间时还需同时满足“确有更新”和“节流期已过”。系统时钟回拨会使节流条件暂时不成立。
- `updateBindingUsageInfoToStorageInternal` 对空批次安全；`last_used.expect` 由同一闭包内的 `Option` 检查保证。它逐条调用存储，trait 注释要求实现方保障批量原子性，但函数本身无法回滚已成功的前序 `save_usage`。
- `saveBindingUsage` 显式拒绝空 SQL digest；计划摘要可以为空，具体 NULL/空串语义由 `BindingStore` 实现决定。
- `getBindingPlanDigest` 将任何错误降级为空字符串，调用方不能区分“合法空摘要”与“计算失败”。

## 并发与资源生命周期

三个端口 trait 的线程约束不同：`BindingStore` 与 `DestroyableSessionPool` 明确要求 `Send + Sync`；`BindingSqlContext` 本身没有这两个上界，且在一次 `callWithSCtx` 调用中只以共享引用使用。会话池必须保证 `acquire` 得到的资源最终恰好走 `release` 或 `destroy` 之一。

`Arc<Binding>` 允许缓存、写回器与匹配路径共享对象；使用时间通过两个 Mutex 更新。批量写回先在持锁很短的 getter 中复制时间值，随后无锁调用存储，避免跨 I/O 持有 Mutex。保存成功后再分别取得锁推进 `LastSavedAt`。并发命中若发生在筛选或写入期间，新的 `LastUsedAt` 可能晚于本次实际写入的时间；因为 `LastSavedAt` 会被设为批次完成时刻，这种交错在本轮可能被合并，下一次是否写回仍由时间比较和六小时节流决定。

`callWithSCtx` 没有异步任务、通道或后台线程；事务和会话生命周期都局限在同步函数调用内。`GenerateBindingSQL` 和行转换只操作局部拥有值，没有共享可变状态。

## 与 Go 版本的对应关系

直接对照文件是 [utils.go](utils.go)，相关端到端 Go 测试是 [tests/bind_usage_info_test.go](tests/bind_usage_info_test.go)。主要保持项如下：会话成功归还/失败销毁；可选悲观事务；按语句类型注入 Hint；使用时间写回节流；`using` 到 `enabled` 的兼容转换；计划摘要失败时采用 best-effort 空结果。

主要差异如下：

- Go 直接依赖 `sessionctx.Context`、内部 SQL executor 和 `mysql.bind_info`；Rust 把持久化及 SQL/计划能力抽象成 trait，以便在 SQL/session 栈未完全移植时使用适配器。
- Go 的 `readBindingsFromStorage` 组装 SELECT、跳过内建锁记录并逐条 `prepareHints`；Rust 同名函数只调用 `BindingStore::read_bindings_since`，过滤和准备责任由存储实现或上层承担。
- Go 使用信息单批路径通过 `callWithSCtx(true)`，先锁绑定信息表，再批量行锁并在事务中 UPDATE；Rust 当前写回直接逐条调用 `BindingStore::save_usage`，而 `addLockForBinds` 未接入该路径。原子性是 `BindingStore` 的契约，不由工具函数强制实现。
- Go 的行锁条件同时使用 plan digest 和 SQL digest，并特殊处理 NULL plan digest；Rust `addLockForBinds` 仅按 SQL digest 逐行执行参数化查询。
- Go 的 `getBindingPlanDigest` 临时修改会话变量、解析 SQL、拒绝含参数语句、计算摘要并恢复状态；Rust 只委托 `BindingSqlContext::plan_digest` 并吞掉错误，完整语义由适配器承担。
- Go 的批次大小和间隔是可变包变量，测试会缩小它们；Rust 使用常量。Go 外层先过滤后凑批，Rust先按输入固定切片、再在每片内过滤，因此每个存储调用最多 100 条，但可能包含更少待写项。
- Go `GenerateBindingSQL` 操作 AST 并先清除旧 Hint；Rust从 `Statement.SQL` 重新解析/恢复，再用关键字扫描注入。Rust 测试覆盖 EXPLAIN UPDATE、INSERT SELECT、CTE 和默认库恢复，但不等于覆盖 Go AST 的全部语句形态。

## 扩展指南

新增持久化后端时，实现 `BindingStore` 的全部方法，并重点保证 trait 声明的批量原子性、空计划摘要语义和错误传播；同步扩展 [binding_operator_test.rs](binding_operator_test.rs) 或 [tests/bind_usage_info_test.rs](tests/bind_usage_info_test.rs) 中的独立测试桩。不要把测试写回 `utils.rs`。

修改使用信息策略时，应同时审查 `shouldUpdateBinding`、外层切片方式、失败后时间戳不推进的不变量，以及 [binding_cache.rs](binding_cache.rs) 的入口。性能风险主要是逐条 `save_usage` 与逐条 `SELECT ... FOR UPDATE`；若改为真正批量操作，需保留失败原子性并核对 Go 多节点锁语义。

支持新的 SQL 形态或改变 Hint 注入位置时，优先修改 `GenerateBindingSQL`/`keyword_position`，并在独立的 [utils_test.rs](utils_test.rs) 增加嵌套查询、注释/字符串、无 SELECT INSERT、EXPLAIN 和不支持语句回归；同时核对 `RestoreDBForBinding` 的规范输出。轻量关键字扫描器若无法可靠覆盖新语法，应接入 parser/AST，而不是继续叠加脆弱字符串替换。

增强绑定读取或校验时，要先决定责任属于 `BindingStore::read_bindings_since` 还是 `readBindingsFromStorage` 上层，避免重复执行 Go 的伪记录过滤或 Hint 准备。接入 `getBindingPlanDigest` 时，必须记录空串降级的可观测性；需要区分失败时应改成 `Result<String>`，并同步调用方与测试。

## 验证依据

- RustCodeGraph 索引状态：11467 个文件、307296 个节点、1848419 条边；`files --filter pkg/bindinfo` 确认目标、crate 入口和测试均已索引。
- RustCodeGraph 源码读取：`node --file pkg/bindinfo/utils.rs --offset 1 --limit 500` 覆盖目标文件全部 405 行；`node --file pkg/bindinfo/lib.rs` 核对模块装配与公开重导出。
- RustCodeGraph 符号/边：`node GenerateBindingSQL`、`node updateBindingUsageInfoToStorage`、`node shouldUpdateBinding`、`node newBindingFromStorage`、`node getBindingPlanDigest`、`node validateAndPrepareBinding`、`node addLockForBinds`、`node saveBindingUsage`；确认关键内部边包括 `GenerateBindingSQL → keyword_position`、`updateBindingUsageInfoToStorage → updateBindingUsageInfoToStorageInternal → shouldUpdateBinding/saveBindingUsage`、`saveBindingUsage → BindingStore::save_usage`、`getBindingPlanDigest → BindingSqlContext::plan_digest`。
- 直接读取：[Cargo.toml](Cargo.toml)、[lib.rs](lib.rs)、[binding.rs](binding.rs)、[binding_cache.rs](binding_cache.rs)、[binding_operator.rs](binding_operator.rs)、[utils.go](utils.go)、[utils_test.rs](utils_test.rs)、[main_test.rs](main_test.rs)、[tests/bind_usage_info_test.rs](tests/bind_usage_info_test.rs)、[tests/bind_usage_info_test.go](tests/bind_usage_info_test.go)。
- 测试证据：[utils_test.rs](utils_test.rs) 验证 BEGIN 失败销毁会话、按最后保存时间节流、存储字段兼容转换及多种语句的 Hint 注入；[tests/bind_usage_info_test.rs](tests/bind_usage_info_test.rs) 验证过期才写、无新使用不重复写以及单批失败不推进任何 `LastSavedAt`；[main_test.rs](main_test.rs) 验证默认库恢复与 Hint 注入。Go 测试提供真实系统表、批次和开关行为的对照，但不是 Rust 运行证据。
- 本任务按计划仅做静态事实与结构验证，未运行 Cargo、Rust 测试或 Go 测试。
