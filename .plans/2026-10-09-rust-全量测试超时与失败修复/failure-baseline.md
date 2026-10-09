# Rust 全量测试失败基线

本基线来自 `target/rust-test.3mwXHM`。原日志包含两份相同的 nextest 结果段，原始行数为 736 `TIMEOUT`、142 `FAIL` 和 2 `SIGABRT`；按 `outcome + binary_id + test` 去重后为 368/71/1，共 440 个唯一结果。完整的机器可读映射见 `failure-baseline.tsv`。

## 诊断方法

- 每个失败 package 选取一个精确 `binary_id + test` 样本，用默认 profile、`--test-threads 1`、`--no-fail-fast` 复跑；84/84 个 package 均实际执行 1 个测试，无零测试。
- 默认复跑中 39 个 package 样本通过，13 个稳定失败，32 个仍在 10 秒超时。
- 对 32 个仍超时的样本使用 `diagnostic` profile（60 秒、单线程）复跑：29 个在 10–60 秒内通过，3 个仍超过 60 秒。
- TSV 中每个原始结果都有精确回归过滤器，并明确标记类别是由同 package 的一个精确样本推导，不把 package 样本证据冒充为 440 次独立复跑。

## 类别

| 类别 | 唯一测试数 | 判定 |
| --- | ---: | --- |
| `resource_contention_or_budget_boundary` | 249 | 对应 package 样本在默认 10 秒单线程复跑中通过 |
| `completes_in_10_to_60_seconds` | 119 | 默认复跑超时，60 秒复跑通过 |
| `stable_assertion_or_environment_failure` | 68 | 单线程复跑仍稳定失败 |
| `stack_overflow_abort` | 1 | 稳定复现 stack overflow/SIGABRT |
| `exceeds_60_seconds` | 3 | parser、parsergen 和 util-profile 样本在 60 秒仍超时 |

## 代表性证据与后续任务

- 任务 2：planner 样本稳定报 `synchronous statistics are pending but this PlanContext has no StatsHandle`。
- 任务 3：errors/parser-terror 样本稳定失败于 stack frame/file 位置语义。
- 任务 4：executor/server 样本稳定显示重复的 1062/1105 warning 集合差异。
- 任务 5：`cached_unary_agg_union_round_trip` 单独复跑稳定发生 stack overflow 并以 SIGABRT 终止。
- 任务 6–7：session bootstrap 样本显示升级版本 `317`/`262` 差异；其余 session 超时按生命周期边界继续收敛。
- 任务 8：并发、runtime 及其他资源竞争类归入此任务；其中 util-profile 样本超过 60 秒。
- 任务 9：planner 其余慢测和稳定 plan/golden 差异归入此任务。
- 任务 10：RealTiKV 环境失败和最终残余归因；`test_split_file` 稳定报 PD cluster 无响应。parser/parsergen 两个 60 秒仍超时的新残余也在最终回归中处理。

`diagnostic` profile 只用于隔离诊断；默认 profile 的 5 秒 slow 报告和约 10 秒终止门槛未改变。
