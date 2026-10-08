# `pkg/planner/core/point_get_plan.rs` 逻辑说明

## 文件定位

`point_get_plan.rs` 属于 `astersql-planner-core` crate；`pkg/planner/core/Cargo.toml` 以 `lib.rs` 为 crate 根，`pkg/planner/core/lib.rs` 通过 `pub mod point_get_plan` 将本模块公开。文件实现的是一套以 `FastQuery` 为输入的简化点查计划构造器：把已经规整为单表、投影字段、谓词和锁选项的数据转换为 `FastPlan::{Point,Batch,Update,Delete}`，而不是直接解析 SQL AST。

这条简化路径应与真实规划主链区分开。RustCodeGraph 对 `point_get_plan.rs::TryFastPlan` 的调用边显示，直接生产调用者只在本文件的 `tryUpdatePointPlan`、`tryDeletePointPlan`，另有 `pkg/planner/core/integration_test.rs` 和 `pkg/planner/core/point_get_plan_test.rs` 测试调用；`pkg/planner/optimize.rs` 的 `OptimizeRuntimeService::try_fast_plan` 是由运行时注入的抽象入口，并没有静态调用本文件的 `TryFastPlan`。此外，`pkg/planner/core/point_get_plan_runtime.rs` 提供另一条面向真实逻辑计划的 `TryFastIntegerPointGet` 路径，不能与本文件的简化模型混为一谈。

## 核心职责

- `TryFastPlan` 按“OR 批量点查 → IN 批量点查 → 单点点查”的优先级尝试构造只读快路径，并以 `None` 表示不适用、应由上层回退。
- `tryOr2BatchPointGet`、`tryWhereIn2BatchPointGet` 和 `tryPointGetPlan` 分别构造 OR 析取、IN 列表和等值谓词的点查访问路径。
- `choose_index`、`checkTblIndexForPointPlan`、`getNameValuePairs`、`getPointGetValue` 和 `getIndexValues` 负责唯一键完备性、索引提示、常量类型转换和键值顺序。
- `buildSchemaFromFields` 负责输出列与 `tidb_row_checksum` 伪列；`getLockWaitTime` 把简化锁信息折算为锁标志和毫秒等待值。
- `tryUpdatePointPlan`、`tryDeletePointPlan` 在只读点查之上包装简化的点更新/点删除计划；`pointPlanToNode` 把结果降格为通用 `PlanNode` 供计划树表示。
- `checkFastPlanPrivilege` 是独立的权限集合校验帮助函数；当前 `TryFastPlan` 只接收已经计算好的 `privilege_ok`，不会自行查询权限系统。

## 主要符号

数据模型如下。

- 常量 `GlobalWithoutColumnPos` 与 `PointPlanKey` 对齐 Go 同名概念；本文件内没有读取这两个常量，属于供 crate 外部或后续接线使用的公开接口。
- `FastQuery` 汇总表元数据 `TableInfo`、别名、`FastField` 投影、`Predicate` 谓词、`LockInfo`、顺序/Limit 和 USE/IGNORE INDEX 名称。它是本文件所有快路径判断的输入边界。
- `Predicate` 支持 `Eq`、`NullSafeEq`、`In`、`And`、`Or`、`Other` 和 `False`。不是所有变体都能进入快路径：例如 `NullSafeEq` 会被 `getNameValuePairs` 拒绝，`Other` 会阻止单点计划。
- `FastPlan` 是四种结果的和类型。`PointGetPlan` 保存 handle 或唯一索引键、访问条件、schema、锁和分区字段；`BatchPointGetPlan` 保存多组 handle/索引键以及顺序信息；`PointUpdatePlan`、`PointDeletePlan` 以 `Box<FastPlan>` 持有源点查计划。
- `nameValuePair` 保存等值谓词抽出的列、值、参数标记和字段类型。当前提取函数总把 `param_marker` 设为 `None`，说明参数标记字段尚未在该简化入口中接线。
- 公开入口包括 `TryFastPlan`、三种点查尝试函数、DML 包装函数、schema/键值/权限帮助函数及 `pointPlanToNode`。内部函数仅有 `predicateBranches`、`pairsForBranch`、`choose_index`、`predicate_expr` 及嵌套的 `expand`、`combine`、`containsOr`、`walk`。
- 文件没有 trait、类型别名、条件编译项或异步函数；唯一 `impl` 是 `subQueryChecker` 的 `Enter`/`Leave`。

## 执行流程

1. `TryFastPlan` 首先拒绝权限不足或空投影。随后先调用 `tryOr2BatchPointGet`，再用 `getNameValuePairs` 提取普通等值条件并调用 `tryWhereIn2BatchPointGet`，最后调用 `tryPointGetPlan`。任一步成功即返回，失败则自然回退为 `None`。
2. OR 路径由 `predicateBranches` 把嵌套 `And`/`Or` 展开为析取范式分支。每个分支必须仅由 `Eq` 和 `And` 构成；`False` 产生空分支集合，其它谓词使展开失败。主键 handle 表按每个分支抽取主键值；否则根据首分支选唯一索引，再逐分支验证完整索引键并按索引列顺序组装 `index_values`。
3. IN 路径寻找顶层第一个 `Predicate::In`。若目标是整型主键 handle，则把列表逐值转换到 `handles`；否则寻找一个由等值条件加 IN 列共同覆盖的唯一索引，为每个 IN 值构造一行复合索引键。
4. 单点路径拒绝任意顶层 `In`、`Other` 和 `LIMIT 0`。它优先通过 `findPKHandle` 使用主键 handle；否则调用 `choose_index` 选完整唯一索引。`check=true` 时再由 `checkTblIndexForPointPlan` 验证访问路径。
5. 三条读路径均调用 `buildSchemaFromFields` 和 `getLockWaitTime`，并把原始谓词经 `predicate_expr` 压缩为仅保留操作名称的 `Expression` 访问条件。批量路径在出现 Limit 时设置 `keep_order`，并传递 `order_desc`。
6. `tryUpdatePointPlan` 先以字符串约定排除赋值子查询，再调用 `TryFastPlan`，最后由 `buildOrderedList` 校验赋值列、按列 offset 记录值并包装 `PointUpdatePlan`。`tryDeletePointPlan` 同样复用 `TryFastPlan` 后包装 `PointDeletePlan`。二者当前固定使用 `50_000` 毫秒会话等待值。
7. `pointPlanToNode` 只映射计划种类和估算行数：批量计划取 `handles.len()` 与 `index_values.len()` 的较大值，其余固定为 `1.0`；它不会复制 schema、锁或访问条件。

## 数据与状态

所有构造函数都以借用输入、克隆元数据并返回拥有所有权的计划值为主，没有模块级可变状态。`newPointGetPlan` 和 `newBatchPointGetPlan` 会克隆 `TableInfo`/schema，并初始化空键集合和默认锁状态；后续尝试函数逐项填充。

点查定位存在两种互斥表示：`PointGetPlan.handle` 用于 `pk_is_handle` 表，`PointGetPlan.index` 加 `index_values` 用于唯一索引；批量版本对应 `handles` 与 `index_values`。`getIndexValues` 严格按 `IndexMeta.columns` 的 offset 顺序组键，一旦某列缺失或无法转换便停止，因此调用者还需用长度等于索引列数来判定完整性。

`buildOrderedList` 用局部 `HashSet<usize>` 防止同一列被重复赋值；结果 `(column.offset, Value)` 保持赋值列表遍历顺序，而不是另行按 offset 排序，名称“ordered”在当前实现中实际表示已解析为列位置。`subQueryChecker.found` 是实例字段，但 `isExprHasSubQuery` 并未使用该 visitor，而是直接检查 `Expression.name` 的 `subquery:` 前缀。

`partition_id`、`partition_ids`、`PointPlanVal.names`、`nameValuePair.param_marker` 等字段在本文件构造流程中没有被填充或消费，应视为预留/未接线状态，而不是已有完整分区或参数计划缓存能力。

## 依赖与调用关系

本文件直接依赖同 crate 的 `planbuilder` 数据类型与转换函数（`TableInfo`、`ColumnInfo`、`IndexMeta`、`Schema`、`Value`、`convertValue`、`BuilderError`），以及 `task` 的 `Expression`、`FieldType`、`PlanKind`、`PlanNode`、`TypeCode`；标准库只使用 `HashSet`。这些是 crate 内模块依赖，不对应 `Cargo.toml` 中新增的外部 crate 依赖。

RustCodeGraph 的 `node point_get_plan.rs::TryFastPlan` 给出的下游边为 `tryOr2BatchPointGet`、`getNameValuePairs`、`tryWhereIn2BatchPointGet`、`tryPointGetPlan` 和 `FastPlan::Batch`；上游边为本文件的 `tryUpdatePointPlan`、`tryDeletePointPlan`，以及直接测试。主要下游继续连接到 `buildSchemaFromFields`、`getLockWaitTime`、`choose_index`、`checkTblIndexForPointPlan`、`getPointGetValue` 和 `getIndexValues`。

`pkg/planner/core/lib.rs` 同时公开 `point_get_plan` 与 `point_get_plan_runtime`。后者以及 `pkg/planner/optimize.rs::OptimizeRuntimeService::try_fast_plan` 表明完整应用的快路径通过运行时服务和真实计划类型接线；当前静态图没有证明该服务最终调用本文件的 `FastQuery` 入口。因此安全结论是：本文件是公开的简化构造层并受到单元/集成测试覆盖，但其与完整 SQL 优化入口的生产桥接在所检查的直接证据中未验证。

## 错误处理与边界

快路径候选函数大量使用 `Option`，把不满足形状、类型、索引或权限条件统一表示为 `None`，允许调用方回退通用优化器。需要向调用者解释原因的帮助函数使用 `Result<_, BuilderError>`：`getNameValuePairs` 报未知列、重复约束和非普通等值；`buildSchemaFromFields` 报未知投影列；`checkTblIndexForPointPlan` 报不完整唯一索引或缺主键 handle；`checkFastPlanPrivilege` 报访问拒绝；`buildOrderedList` 报未知列、生成列赋值、重复赋值或非法赋值值。`TryFastPlan` 会以 `.ok()?` 抹去这些错误文本，转换为回退信号。

重要边界包括：空字段或 `privilege_ok=false` 拒绝；`LIMIT 0` 拒绝；NULL 不可转换；`NullSafeEq` 不进入普通 EQ 快路径；唯一索引必须非 invisible、非前缀索引且完整覆盖；USE/IGNORE INDEX 比较不区分大小写；重复列等值约束报错后回退；主键/索引值必须通过 `checkCanConvertInPointGet` 和 `planbuilder::convertValue`。当前类型白名单只覆盖 Int、UInt、String、Bytes 的部分组合，比 Go 的 Datum/FieldType 转换规则窄。

OR 展开可能产生各 AND 子项分支数的笛卡尔积，当前没有分支数量上限。`predicate_expr` 只保留谓词类别名称，不保留列和值；因此这些 `access_conditions` 不能被视为完整可执行表达式。`getHashOrKeyPartitionColumnName` 只返回首个主键列，且点查构造流程不填充分区 ID；分区路由能力不能从此文件推断为完整支持。

## 并发与资源生命周期

本文件没有线程、异步任务、通道、I/O、锁对象或事务句柄。所有集合与计划对象都在单次函数调用内创建，并通过所有权返回；共享输入只被不可变借用，所以文件自身没有数据竞争或清理协议。

`LockInfo` 和计划中的 `lock`/`lock_wait_time` 是描述执行期行锁意图的数据，不是本模块实际持有的互斥锁。`getLockWaitTime` 对 `NOWAIT` 返回 0，对显式秒数使用 `saturating_mul(1000)` 防止整数乘法溢出，否则采用调用者传入的会话默认毫秒值。它没有像 Go `getLockWaitTime` 那样自行判断事务是否满足悲观锁资格。

Go 实现的 `subQueryChecker` 使用 `sync.Pool` 复用 visitor 并在归还前重置状态；Rust 简化实现没有池化，也没有遍历真实 AST，因此不存在相应的跨调用复用生命周期。相关 SQL 集成测试 `pkg/planner/core/tests/pointget/point_get_plan_test.rs` 使用静态 `Mutex` 串行化会修改全局/会话可见配置的测试，但该 mutex 不属于生产文件。

## 与 Go 版本的对应关系

名称和高层意图来自 `pkg/planner/core/point_get_plan.go`：两边都有 `TryFastPlan`、锁等待计算、单点/批量点查、唯一索引键抽取、schema 构造、点更新/删除、子查询排除和辅助常量。`pkg/planner/core/Cargo.toml` 的 `[package.metadata.porting].go-package = "pkg/planner/core"` 也声明了该 crate 的 Go 对照包。

但 Rust 文件是显著简化的移植层，差异至少包括：

- Go `TryFastPlan` 接收 `PlanContext` 与解析/解析后 AST，负责 fix-control、稳定结果模式、PlanID 重置、`sql_select_limit`、MemDB、权限和语句类型分派；Rust 接收预构造 `FastQuery`、布尔权限结果和等待值。
- Go IN 路径支持 AST 行表达式及更完整的表/列状态、物化视图、分区和 hint 检查；Rust 只处理单个 `Predicate::In(String, Vec<Value>)`，并用精简 `IndexMeta` 过滤唯一、可见、无前缀索引。
- Go 单点路径检查 HAVING/ORDER/LIMIT offset、生成列和非 public 列、InfoSchema 最新索引状态、TableDual、`_tidb_rowid`、隔离级别及参数常量；Rust 数据模型没有这些状态，`False` 也不会生成专门 TableDual。
- Go 值转换会检查 collation、BIT、溢出、截断和转换前后相等；Rust 使用较窄的类型匹配后委托 `convertValue`。Rust 独立测试 `primary_key_in_uses_converted_handle_values_like_go` 验证了字符串主键值转整型，`null_safe_equality_does_not_enter_go_eq_only_fast_path` 验证 `<=>` 不误入 EQ 快路径。
- Go 更新/删除构造真实 physical plan、权限、表映射、分区集合、外键触发器、表达式重写和悲观锁；Rust 只包装值对象，赋值仅接受 `Value`，以 `subquery:` 字符串约定检测子查询。
- Go `PointPlanVal` 只保存 `base.Plan` 并用于多语句缓存；Rust 还保存 `names`，但当前文件未展示缓存读写。Go 的 `GlobalWithoutColumnPos` 用于全局索引分区列语义；Rust 常量当前未使用。

因此扩展时应以 Go 文件为语义基线逐项补齐，不能把当前 Rust API 的成功返回等同于 Go 生产路径已经覆盖全部安全检查。

## 扩展指南

新增点查形态时，优先在 `Predicate`、`getNameValuePairs` 和三个 `try*PointGet` 入口明确接受/拒绝规则，并同步 `predicate_expr`；新增索引能力时修改 `choose_index`、`checkTblIndexForPointPlan`、`getIndexValues`，特别关注复合键完整性、前缀/不可见/全局/多值索引和 hint 语义。扩展类型转换时同时审查 `checkCanConvertInPointGet`、`getPointGetValue` 与 `planbuilder::convertValue`，并对照 Go 的 collation、溢出、截断与转换等值约束。

若要把该简化层接入完整应用，应先明确它与 `point_get_plan_runtime.rs`、`OptimizeRuntimeService::try_fast_plan` 的职责边界，补齐 AST/会话到 `FastQuery` 的可信转换，并确保权限、InfoSchema 最新索引、分区、事务锁资格、计划 ID、fix-control、TableDual 和回退行为不会遗漏。不要仅增加静态调用就宣称完成 Go 等价移植。

测试应至少同步 `pkg/planner/core/point_get_plan_test.rs`（值转换和谓词拒绝的聚焦测试）与 `pkg/planner/core/integration_test.rs::test_point_get_with_select_lock`（OR/IN/复合唯一键、锁等待和访问条件）。若变化影响真实 SQL、计划缓存或执行器，再同步 `pkg/planner/core/tests/pointget/point_get_plan_test.rs` 及对应 Go 测试 `pkg/planner/core/tests/pointget/point_get_plan_test.go`；若改的是另一条真实逻辑计划入口，则还需检查 `point_get_plan_runtime_test.rs`。主要风险是错误接受非唯一/不完整键导致错误行定位、值转换差异导致结果不兼容、OR 展开导致规划时间/内存膨胀，以及锁/分区/计划缓存元数据遗漏。

## 验证依据

- 源码全量检查：`pkg/planner/core/point_get_plan.rs`；模块和 crate 边界：`pkg/planner/core/lib.rs`、`pkg/planner/core/Cargo.toml`。目标包根目录没有 `doc.go`，因此无同包 package contract 可读。
- RustCodeGraph 索引状态：11,467 个文件、307,296 个节点、1,848,419 条边。执行了 `query` 查找 Rust/Go 的 `TryFastPlan`、`tryOr2BatchPointGet`、`tryWhereIn2BatchPointGet`、`tryPointGetPlan`，并对 `point_get_plan.rs::TryFastPlan` 及三个关键分支执行 `node`/`callers`/`callees`。精确 `node` 的 Trail 证实入口的五条主要下游边以及更新、删除和测试调用者；独立 `callers` 子命令未输出调用者，因此以上游 Trail 与仓库引用搜索交叉核对。
- 直接测试证据：`pkg/planner/core/point_get_plan_test.rs`；锁、OR、IN、复合唯一索引证据：`pkg/planner/core/integration_test.rs::test_point_get_with_select_lock`；真实 SQL/计划缓存对照：`pkg/planner/core/tests/pointget/point_get_plan_test.rs`；相邻但不同入口的边界证据：`pkg/planner/core/point_get_plan_runtime_test.rs`。
- Go 语义基线：`pkg/planner/core/point_get_plan.go`；Go SQL 测试索引：`pkg/planner/core/tests/pointget/point_get_plan_test.go`。应用入口边界参考 `pkg/planner/optimize.rs::OptimizeRuntimeService::try_fast_plan` 与 `tryFastPlanIfTiKV`。
- 本任务是纯文档分析，按计划未运行 Cargo。人工复核确认本文区分当前接线与未验证能力，说明文件存在原因、主流程、失败回退、扩展位置及兼容/性能风险；固定章节结构另由任务指定命令验证。
