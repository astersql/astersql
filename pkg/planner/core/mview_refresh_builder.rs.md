# `pkg/planner/core/mview_refresh_builder.rs`

## 文件定位

[`mview_refresh_builder.rs`](mview_refresh_builder.rs) 是 `astersql-planner-core` 的公开模块，由 [`lib.rs`](lib.rs) 声明并再导出。它接收 typed 刷新请求和显式 MV 元数据，生成刷新计划或执行端布局；不是 SQL 解析器或刷新事务调度器。本文以冻结提交 `2c944fbcbfb2c904cc8cbddafe17954138bd8867` 为准，增量范围为 `ad0a6f48804e48373cc79c5f53f0072daf99ceb9..2c944fbcbfb2c904cc8cbddafe17954138bd8867`；不纳入工作区后续源码。

## 核心职责

构建公开 REFRESH/DRY RUN/PROFILE；为 FAST 计算增量列、聚合依赖、MV handle、commit-ts 窗口和 MIN/MAX lookup 候选索引；为 COMPLETE DELTA APPLY 计算两侧行像布局，并把结果包装成轻量物理计划。

## 主要符号

`RefreshRequest`/`RefreshObserveType`/`RefreshPlan` 表示用户请求与 observe 输出。`MViewDefinition` 保存表 ID、MV/base/mlog 列、group keys、聚合及索引；`MViewBuildOptions` 保存 from/to 时间戳。`MViewHandleKind` 区分整数主键、common handle、额外 rowid。`BuildMView` 返回 `MViewBuildResult`；`BuildCompleteDiffSource` 返回 `CompleteDiffBuildResult`；`BuildRefreshMaterializedViewImplement` 按 `RefreshImplementMode::{Fast, CompleteDeltaApply}` 返回两个执行端计划之一。

## 执行流程

公开请求先校验 view 和数据库，空 database 回退 current_database，observe 计划输出单列 `refresh steps`。FAST 先校验定义、要求 COUNT(*)，按聚合输出生成 delta 列，再追加 MV 列与可选 rowid。COUNT(*) 依赖 delta offset 0；COUNT/SUM 各一列；MIN/MAX 各有 added value/count、removed value/count 四列。nullable SUM 必须有同表达式 COUNT，依赖再追加 `delta_column_count + count.mv_offset`；nullable MIN/MAX 有匹配 COUNT 才追加。匹配会去空白、反引号并转 ASCII 小写，不去括号，也不做 AST 语义等价判定。

FAST 时间窗口为 `(from_ts, to_ts]`，to_ts=0 时省略上界；结果记录 `read_from_tiflash=true`。group key 不可空用 `=`，可空用 `<=>`。实现入口实际创建 Selection(TableScan($mlog#ID))，MIN/MAX 再附无 children 的 IndexJoin 描述及占位 range；有 MIN/MAX 而无支持索引时拒绝。target_refresh_read_ts>0 才附 snapshot 值。

COMPLETE DELTA APPLY 选第一列可见元数据中的 NOT NULL 列为 marker，布局为 op、可选旧 rowid、当前行像、重算行像；group keys 及非 key payload 的比较符按可空性选择。实现入口只创建 HashJoin 描述并包装 sink，不生成完整 full outer join AST/子树。

## 数据与状态

所有偏移为 usize，delta 依赖与 MV 内部偏移属于不同坐标系。整数/common handle 在 FAST source 中加 delta 列基址；额外 rowid 追加到 source 尾部。COMPLETE op offset=0，额外 rowid offset=1，当前行像从 2（无额外 rowid 从 1）开始。`ValidateSourceLayout(0)` 不比对外部 schema 长度，非零参数才检查长度；它检查列数、marker/group key/handle 范围及行像数量，未逐项验证所有映射的唯一性和顺序。

## 依赖与调用关系

[`planbuilder.rs`](planbuilder.rs) 的 typed `Build` 分派公开和内部刷新语句，权限收集在该入口完成。构建器使用 [`common_plans.rs`](common_plans.rs) 的共享节点和物化视图结构，索引候选调用 [`mview_refresh_lookup.rs`](mview_refresh_lookup.rs)。[`../mview/mview.rs`](../mview/mview.rs) 公开再导出构建与 lookup API。executor `builder.rs` 有同名 PlanData trait 与依赖工厂分派，但本文件没有实现这些 trait 或直接调用 executor；不能仅凭相同名字宣称端到端 SQL 刷新接通。

## 错误处理与边界

错误为 `Result<_, String>`，包括空 MV 列、空/越界 group key、元数据长度不符、mlog 缺 old_new/dml_type、缺 COUNT(*)、聚合输出越界、nullable SUM 无 COUNT、handle 越界、COMPLETE 无 NOT NULL marker。定义校验并不要求 mlog commit-ts 列实际存在，也不验证 from<=to；内部构建不查 InfoSchema/GC/权限。CommonPrimaryKey 的 FAST 要求非空 offsets，COMPLETE 分支没有同样的非空检查。完整 COMPLETE 刷新策略选择不在该二值枚举中。

## 并发与资源生命周期

同步内存计算，使用 HashMap/HashSet、String/Vec 和拥有子树的 PlanNode，无锁、线程、异步或存储句柄。snapshot 是时间戳值。重用输入通过不可变借用，结果拥有克隆的列名/映射；大定义产生相应分配成本。

## 与 Go 版本的对应关系

对照 [`mview_refresh_builder.go`](mview_refresh_builder.go) 的公开计划与内部物理计划构建，以及 [`../mview/mview.go`](../mview/mview.go) 的 delta/complete diff 规划。Go 使用 session、InfoSchema、parser AST、优化结果、GC 和 ranger；Rust 以 typed 元数据及轻量节点表达可验证子集。read_from_tiflash 标志、join operator 字符串及 lookup 描述不能等同 Go 实际 AST hint、聚合/连接执行和优化器提取。

## 扩展指南

新增聚合时同步 delta 列命名、依赖顺序、nullable 规则、executor 消费布局与独立测试。新增刷新模式须分别检查用户入口、内部模式、权限与真实 runtime adapter。保持 COUNT(*) 和 handle 约束，扩展校验不要把元数据标志当作真实存储行为。测试继续放在独立文件。

## 验证依据

本文以冻结提交 `2c944fbcbfb2c904cc8cbddafe17954138bd8867` 为准，增量范围为 `ad0a6f48804e48373cc79c5f53f0072daf99ceb9..2c944fbcbfb2c904cc8cbddafe17954138bd8867`；不纳入工作区后续源码。

已审计新增源码全体、core `lib.rs`、core 与 planner 的 `Cargo.toml`、planner `lib.rs`，以及独立 `core/casetest/mview/mv_refresh_test.rs`。RustCodeGraph 当前索引包含 Rust；冻结事实采用 `git show`/`git diff`，不将当前图关系当成冻结调用关系。目标 core/mview 包均无 `doc.go`。Ready 技能目录缺失，使用任务指定结构、diff、空白和相对链接检查；纯文档任务不运行 Cargo/Go 测试，不把测试源码当作已执行结果。

静态测试证据覆盖 observe schema、COUNT/SUM/MIN/MAX 依赖、可空性、窗口、索引可见性、snapshot、complete 行像/handle、ExplainInfo 及 typed 权限分派。部分测试名称提及表达式变体，但实现只做字符串规范化，本文未据名称扩大语义。
