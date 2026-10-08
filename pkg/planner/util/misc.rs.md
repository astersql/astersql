# `pkg/planner/util/misc.rs`

## 文件定位

源文件为 [`misc.rs`](./misc.rs)。它属于 `astersql-planner-util` crate；crate 根 `pkg/planner/util/lib.rs` 以私有模块 `mod misc` 装入它，再通过 `pub use misc::*` 将其公开 API 统一再导出。`pkg/planner/util/Cargo.toml` 的 `[package.metadata.porting]` 指向 Go 包 `pkg/planner/util`，直接语义基线是同目录的 `misc.go`。

本文件不负责构建完整逻辑或物理计划，而是给规划阶段提供四类横切工具：递归容器遍历、计划字段克隆、小型编码/元数据辅助，以及与表达式下推和隔离读引擎选择相关的上下文与路径过滤。实际主链接线包括 `pkg/session/runtime/planning.rs` 对访问路径的隔离读过滤，以及多个物理算子对 TiFlash/MPP 下推条件的判断。

## 核心职责

1. `RecursiveSlice<E>`、`RecursiveFlattenIter<'a, E>` 与 `SliceRecursiveFlattenIter` 用类型安全的显式树替代 Go 版本基于反射的任意维切片遍历，按深度优先、从左到右的顺序产出全局连续下标与元素借用。
2. `CloneFieldNames`、`CloneExprs`、`CloneAssignments`、`CloneHandleCols`、`CloneCols`、`CloneDatums`、`CloneDatum2D`、`CloneHandles` 保留 `None`，并按各领域类型的 `Clone`/`CloneHandleCols`/`Copy` 契约复制计划字段，避免调用者自行重复容器复制逻辑。
3. `QueryTimeRange`、`EncodeIntAsUint32`、`GetMaxSortPrefix`、`ExtractTableAlias` 提供指标时间条件、稳定的四字节编码、可下推排序前缀和 hint 表别名提取等规划辅助。
4. `GetPushDownCtx` 与 `GetPushDownCtxFromBuildPBContext` 把计划构建上下文投影为表达式层所需的 `PushDownContext`。
5. `ShouldCheckTiFlashPushDown` 和 `FilterPathByIsolationRead` 将会话级隔离读引擎配置落实到 TiFlash/MPP 候选判断及 `AccessPath` 过滤，并产生与 Go 版本一致意图的错误和警告。

## 主要符号

- `RecursiveSlice<E>`：公开递归枚举；`Values(Vec<E>)` 是叶子，`Slices(Vec<RecursiveSlice<E>>)` 是中间节点。Rust 调用者必须显式构造这棵树，而不能直接传任意维原生切片。
- `RecursiveFlattenIter<'a, E>`：公开迭代器类型，但内部状态 `stack`、`values`、`index` 均私有；`Iterator::Item` 为 `(usize, &'a E)`，因此遍历不复制元素。
- `SliceRecursiveFlattenIter<E>(&[RecursiveSlice<E>])`：迭代器构造入口。名称沿用 Go 风格，crate 根通过 `#![allow(non_snake_case)]` 接受这种命名。
- 八个 `Clone*` 函数：均接收 `Option<&[...]>` 或等价借用并返回拥有所有权的 `Option<Vec<...>>`；其中 trait object 不能依靠普通 `Clone`，所以 `CloneHandleCols` 调用领域方法 `HandleCols::CloneHandleCols`，`CloneHandles` 调用 `kv::Handle::Copy`。
- `QueryTimeRange { From, To }`：两个公开的 `chrono::DateTime<FixedOffset>` 字段；`Condition` 生成含毫秒的闭区间 SQL 片段，`MemoryUsage` 返回结构体本体大小。
- `MetricTableTimeFormat`：保留 Go 时间布局字符串 `2006-01-02 15:04:05.999` 的公开常量；Rust 的 `Condition` 实际使用 chrono 格式串 `%Y-%m-%d %H:%M:%S%.3f`。
- `EncodeIntAsUint32(Vec<u8>, i32)`：将有符号值按 `as u32` 转换后，以大端四字节追加到原缓冲区。
- `GetMaxSortPrefix`：返回 `sort_columns` 在 `all_columns` schema 中从首项开始连续可定位的下标；遇到第一个缺失列立即停止。
- `ExtractTableAlias`：仅在计划输出名可归一为单一表时构造 `hint::HintedTable`；处理缺省数据库和派生表所属查询块偏移。
- `GetPushDownCtx` / `GetPushDownCtxFromBuildPBContext`：前者从 `PlanContext` 取 `BuildPBContext`，后者传递表达式上下文、客户端、 explain 标记、两类 warning handler 与 `GroupConcatMaxLen`。
- `ShouldCheckTiFlashPushDown`：要求计划确实含 TiFlash，且会话允许从 TiFlash 隔离读；两条件缺一即为 `false`。
- `FilterPathByIsolationRead`：本文件唯一返回 `Result` 的入口；系统库直接放行，普通库保留会话允许的引擎路径以及始终允许的 TiDB 路径。

## 执行流程

递归扁平化从仅包含顶层迭代器的栈开始。`next` 先尝试耗尽当前叶子 `values`；每产出一个元素，先保存当前 `index`，再递增计数。叶子耗尽后继续读取栈顶：遇到 `Values` 切换叶迭代器，遇到 `Slices` 压入新的子层迭代器，层级耗尽则弹栈；栈空即结束。空节点不占下标，消费者的 `continue` 不会阻止迭代器内部下标增长，消费者 `break` 则通过丢弃迭代器自然终止。

`ExtractTableAlias` 先从 `plan.output_names()` 找第一个非空表名，再扫描全部非空输出名。不同表名、明确且冲突的数据库名，或“数据库名非空但表名为空”的不完整元数据都会拒绝提取。随后读取计划的 query block offset；若当前块在 `PlannerSelectBlockAsName` 中登记了显式表名且与父块不同，则把别名归到 `parent_offset`。缺省数据库来自会话 `CurrentDB()`，最终生成 `HintedTable`。

`FilterPathByIsolationRead` 的普通库流程为：

1. 从会话取得允许读取的 `StoreType` 集合，并逆序扫描原路径，按首次出现记录实际可用引擎；逆序保证错误中的引擎次序与 Go 实现一致。
2. 原地保留配置允许的路径；`StoreType::TiDB` 无条件保留。
3. 读取系统变量 `tidb_isolation_read_engines` 的字符串值。若已无路径，生成列出表名、配置值和实际可用引擎的错误；配置包含 `tiflash` 时追加副本提示，严格 SQL 模式移除了 TiFlash 时再追加只读提示。
4. 不论是否已经形成“无路径”错误，只要允许集合不含 TiFlash，仍调用 `RaiseWarningWhenMPPEnforced`：严格模式分支说明非只读查询会阻塞 MPP，普通分支说明隔离读配置不匹配。
5. 最后返回过滤后的路径或预先构造的错误。这一顺序保证错误场景仍能留下 MPP warning，见 `misc_test.rs`。

## 数据与状态

文件本身没有全局可变状态。`RecursiveFlattenIter` 的可变状态完全由迭代器实例拥有：栈表示尚未遍历完的层级，`values` 表示当前叶子，`index` 是已经向消费者产出的元素数。其额外空间与嵌套深度成正比，而不是与元素总数成正比。

克隆函数只创建新的外层容器，并依赖元素类型的既有复制契约；是否存在更深层共享由相应类型的 `Clone`、`Assignment::Clone`、`HandleCols::CloneHandleCols` 或 `Handle::Copy` 决定，不能仅凭本文件推断为完全深拷贝。所有这些函数都区分 `None` 与 `Some(empty)`。

`FilterPathByIsolationRead` 消费并修改传入的 `Vec<AccessPath>`，不修改路径对象的字段；会话变量和语句上下文通过共享的 `PlanContext` 读取。唯一可见副作用是 `RaiseWarningWhenMPPEnforced` 可能向 statement context 追加 warning。`QueryTimeRange` 的起止点都带固定时区偏移，条件格式化保留到毫秒。

## 依赖与调用关系

crate 依赖由 `pkg/planner/util/Cargo.toml` 声明。本文件直接依赖 `expression`、`types`、`kv`、`chrono`、`plan-base`、`hint`、`parser-ast`、`metadef` 和 `vardef`，并依赖本 crate 再导出的 `AccessPath` 与 `HandleCols`。

已核实的上游关系包括：

- `pkg/session/runtime/planning.rs::populate_session_data_source` 在构造普通表/TiFlash 候选路径并保存 `AllPossibleAccessPaths` 后，调用 `FilterPathByIsolationRead` 写回真正可参与规划的 `PossibleAccessPaths`；错误通过 `?` 中止数据源填充。
- `ShouldCheckTiFlashPushDown` 被 `physical_projection.rs`、`base_physical_agg.rs`、`physical_selection.rs`、`physical_limit.rs`、`physical_window.rs` 和 `base_physical_plan.rs` 使用，决定是否枚举或接受 MPP/TiFlash 物理候选。
- `pkg/planner/cascades/old/transformation_rules.rs::PushSelDownTiKVSingleGather::on_transform` 调用 `GetPushDownCtx`，再把所得上下文交给 `expression::PushDownExprs`。
- `pkg/planner/core/generator/plan_cache/plan_clone_generator.rs` 会生成对 `CloneAssignments`、`CloneDatums`、`CloneHandles`、`CloneDatum2D`、`CloneFieldNames` 和 `CloneHandleCols` 的 Go 风格克隆代码；当前 Rust 生成目标中的若干调用仍以注释形式保留，因此不能把每个 `Clone*` 都表述为已有运行时调用。
- RustCodeGraph 将 `misc.rs` 标为被 17 个文件使用，但对若干精确 Rust 函数查询未建立静态 caller/callee 边；上述精确生产调用因此以源码搜索和调用点读取补齐，而不是从缺失图边推断“无调用者”。

下游方面，`GetPushDownCtxFromBuildPBContext` 调用 `expression::NewPushDownContext`；`GetMaxSortPrefix` 依赖 `expression::NewSchema` 与 `Schema::ColumnIndex`；`ExtractTableAlias` 依赖 `Plan`/`PlanContext`/会话变量；隔离读过滤依赖 `metadef::IsSystemRelatedDB`、`SessionVars` 和 `AccessPath::StoreType`。

## 错误处理与边界

- 扁平化迭代器接受空顶层、空叶子和空中间节点；它没有递归函数调用栈，但极深输入会线性增长显式 `Vec` 栈。枚举类型排除了 Go 反射版运行时收到非切片层级的可能性。
- `Clone*` 对 `None` 返回 `None`，不将其折叠为空向量。调用方若依赖“未设置”和“已设置为空”的区别，应保留这一约束。
- `QueryTimeRange::Condition` 不验证 `From <= To`，也不对 SQL 片段进行参数绑定；输入是强类型时间而非任意字符串，因此这里只承担格式化。`MemoryUsage` 只能在有效引用上调用，不存在 Go nil receiver 返回零的分支。
- `EncodeIntAsUint32` 对负数采用 Rust 的截断转换语义，所得位模式与把 Go `int` 转为 `uint32` 后大端编码一致；结果总是追加四字节。
- `GetMaxSortPrefix` 只接受连续前缀：后续列即便存在，只要前一排序列缺失也不会继续收集。
- `ExtractTableAlias` 对无非空表名、多表、明确数据库冲突和不完整 DB/Tbl 元数据返回 `None`。双方数据库名有一方为空时不判冲突，以保留 Go issue #46160 所体现的兼容行为。
- `FilterPathByIsolationRead` 对系统相关数据库完全绕过过滤，也不生成 MPP warning。普通库即使配置未列 TiDB 仍保留 TiDB 路径。读取系统变量失败被 `unwrap_or_default` 折叠为空字符串；这是当前 Rust 行为。无路径错误使用 `expression::errors::New`，错误类别是否与 Go 的 `plannererrors.ErrInternal` 完全等价未由本文件证明。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道、事务、文件句柄或网络资源。所有临时向量、字符串、schema 和迭代器状态都遵循普通 Rust 所有权，在函数返回或迭代器被丢弃时释放。

`RecursiveFlattenIter` 只持有输入树的共享借用，生命周期 `'a` 保证迭代期间树仍有效；它没有声明 `Send`/`Sync` 的额外契约，是否可跨线程取决于泛型元素与标准迭代器的自动 trait。`PlanContext` 和会话内部可能使用原子/共享状态，但本文件只通过接口同步读取；warning 的写入由 `SessionVars::RaiseWarningWhenMPPEnforced` 自身负责。调用者不应在并发执行期间绕过会话上下文的既有同步契约去修改相关变量。

## 与 Go 版本的对应关系

Rust 基本逐项对应 `pkg/planner/util/misc.go` 的公开工具，但存在有意的类型适配和少量可观察差异：

- Go `SliceRecursiveFlattenIter` 接受任意维切片，以反射递归并通过 `iter.Seq2` 的回调布尔值提前停止；Rust 用 `RecursiveSlice<E>` 消除反射和动态类型断言，迭代顺序与全局下标语义由 `slice_recursive_flatten_iter_test.rs` 对照 Go 测试覆盖。
- Go 克隆函数接受 nil slice，Rust 用 `Option` 表达 nil。Go 逐元素调用各类型的 Clone/Copy；Rust 对普通可克隆值使用 `to_vec`，对领域 trait object 保留专用克隆方法。两者都保留 nil/None 与空容器的区别。
- Go `QueryTimeRange::Condition` 使用 `MetricTableTimeFormat`；Rust 用等价的 chrono 格式直接格式化，公开常量只是保留 Go 布局。Go `MemoryUsage` 支持 nil receiver 并返回零，Rust 引用天然非空，仅返回 `size_of::<QueryTimeRange>()`。
- `EncodeIntAsUint32`、`GetMaxSortPrefix`、别名选择、下推上下文字段、TiFlash 双条件判断和隔离读过滤的主分支与 Go 文件对齐。Rust 使用拥有所有权的 `Vec<AccessPath>` 与 `retain`，Go 在原 slice 上逆序删除；二者都用逆序首次出现顺序组织可用引擎文本。
- Rust 的 `GetPushDownCtxFromBuildPBContext` 直接传 `GetExprCtx()`，Go 传 `GetExprCtx().GetEvalCtx()`；这是 Rust `expression::NewPushDownContext` 类型接口的适配，不应机械改成 Go 调用链。
- Go 的隔离读错误是 `plannererrors.ErrInternal.GenWithStackByArgs`，Rust 当前是 `expression::errors::New` 文本错误。现有测试验证消息和分支行为，未验证错误类型/错误码完全一致。

## 扩展指南

- 扩展递归遍历时优先修改 `RecursiveSlice`/`RecursiveFlattenIter::next`，并在独立文件 `pkg/planner/util/slice_recursive_flatten_iter_test.rs` 增加空层级、深层级、下标和提前终止用例；不要把测试嵌入 `misc.rs`。若希望直接接受新的容器形态，需要明确评估是否破坏当前无反射、借用式 API。
- 新增计划字段克隆规则时，先确认元素的共享安全语义，再选择标准 `Clone` 或领域专用复制方法；若它服务计划缓存生成，还需同步 `pkg/planner/core/generator/plan_cache/plan_clone_generator.rs` 及其独立测试，防止只改工具却没有生成调用。
- 修改别名提取时应覆盖无表名、DB/Tbl 不完整、多表、缺省数据库、子查询显式别名和 query block offset；兼容 Go 的空数据库名规则，避免 hint 错配到其他表。
- 修改 `PushDownContext` 参数时需同步 `plan-base::BuildPBContext` 与 `expression::NewPushDownContext` 的字段契约，并检查 cascades 及其他表达式下推入口是否遗漏 warning handler 或 explain 状态。
- 增加存储引擎或改变隔离读规则时，核心接入点是 `FilterPathByIsolationRead` 的“可用引擎收集”和 `retain` 谓词，以及 `ShouldCheckTiFlashPushDown` 的专属策略。同步更新 `pkg/planner/util/misc_test.rs` 和 `pkg/planner/core/integration_test.rs`，至少覆盖系统库绕过、混合引擎、零路径错误、错误文本顺序、严格模式提示及 MPP warning。该路径位于会话建表源主链，错误兼容性和候选裁剪会直接影响可执行计划；额外集合扫描也应避免引入与路径数平方相关的高常数。

## 验证依据

- Rust 源码与模块边界：`pkg/planner/util/misc.rs`、`pkg/planner/util/lib.rs`、`pkg/planner/util/Cargo.toml`。
- Go 对照：`pkg/planner/util/misc.go`；递归迭代的原始 Go 用例为 `pkg/planner/util/slice_recursive_flatten_iter_test.go`。
- 独立 Rust 测试：`pkg/planner/util/slice_recursive_flatten_iter_test.rs` 覆盖空树、嵌套顺序、空节点、continue 后下标和 break；`pkg/planner/util/misc_test.rs` 覆盖隔离过滤错误中的逆序引擎列表，以及零路径时仍产生 MPP warning。
- 集成证据：`pkg/planner/core/integration_test.rs::test_none_access_paths_found_by_isolation_read` 覆盖 TiFlash-only 拒绝 TiKV、系统库绕过和混合引擎保留；`pkg/session/runtime/planning.rs::populate_session_data_source` 是当前 Rust 会话规划主链调用点。
- 物理规划调用：`pkg/planner/core/operator/physicalop/{physical_projection.rs,base_physical_agg.rs,physical_selection.rs,physical_limit.rs,physical_window.rs,base_physical_plan.rs}`；表达式下推调用为 `pkg/planner/cascades/old/transformation_rules.rs`。
- RustCodeGraph：`status` 显示本仓库索引含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/planner/util` 找到目标及对照测试；`node --file` 读取 `misc.rs`、`misc.go` 与两份迭代/隔离测试；精确 `query` 区分了同名 Go/Rust `FilterPathByIsolationRead`，其 callers 结果识别到 Rust 单测和 planner core 集成测试，但部分生产 Rust 调用边缺失，已用源码搜索与调用点读取补足。
- 本任务是纯文档分析，未运行 Cargo。交付前执行任务指定的结构命令，确认目标文件存在且恰有 11 个固定二级标题；并人工复核本文未把未接线 API、错误类型等未证事实描述为已支持。
