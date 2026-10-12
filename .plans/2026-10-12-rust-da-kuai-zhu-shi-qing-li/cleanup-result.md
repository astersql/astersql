# Rust 大块注释清理最终验收

日期：2026-10-12（Asia/Shanghai）

## 结论

- 审计基线：`f63b32cec4`，`00-candidates.tsv` 共 580 个唯一候选。
- 从六份分片报告机械提取 P3/R 目标 70 个，唯一目标 70，重复 0。
- 当前工作树逐内容复扫结果：目标消失 70，目标剩余 0，移动到其他 Rust 文件 0。
- 非目标候选逐内容保留 510，意外消失 0。
- 因而本计划的 70/70 清理目标已完成，P0/P1/P2 与合法说明、模板、夹具未因本批清理而丢失。

复扫不依赖旧行号。脚本从 `f63b32cec4` 读取 TSV 所指向的完整内容区间，再对当前 `rg --files -g '*.rs'` 返回的全部 6,959 个 Rust 文件做同路径和全仓库内容匹配。集合结果为：

```text
baseline=580
targets=70
unique_targets=70
same_path=510
moved_elsewhere=0
absent_repo_wide=70
missing_equals_targets=true
unexpected_missing=0
targets_still_present=0
```

## 批次 1 提交

| 模块 | 目标数 | 完成提交 |
| --- | ---: | --- |
| Domain / Session / TTL | 14 | `ac10b739f6` |
| Planner | 12 | `894281dd1a` |
| Executor | 18 | `0f0b821101` |
| DDL / Store | 7 | `2d6b900725` |
| DXF / Errors / Meta | 5 | `114fb49475` |
| Parser | 13 | `33460f2754` |
| Privilege | 1 | `3f872f7545` |
| 合计 | 70 | 7 个提交 |

七个提交均可由 `git show --stat --oneline <commit>` 解析。任务文件已按各任务的完成流程删除，因此其任务内 Cargo 槽位、`CARGO_TARGET_DIR` 和逐条测试输出不在 Git 提交元数据中；最终验收不臆造这些历史数据。本任务只做只读复扫和 Markdown 报告，没有运行 Cargo，槽位为“不适用”。

## 机械目标清单

以下键来自分片报告，并全部存在于原始 `00-candidates.tsv`；复扫状态均为 `absent_repo_wide`。

```text
pkg/domain/domain_sysvars.rs:21-166
pkg/domain/extract.rs:23-591
pkg/domain/plan_replayer.rs:26-601
pkg/domain/plan_replayer_dump.rs:23-138
pkg/domain/ru_stats.rs:23-284
pkg/domain/runaway.rs:21-66
pkg/domain/schema_checker.rs:23-119
pkg/domain/sysvar_cache.rs:24-304
pkg/domain/test_helper.rs:23-142
pkg/domain/topn_slow_query.rs:24-280
pkg/session/test/tidb_test.rs:21-108
pkg/session/test/vars/vars_test.rs:21-385
pkg/sessionctx/sessionstates/session_token_test.rs:21-395
pkg/ttl/sqlbuilder/sql.rs:22-511
pkg/planner/core/joinorder/conflict_detector.rs:22-594
pkg/planner/core/joinorder/util.rs:22-290
pkg/planner/core/operator/physicalop/physical_cte.rs:19-105
pkg/planner/core/operator/physicalop/physical_cte_table.rs:19-79
pkg/planner/core/operator/physicalop/physical_index_merge_join.rs:21-110
pkg/planner/core/operator/physicalop/physical_lock.rs:21-147
pkg/planner/core/operator/physicalop/physical_sequence.rs:21-181
pkg/planner/core/operator/physicalop/physical_shuffle.rs:21-95
pkg/planner/core/rule/rule_build_key_info.rs:16-38
pkg/planner/core/rule/rule_column_pruning.rs:21-59
pkg/planner/core/rule/rule_join_key_type_cast.rs:16-52
pkg/planner/indexadvisor/model.rs:34-85
pkg/executor/aggregate/agg_hash_base_worker.rs:19-66
pkg/executor/aggregate/agg_hash_partial_worker.rs:18-73
pkg/executor/aggregate/agg_stream_executor.rs:18-106
pkg/executor/join/base_semi_join.rs:22-257
pkg/executor/join/concurrent_map.rs:22-107
pkg/executor/join/hash_join_spill.rs:22-108
pkg/executor/join/hash_join_stats.rs:21-422
pkg/executor/join/hash_table_v1.rs:18-725
pkg/executor/join/hash_table_v2.rs:20-296
pkg/executor/join/index_lookup_join.rs:22-674
pkg/executor/join/index_lookup_merge_join.rs:22-564
pkg/executor/join/inner_join_probe.rs:22-120
pkg/executor/join/join_row_table.rs:22-227
pkg/executor/join/join_table_meta.rs:21-364
pkg/executor/join/left_outer_semi_join_probe.rs:19-242
pkg/executor/join/merge_join.rs:19-387
pkg/executor/join/outer_join_probe.rs:19-419
pkg/executor/join/semi_join_probe.rs:22-332
pkg/ddl/placement/bundle_test.rs:23-1598
pkg/ddl/placement/constraints_test.rs:22-202
pkg/ddl/placement/meta_bundle_test.rs:22-431
pkg/ddl/placement/rule_test.rs:54-77
pkg/ddl/placement/rule_test.rs:79-203
pkg/ddl/serverstate/syncer_test.rs:16-98
pkg/store/copr/region_cache_test.rs:21-249
pkg/dxf/framework/planner/plan_test.rs:23-48
pkg/dxf/framework/planner/planner_test.rs:23-80
pkg/errors/tests/api_parity_test.rs:49-69
pkg/meta/metabuild/context_test.rs:21-176
pkg/meta/model/dependency_tests.rs:10-140
pkg/parser/ast/sem.rs:571-711
pkg/parser/digester.rs:85-178
pkg/parser/digester.rs:183-530
pkg/parser/digester_1_aster_unit_test.rs:89-159
pkg/parser/digester_1_aster_unit_test.rs:202-347
pkg/parser/digester_1_aster_unit_test.rs:399-479
pkg/parser/digester_test.rs:214-379
pkg/parser/format/format.rs:336-366
pkg/parser/lexer.rs:16-42
pkg/parser/lexer.rs:538-809
pkg/parser/lexer_test.rs:312-362
pkg/parser/yy_parser_4_aster_unit_test.rs:37-132
pkg/parser/yy_parser_test.rs:154-247
pkg/privilege/privileges/privileges_test.rs:638-1582
```

## 非目标保留抽查

| 分类 | 基线候选 | 结果 |
| --- | --- | --- |
| P0 完全未实现占位 | `pkg/ddl/placement_policy_test.rs:54-93` | 完整内容仍在同文件 |
| P1 部分实现缺口 | `pkg/planner/core/joinorder/join_order.rs:21-395` | 完整内容仍在同文件 |
| P2 非执行测试档案 | `pkg/planner/core/casetest/plan_test.rs:22-737` | 完整内容仍在同文件 |
| F 合法模板/夹具 | `cmd/pluginpkg/pluginpkg.rs:50-85` | 完整内容仍在同文件 |
| D 合法说明 | `pkg/config/kerneltype/doc.rs:30-57` | 完整内容仍在同文件 |

此外，510 个非目标候选全部通过逐内容同路径匹配，不只依赖上述抽样。

## 验证

验证档位：Ready。原因是本任务交付计划结果报告并验收前序代码清理；仓库声明的 `.agents/skills/tidb-verify-profile` 不存在，因此直接执行计划指定检查。

最终命令与退出码：

```text
~/.rustcodegraph/bin/rustcodegraph status                                  0
python3 <内存复扫脚本：提取 70 个目标并匹配全部 580 个基线内容区间>       0
git diff --check                                                          0
make lint                                                                 0
```

未验证边界：本任务没有重新运行七个模块各自的 Cargo 单测，也没有运行昂贵的全仓 Cargo 测试或 RealTiKV 测试；最终报告只验收档案集合、非目标保留和仓库规定的 lint。
