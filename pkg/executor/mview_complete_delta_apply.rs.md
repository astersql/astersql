# `pkg/executor/mview_complete_delta_apply.rs`

## 文件定位

按输入顺序将差异转换为 insert、delete 或 update。

## 核心职责

- 按输入顺序将差异转换为 insert、delete 或 update。
- 比较 UPDATE 新旧行，只标记变化列，无变化时跳过写入和计数。
- 将整组变更交给 runtime 的原子执行边界，成功后返回三类操作统计。
- 拒绝未知操作码与 UPDATE 列数不匹配，传播 batch/runtime 错误。

## 主要符号

`DIFF_INSERT=1`、`DIFF_DELETE=2`、`DIFF_UPDATE=3` 对应 Go 的差异操作码。`DeltaRow` 保存 `op`、`current: Vec<Datum>` 与 `recomputed: Vec<Datum>`，可克隆；它没有表 ID、列映射或 handle 元数据。

`MutationBatch` 提供 `insert(row)`、`delete(row)`、`update(old, new, touched)`；`MutationRuntime: Send + Sync` 提供 `run_atomic`，接受可变回调，并要求成功发布全部写入、失败不发布任何写入。`ApplyStats` 是可复制、可比较的三个 `u64` 计数。`Result<T>` 的错误类型为 `String`。唯一入口 `apply(runtime, rows)` 返回 `Result<ApplyStats>`。

## 执行流程

1. `apply` 初始化全零统计，并调用一次 `run_atomic`；即使输入为空也会调用 runtime。
2. 回调逐行读取 `op`。INSERT 将 `recomputed` 交给 batch，成功后增加 inserted；DELETE 使用 `current`，成功后增加 deleted。
3. UPDATE 先要求两行列数相等，再以 `Datum::Equals` 对应比较，收集 `!Equals` 为 `Vec<bool>`。
4. touched 中至少一个值为真才调用 batch.update，并在成功后增加 updated；全部相等（包括两行均为空）跳过写入。
5. 未知操作码立即返回含操作码与零基行号的错误；其余 batch 错误由 `?` 传播。runtime 整体成功后才把统计返回调用方。

## 数据与状态

输入是借用的 `&[DeltaRow]`，本文件不改写输入，不物化额外结果集。每个 UPDATE 分配一个与列数相等的 touched 向量，比较成本随列数增长；INSERT/DELETE 不检查新旧列数或行内容合法性。

统计按成功完成的实际 batch 方法计数，无变化 UPDATE 不计入 updated。错误前局部统计可能已经增加，但 `apply` 最终返回错误，不向调用者暴露部分统计；真正数据回滚依赖 runtime。接口没有限制回调调用次数，也未实现重试统计复位，适配实现需明确回调执行契约，避免多次执行导致统计累加。

## 依赖与调用关系

唯一直接外部类型依赖为 `astersql_types::datum::Datum`；其余使用标准库向量、切片、字符串与 Result。`lib.rs` 在测试配置下单独挂载 `mview_complete_delta_apply_test.rs`。

抽象构建边为 `Plan::MViewCompleteDeltaApply -> buildMViewCompleteDeltaApply -> build_required_child -> build_mview_complete_delta_apply_executor`。计划提供 `target_table_id`，工厂接收已构建 child；本文件的 `apply` 却接收 runtime 和行切片，二者之间的子树拉取、行解码、表解析和事务适配不在此文件中。另一个 `MViewDeltaMerge` builder 分支与聚合合并模块不能当成本文件的调用证据。

## 错误处理与边界

UPDATE 列数不匹配报 `UPDATE row {idx} column count mismatch`；未知操作码报 `invalid diff op {op} at row {idx}`。错误在按顺序处理到该行时才发现，不是全输入预校验，因此 runtime 必须能撤销此前操作。

原子性是一项明确的 `MutationRuntime` 契约，函数自身不创建事务、不执行回滚。batch/runtime 可返回任意字符串错误，不提供结构化错误分类。表约束、外键、IGNORE、权限、handle 构造、语句 affected rows 和消息均由外部适配承担；`ApplyStats` 不是已经写入 session 的统计。

## 并发与资源生命周期

文件内顺序调用 batch，无线程、锁、异步任务或持久资源。runtime 可被共享（要求 Send + Sync），但此约束不能保证跨调用事务隔离；具体实现负责同步和事务边界。回调借用统计，batch 的可变借用仅在 runtime 回调期间有效。

输入行存储由调用方拥有；touched 是逐 UPDATE 临时向量，调用结束即释放。没有 Open/Next/Close 状态或已执行标志，多次调用 `apply` 会重复申请并执行一批操作，幂等性须由上层保证。

## 与 Go 版本的对应关系

两边共享操作码、按差异执行写入、只更新变化列以及无变化 UPDATE 跳过的意图。

Rust 当前只实现已解码行的应用算法，逐列使用 `Datum::Equals`，没有 Go 的类型感知列比较、chunk 批比较和 touched bitmap 优化，也没有完整刷新执行器生命周期。

## 扩展指南

生产接线应在 builder 工厂及 runtime 中完成目标表解析、child 拉取/差异解码、事务回滚与 session 统计；扩展时逐段对照 Go 的列映射、类型比较与表 mutation 语义。保持整个批次出错不发布、写入成功才计数、无变化更新不写入的约束。

测试继续放在独立的 `mview_complete_delta_apply_test.rs`。新增 DELETE、列数错误、runtime 提交错误与类型边界验证时，应断言写入集合和回滚结果，而非只看返回成功；若增加重试，还需覆盖统计是否重复累计。性能优化可考虑复用 touched 缓冲，但必须保持 Datum 比较和计数语义。
