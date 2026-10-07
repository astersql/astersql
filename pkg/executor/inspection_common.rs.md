# `pkg/executor/inspection_common.rs`

## 文件定位

本文对应源码 [`inspection_common.rs`](inspection_common.rs)。本文件属于 `astersql-executor` crate；crate 根由 `pkg/executor/Cargo.toml` 的 `[lib] path = "lib.rs"` 指定，`pkg/executor/lib.rs` 以 `pub mod inspection_common;` 将其公开。它承载巡检规则目录的 Rust 侧行物化逻辑，目标语义对应 Go 的 `pkg/executor/inspection_common.go`，即为 `INFORMATION_SCHEMA.INSPECTION_RULES` 生成 `(name, type, remark)` 三列。

当前接线状态需要特别区分：`inspectionRuleRetriever`、两个类型常量和 `retrieve` 虽然是公开 API，但仓库内除本文件外没有 Rust 构造点或调用点。Go 主链已经由 `pkg/executor/builder.go` 的 `TableInspectionRules` 分支接到 `MemTableReaderExec`；Rust `pkg/executor/builder.rs` 尚无对应接线。因此本文件目前是可复用的已移植组件，不应描述为已经承接 Rust SQL 查询主链。

## 核心职责

- 用 `inspectionRuleTypeInspection`（`"inspection"`）和 `inspectionRuleTypeSummary`（`"summary"`）统一两类规则的对外类型值。
- 由 `inspectionRuleRetriever` 持有调用方提前取得的规则名快照、类型过滤集合以及一次性读取状态。
- `retrieve` 根据 `requested_types` 选择类别，将每条规则物化成三个 `Datum`：规则名、类型、空备注。
- 对 summary 名称副本排序，保证来自无序注册表的名称能够稳定输出；inspection 名称则保持传入快照的原顺序。
- 通过 `retrieved` 和 `skip_request` 实现一次性读取及规划期判空后的快速返回。

本文件不负责解析 SQL 谓词、读取全局注册表、执行巡检规则，也不负责把检索器装入内存表执行器。谓词提取的 Rust 事实位于 `pkg/planner/core/memtable_predicate_extractor.rs::InspectionRuleTableExtractor`，实际巡检执行位于 `pkg/executor/inspection_result.rs`，summary 注册表位于 `pkg/executor/inspection_summary.rs::inspectionSummaryRules`。

## 主要符号

- `pub const inspectionRuleTypeInspection: &str`：inspection 明细规则的持久类型标签，值为 `"inspection"`。
- `pub const inspectionRuleTypeSummary: &str`：summary 汇总规则的持久类型标签，值为 `"summary"`。
- `pub struct inspectionRuleRetriever`：一次检索所需的全部快照状态。
  - `retrieved`：是否已完成首次物化。
  - `skip_request`：上游判定谓词不可能满足时的短路标记。
  - `requested_types: BTreeSet<String>`：请求的类型集合；空集合表示两类都允许。
  - `inspection_rule_names`：inspection 规则名称快照，顺序原样保留。
  - `summary_rule_names`：summary 规则名称快照，物化前排序。
- `fn type_enabled(&self, rule_type: &str) -> bool`：内部过滤判断。集合为空或精确包含类型字符串时返回 `true`；它不做大小写折叠或别名处理。
- `pub fn retrieve<C>(&mut self, _ctx: C) -> Vec<Vec<Datum>>`：公开物化入口。泛型上下文仅为接口形状保留，当前实现完全不读取 `_ctx`，也不返回 `Result`。

文件没有 trait、enum、条件编译项或模块级可变状态。

## 执行流程

1. `retrieve` 先检查 `retrieved || skip_request`；任一为真即返回空 `Vec`，且不改变其他字段。
2. 首次有效调用立即把 `retrieved` 设为 `true`。因此后续调用始终为空，即使第一次得到的规则列表本身为空。
3. 若 `type_enabled("inspection")`，按 `inspection_rule_names` 的迭代顺序追加行；每行为 `[NewStringDatum(name), NewStringDatum("inspection"), NewStringDatum("")]`。
4. 若 `type_enabled("summary")`，先克隆 `summary_rule_names`，在局部副本上按 Rust 字符串自然序排序，再以同样的三列形状追加，类型列改为 `"summary"`。
5. 返回累计行。两类都启用时，所有 inspection 行必定位于所有 summary 行之前；代码不对同类中的重名项去重。

`requested_types` 含未知字符串且不含两个已知类型时，首次调用会设置 `retrieved = true` 并返回空行。`skip_request` 为真时则在设置 `retrieved` 之前短路。

## 数据与状态

检索器是请求级、可变的一次性对象。`BTreeSet<String>` 为类型过滤提供确定的集合语义，但本文件只进行成员测试，不依赖集合遍历顺序。两个 `Vec<String>` 是注册表名称快照：注释明确其目的在于避免物化和排序期间长期持有注册表锁；本文件自身不获取锁，也不保留注册表引用。

输出的每一行严格包含三个 `astersql_types::datum::Datum`，全部通过 `NewStringDatum` 构造。第三列 remark 当前固定为空字符串，对应 Go 文件中的 “TODO: add rule explanation”。输入名称被克隆或移动进输出，原 `inspection_rule_names` 保留，summary 排序只修改局部克隆。

状态不变量包括：一次有效调用后 `retrieved == true`；返回行只可能带两个常量定义的类型；summary 输出有序；本文件不会修改过滤集合或两个源快照。

## 依赖与调用关系

直接标准库依赖只有 `std::collections::BTreeSet`；直接工作区依赖是 `astersql-types` 的 `Datum` 和 `NewStringDatum`，由 `pkg/executor/Cargo.toml` 中 `astersql-types = { path = "../types" }` 声明。该逻辑不受 `nextgen` feature 控制。

预期的完整链路由 Go 对照和 Rust 现有组件共同给出：SQL 对 `inspection_rules.type` 的谓词先由 `InspectionRuleTableExtractor` 提取为 `Types/SkipRequest`；builder 创建检索器；检索器读取 inspection 与 summary 规则名并物化行；内存表执行器把行返回给 SQL 层。Rust 上游提取器已存在，其 `Types`/`SkipRequest` 与本文件的 `requested_types`/`skip_request` 可直接对应，但 Rust builder 尚未建立这条边。

Rust 侧规则来源也仍是分离的：`pkg/executor/inspection_result.rs::inspectionResultRetriever::retrieve` 内部构造五个 inspection 规则名（`config`、`version`、`node-load`、`critical-error`、`threshold-check`）；`pkg/executor/inspection_summary.rs::inspectionSummaryRules` 返回 summary 规则映射。本文件没有直接调用这两个位置，而是要求未来构造方形成名称快照后注入。

## 错误处理与边界

`retrieve` 没有错误返回路径：字符串克隆、排序和 Datum 构造均被视为内存内的不可恢复操作；分配失败会遵循 Rust 运行时行为，而不是转成业务错误。与 Go 签名返回 `([][]types.Datum, error)` 不同，Rust 当前返回裸 `Vec<Vec<Datum>>`，因此未来接入统一 retriever trait 时不能凭空宣称已有错误传播能力。

边界行为如下：重复调用为空；`skip_request` 为空；空类型集合表示不过滤，而不是“没有类型”；未知或大小写不同的类型不会匹配；空名称仍会被物化；重复名称不会消除；inspection 列表不在本文件排序。调用方必须保证传入快照和 SQL schema 的三列契约正确。

上游互斥谓词的真实证据在 `InspectionRuleTableExtractor`：Rust 测试 `pkg/planner/core/operator/logicalop/logicalop_test/logical_mem_table_predicate_extractor_test.rs::TestInspectionRuleTableExtractor` 验证 `type IN ('inspection','summary')` 提取两个类型，而同时要求 `type='inspection'` 与 `type='summary'` 会设置 `SkipRequest`。

## 并发与资源生命周期

类型未实现内部同步；`retrieve(&mut self, ...)` 要求独占可变借用，单个实例不能在安全 Rust 中被多个线程同时调用。它没有 `Arc`、锁、通道、异步任务、事务或外部 I/O，资源生命周期仅限所拥有的集合、字符串向量和临时输出。

快照策略把并发责任放在构造方：构造方应在必要的注册表同步边界内复制名称，然后释放锁，再调用本文件排序和物化。这样可以避免在潜在较长的输出分配阶段持锁。由于仓库内目前没有 Rust 构造方，具体锁类型和快照时点尚未验证，不能从注释推断为已经落地。

## 与 Go 版本的对应关系

`pkg/executor/inspection_common.go` 是直接语义来源。两版均有同名两类常量、只取一次/跳过逻辑、空过滤表示全选、inspection 先于 summary、summary 排序、三字符串列及空 remark。

主要差异是数据获取和接口形状：

- Go 结构持有 `*plannercore.InspectionRuleTableExtractor`，运行时直接遍历包级 `inspectionRules` 与 `inspectionSummaryRules`；Rust 结构把 `skip_request`、类型集合和两个名称快照拆成自有字段。
- Go 的 inspection 名来自实现 `name()` 的规则对象；Rust 只接收名称字符串，无法在这里访问规则行为。
- Go 接受 `context.Context` 与 `sessionctx.Context` 并返回 error；Rust 接受未使用的泛型上下文并返回裸向量。
- Go 已在 `builder.go` 接入 `TableInspectionRules`，Rust 没有对应 builder 引用。

Go 集成用例 `tests/integrationtest/t/executor/inspection_common.test` 及结果文件验证当前 Go 注册表共 15 行，其中 inspection 5 行、summary 10 行、互斥的双重等值条件为 0。它们可证明对照语义和当前 Go 目录数量，但不能证明 Rust 检索器已接线或 Rust 快照数量必然相同。

## 扩展指南

- 新增规则类型时，应同时扩展类型常量、`type_enabled` 驱动的物化分支、上游 `InspectionRuleTableExtractor` 的可接受值及 SQL schema 契约；还要决定新类别相对 inspection/summary 的稳定输出顺序。
- 增加 remark 时，应把输入从单纯名称升级为带说明的快照结构，并同步 Go 的 TODO 和三列输出测试，避免仅在一侧填充。
- 完成 Rust 主链接线时，应在 builder 的 `TableInspectionRules` 分支从 planner extractor 映射 `Types/SkipRequest`，并从 `inspection_result.rs` 与 `inspection_summary.rs` 获取名称快照；快照必须在注册表同步边界内完成，本文件外再释放锁。
- 若要改变 inspection 的排序或去重策略，必须先确认 Go 迭代顺序契约和现有集成结果，不能只因 summary 已排序就擅自统一排序。
- Rust 回归测试应放在独立文件（建议 `pkg/executor/inspection_common_test.rs`），并在 `pkg/executor/lib.rs` 的 `#[cfg(test)]` 模块区接入。至少覆盖空过滤、单类型、双类型、未知类型、`skip_request`、重复调用、summary 排序、inspection 保序、重复/空名称和三列 remark。builder 接线完成后还需增加 SQL 层集成覆盖；不要把测试内嵌到本生产源文件。

兼容风险集中在可见行数、名称与排序；性能风险主要是两个名称列表的克隆和 summary 的 `O(n log n)` 排序。当前规则规模很小，但若目录显著增长，可考虑让构造方生成已排序快照，同时仍需保持 Go 可观察顺序。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件，目标文件已索引；`files --filter pkg/executor/inspection_common.rs` 报告该文件含 4 个符号。
- RustCodeGraph `node --file pkg/executor/inspection_common.rs --offset 1 --limit 260`：核对完整 87 行源码、字段、分支、行形状与排序逻辑。
- RustCodeGraph `query inspectionRuleRetriever`、`query type_enabled`、`query retrieve`：核对同名 Go/Rust 定义与内部方法；`explore` 确认 `type_enabled` 被 `retrieve` 调用。精确限定同名 `retrieve` 的 callers/callees 查询未在时限内返回，因此再用仓库文本引用核验调用面。
- RustCodeGraph `node InspectionRuleTableExtractor` 与 `node TestInspectionRuleTableExtractor`：核对 Rust/Go 提取器字段以及 Rust 谓词边界测试。
- 已读源码与配置：`pkg/executor/inspection_common.rs`、`pkg/executor/Cargo.toml`、`pkg/executor/lib.rs`、`pkg/executor/inspection_common.go`、`pkg/executor/builder.go`、`pkg/planner/core/memtable_predicate_extractor.rs`、`pkg/planner/core/memtable_predicate_extractor.go`、`pkg/executor/inspection_result.rs`、`pkg/executor/inspection_summary.rs`。`pkg/executor` 下未发现 `doc.go`。
- 已读测试证据：`pkg/planner/core/operator/logicalop/logicalop_test/logical_mem_table_predicate_extractor_test.rs`、同路径 Go 测试、`tests/integrationtest/t/executor/inspection_common.test` 与 `tests/integrationtest/r/executor/inspection_common.result`。仓库搜索未发现直接构造 `inspectionRuleRetriever` 的 Rust 测试。
- `rg` 核验：Rust 中目标符号仅在本文件出现；`pkg/executor/lib.rs` 仅完成模块导出，`pkg/executor/builder.rs` 尚无规则表接线。这是本文将当前迁移状态标为“实现存在、主链未接”的依据。
