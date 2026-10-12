# Executor 大块注释残留审计

## 范围与结论

本报告覆盖 `00-candidates.tsv` 中全部 44 个 `pkg/executor/**` 候选（42 个文件）。逐项核对了注释后的可执行 Rust、模块/调用接线和相邻 `#[test]`；RustCodeGraph 用于类型、调用方和测试关系，注释与 raw string 用文本搜索补足。`pkg/executor/**` 不存在 `doc.go`。

分类计数：已实现重复档案 18；部分实现且注释仍描述缺口 13；非执行测试档案 12；合法说明误报 1。高风险顺序：未接统一执行器工厂的 hash join/aggregate 独立实现；join spill/并发取消的局部模型；raw-string 测试把归档完整性当覆盖；其余重复档案清理。

接线证据：join/aggregate/sort/union 模块均由各自 `lib.rs` 编译；RustCodeGraph 找到 `HashJoinV1Exec` 测试构造、`MergeJoinExec::new`、querywatch `next -> from_option_list` 以及 querywatch 的 manager 调用链。根 `builder.rs` 只明确分派 `CalibrateResource`，没有找到这些独立 `HashAggExec`/`HashJoinV2Exec`/`UnionExec` 类型进入统一 SQL builder 的证据。因此“模块可执行”和“生产 SQL 路径已接线”在下列判断中严格区分。

行为测试证据：aggregate 覆盖 grouped/empty/stream、typed spill/restore/IO 清理；join 覆盖 V1/V2、inner/outer/full/semi/anti、NULL/composite key、chunk 边界、spill/reopen/cancel/error 与 stats；sort 覆盖串并行排序、TopN、spill、fallback、kill/error/close；querywatch/calibrateresource/union 均有可运行行为测试。仅检查 `GO_REFERENCE`/`_GO_DRAFT_ARCHIVE` 字符串片段的测试不计为行为覆盖。

## Aggregate

候选：`pkg/executor/aggregate/agg_hash_base_worker.rs:19-66`
分类：已实现重复档案。来源意图：共享 worker 完成信号、聚合函数、chunk、统计与 tracker。现有实现：`BaseHashAggWorker` 从第 72 行起实现完成状态和对齐长度；测试覆盖 finish signal 与对齐。缺失/风险：未见生产 builder 接线，档案可误导为完整 Go worker。建议：接线确认后删除注释。

候选：`pkg/executor/aggregate/agg_hash_executor.rs:18-232`
分类：部分实现且注释仍描述缺口。来源意图：HashAgg 单/并行编排、结果 channel 与 spill。现有实现：`HashAggExec` 有 open/next/execute/close 和 spill 标志，行为测试覆盖 grouped、empty、chunking。缺失/风险：Rust 实现采用本地数据模型，未证明进入统一 SQL builder，Go 的 goroutine/channel/cancel/错误优先级未等价复现。建议：保留为移植清单，先补 builder 与取消/错误测试。

候选：`pkg/executor/aggregate/agg_hash_final_worker.rs:19-320`
分类：部分实现且注释仍描述缺口。来源意图：合并 partial map、恢复 spill、发送最终 chunk。现有实现：`HashAggFinalWorker` 可 merge/restore/generate；typed spill 测试覆盖恢复。缺失/风险：无 channel 背压、finish/cancel 竞态和首错传播等价证据。建议：补并发协议后再删档案。

候选：`pkg/executor/aggregate/agg_hash_partial_worker.rs:18-73`
分类：已实现重复档案。来源意图：group key、partial 更新、final 分片与 spill。现有实现：worker 及 `PartialResultSpill` 可执行，测试覆盖 Murmur3 分片、spill 合并和序列化清理。缺失/风险：与 Go tracker 触发时序仍由 executor 候选承载。建议：删除该局部结构档案，保留跨 worker 差异清单。

候选：`pkg/executor/aggregate/agg_spill.rs:19-451`
分类：部分实现且注释仍描述缺口。来源意图：并行 HashAgg spill 状态机、OOM action、分区恢复。现有实现：helper 状态、磁盘分区和序列化可执行且有 typed spill 测试。缺失/风险：`ParallelAggSpillDiskAction` 仍是 Go 风格空参数占位，OOM action 优先级/回退未完整接线。建议：实现真实 tracker action 并覆盖并发触发。

候选：`pkg/executor/aggregate/agg_stream_executor.rs:18-106`
分类：已实现重复档案。来源意图：按排序 group 边界消费、刷新内存并输出。现有实现：`StreamAggExec` open/next/required rows/close 可执行；测试覆盖边界。缺失/风险：统一 builder 接线未证实。建议：接线核对后删除注释。

候选：`pkg/executor/aggregate/agg_util.rs:18-339`
分类：部分实现且注释仍描述缺口。来源意图：group-key 编码、聚合状态、统计、spill action 选择。现有实现：本地 `Value/Row/AggState`、distinct、merge、runtime stats 可执行。缺失/风险：类型系统和编码不是完整 TiDB datum/collation/SQL mode 语义。建议：不要宣称完整对齐；迁移到共享表达式/类型层后清理档案。

## Join

候选：`pkg/executor/join/base_semi_join.rs:22-257`
分类：已实现重复档案。现有实现与测试覆盖 semi/anti、NULL、other condition、恢复和 chunk 续跑；来源意图中的 probe 状态已可执行。风险：局部模型与统一 builder 接线未证实。建议：删除重复注释前保留语义矩阵文档。

候选：`pkg/executor/join/concurrent_map.rs:22-107`
分类：已实现重复档案。`ConcurrentMap` 实现 320 shard、碰撞链和内存估算；测试覆盖并发插入/遍历。风险低。建议：可删除档案。

候选：`pkg/executor/join/hash_join_spill.rs:22-108`
分类：已实现重复档案。OOM action、fallback、阈值和 helper 调用已实现；测试覆盖低于额度、只触发一次与 fallback。风险：真实 tracker 集成范围有限。建议：与 helper 一起完成集成验证后删除。

候选：`pkg/executor/join/hash_join_spill_helper.rs:21-655`
分类：部分实现且注释仍描述缺口。已有 build/probe spill、分区选择、restore stack、round limit 和清理测试。缺失/风险：磁盘层为内存化 `SpillDisk`，真实 I/O 失败、并发等待和取消语义不完整。建议：移植真实磁盘容器并保留故障测试。

候选：`pkg/executor/join/hash_join_stats.rs:21-422`
分类：已实现重复档案。V1/V2 stats Clone/Merge/Display 与 spill 格式可执行，测试覆盖所有计数器格式。建议：可删除注释。

候选：`pkg/executor/join/hash_join_v1.rs:18-1448`
分类：部分实现且注释仍描述缺口。`HashJoinV1Exec` 支持 open/next/close、inner/outer/full、spill 与 reopen，测试覆盖语义和统计。缺失/风险：没有统一 SQL builder 接线证据；Go pipeline 的 goroutine、channel、kill/首错协议被同步模型简化。建议：补生产接线和并发故障矩阵。

候选：`pkg/executor/join/hash_join_v2.rs:18-1571`
分类：部分实现且注释仍描述缺口。已有分区 row table、build/probe、outer unmatched、spill restore 与 V2 测试。缺失/风险：实现明确依赖 non-moving runtime 假设，且未证明工厂选择 V2；真实磁盘/取消协议不完整。建议：标为高风险待接线，不删除档案。

候选：`pkg/executor/join/hash_table_v1.rs:18-725`
分类：已实现重复档案。hash context、NULL-aware bucket、碰撞链、row container 与 spill 路径有行为测试。建议：可删除档案。

候选：`pkg/executor/join/hash_table_v2.rs:20-296`
分类：已实现重复档案。subtable/tagged pointer、build/lookup/replace/clear 和内存边界有测试。风险：unsafe/指针假设需保留设计说明而非 Go 全文。建议：提炼不变量后删除档案。

候选：`pkg/executor/join/index_lookup_hash_join.rs:19-636`
分类：部分实现且注释仍描述缺口。Rust 的 index lookup join 测试覆盖顺序、NULL-safe、多 key、错误、取消、reopen 和 stats；但候选文件未见对应完整可执行主类型。建议：核对功能由 `index_lookup_join.rs` 吸收后再删除，避免重复实现。

候选：`pkg/executor/join/index_lookup_join.rs:22-674`
分类：已实现重复档案。outer/inner worker、批量 lookup、ordered/unordered 结果与取消错误路径均有大量行为测试。风险：生产 builder 接线未从 Rust 图中证实。建议：接线核对后删除。

候选：`pkg/executor/join/index_lookup_merge_join.rs:22-564`
分类：已实现重复档案。外侧批处理、内侧排序和 merge join 已实现；测试覆盖 multi-key、reopen、limit 不挂起。建议：接线核对后删除。

候选：`pkg/executor/join/inner_join_probe.rs:22-120`
分类：已实现重复档案。probe 重复 key、condition、容量、越界和 spill/restore 有测试。建议：可删除档案。

候选：`pkg/executor/join/join_row_table.rs:22-227`
分类：已实现重复档案。row segment、raw layout、null map、atomic used flag 有构建和 clone 测试。建议：提炼布局不变量后删除档案。

候选：`pkg/executor/join/join_table_meta.rs:21-364`
分类：已实现重复档案。row layout、key 模式、offset 和边界由 `join_table_meta_test.rs` 覆盖。建议：可删除档案。

候选：`pkg/executor/join/joiner.rs:22-1287`
分类：部分实现且注释仍描述缺口。inner/outer 与 projection、false/null/matched 状态有行为测试。缺失/风险：Go expression/chunk/session warning 语义映射为局部 Row/closure，不能证明所有 SQL 类型与错误传播。建议：保留差异清单并接共享表达式层。

候选：`pkg/executor/join/left_outer_semi_join_probe.rs:19-242`
分类：已实现重复档案。left outer semi/anti marker、NULL、other condition、spill/chunk 容量测试齐全。建议：可删除档案。

候选：`pkg/executor/join/merge_join.rs:19-387`
分类：已实现重复档案。`MergeJoinExec::new` 有模块内构造，行为测试与 brute-force 对照覆盖 inner/outer/group 推进。风险：统一 builder 接线未证实。建议：接线确认后删除。

候选：`pkg/executor/join/outer_join_probe.rs:19-419`
分类：已实现重复档案。左右 outer、default row、condition error、reset/chunk 边界和 spill 有测试。建议：可删除档案。

候选：`pkg/executor/join/row_table_builder.rs:22-609`
分类：部分实现且注释仍描述缺口。序列化 key、分区、segment 和过滤平衡有测试。缺失/风险：完整 TiDB datum/collation/decimal/time 编码和真实内存 tracker 未证明。建议：补类型矩阵后清理。

候选：`pkg/executor/join/semi_join_probe.rs:22-332`
分类：已实现重复档案。semi/anti、duplicate、NULL、other condition、resume/spill 测试齐全。建议：可删除档案。

## Internal 与其他执行器

候选：`pkg/executor/internal/calibrateresource/calibrate_resource.rs:22-643`
分类：部分实现且注释仍描述缺口。duration、quota、TiDB/TiFlash 动静态校准与 executor 单次输出均有测试，`builder.rs` 有 CalibrateResource 分派。缺失/风险：指标获取使用注入式服务模型，需继续验证真实 PD/Prometheus 接线和错误兼容。建议：完成生产服务集成后删档案。

候选：`pkg/executor/internal/querywatch/query_watch.rs:22-239`
分类：部分实现且注释仍描述缺口。`next`、ADD/DROP、record 校验、normalize/digest 与 manager trait 可执行，测试覆盖 exact/similar、错误顺序和副作用。缺失/风险：通过 `include!` 复用 parser digester，本地 SHA 适配与真实 domain manager 接线需持续核对。建议：改用稳定 parser API 后清理。

候选：`pkg/executor/select.rs:179-184`
分类：合法说明/示例误报。六行注释说明 SELECT production boundary，不含失活实现；实际 select 路径调用 distsql。建议：保留，无处置。

候选：`pkg/executor/unionexec/union.rs:22-265`
分类：部分实现且注释仍描述缺口。`UnionExec` worker、结果 channel、panic/error/close/reopen 有行为测试。缺失/风险：未找到统一 builder 构造，Go 的 resource pool 与 context cancellation 仅局部模拟。建议：补生产接线后删除档案。

## Raw string 与块注释测试档案

以下候选均归类为“非执行测试档案”：原始 Go 测试文本不参与编译；其后若有 Rust `#[test]`，只按实际断言计覆盖，单纯 `contains`/片段顺序检查不计行为覆盖。

候选：`pkg/executor/importer/import_test.rs:24-612`
证据：`GO_REFERENCE` 后有 Rust 参数、格式、压缩、存储等测试，但 `import_go_reference_is_preserved` 仅检查归档。风险：同文件还定义大量 Go 命名 stub，容易把模拟通过误认为生产导入覆盖。建议：删除归档存在性测试，保留/迁移真实行为用例。

候选：`pkg/executor/importer/importer_testkit_test.rs:22-334`
证据：归档后存在 checksum/post-process/chunk/resource 参数测试，但包含本地 stub。风险与建议同上，优先让测试调用生产类型。

候选：`pkg/executor/importer/table_import_testkit_test.rs:22-120`
证据：归档描述 IMPORT FROM SELECT 错误清理；实际 Rust 测试覆盖临时目录分支，但 `reference_preserves_go_contract` 只检查片段。建议：保留 filesystem 行为测试，删除归档完整性断言。

候选：`pkg/executor/sortexec/parallel_sort_spill_test.rs:21-175`
证据：归档之外存在并行 spill、全局有序、重复 spill、清理与 worker 错误测试。风险：归档本身无覆盖价值。建议：删除 raw string。

候选：`pkg/executor/sortexec/parallel_sort_spill_test.rs:239-380`
证据：第二段为 Go failpoint 测试档案；实际 Rust failpoint/磁盘失败行为由同目录测试覆盖。建议：以可执行失败注入测试替代该段。

候选：`pkg/executor/sortexec/parallel_sort_test.rs:13-268`
证据：可执行测试覆盖并行排序、重开、kill、child error 与内存释放；archive presence 不是行为。建议：删除归档。

候选：`pkg/executor/sortexec/rank_topn_test.rs:23-261`
证据：可执行测试覆盖 offset/count、ties、zero count；归档只保存 Go fixture。建议：将缺少的 case 数据化后删归档。

候选：`pkg/executor/sortexec/sort_spill_test.rs:23-452`
证据：可执行测试覆盖 serial spill、tracker 释放、OOM action 和 failpoint；归档不执行。建议：删除归档。

候选：`pkg/executor/sortexec/sort_spill_test.rs:528-566`
证据：Go `TestFallBackAction` 片段；实际 Rust `test_fall_back_action` 覆盖 fallback。建议：可直接删除该重复片段。

候选：`pkg/executor/test/executor/executor_test.rs:22-3191`
证据：大型 `GO_REFERENCE` 后有众多 Rust `#[test]`，但归档本身不执行。风险：综合 SQL 文本与本地 testkit 断言不等价于生产 SQL 全链路。建议：按独立行为逐步迁移后删除归档。

候选：`pkg/executor/test/showtest/show_test.rs:712-1103`
证据：块注释保存 SHOW CREATE/region split 等旧代码；相邻可执行测试已断言 region split、权限和配置输出。建议：删除重复块，保留实际断言。

候选：`pkg/executor/typed_hash_join_test.rs:61-92`
证据：raw string 保存 typed hash join 片段；真正 join 语义由 `join/*_test.rs` 执行。风险：本文件片段不提供行为覆盖。建议：删除或把尚缺 case 移入独立测试文件。

## 处置优先级

1. 高：为 HashAgg、HashJoin V1/V2、Union 与 index/merge join 建立统一 builder/dispatch 证据；补真实取消、首错、磁盘 I/O 和 tracker 测试。
2. 高：补 datum/collation/SQL mode 类型矩阵，避免局部 `Value/Row` 模型被误认作完整 SQL 语义。
3. 中：移除 importer/sort/executor/show 的 raw-string 或块注释归档及纯 `contains` 完整性测试，先把仍有价值的 case 转成可执行断言。
4. 低：删除已有完整实现和行为测试支撑的重复行注释；对 unsafe row layout 只保留精炼的不变量说明。
