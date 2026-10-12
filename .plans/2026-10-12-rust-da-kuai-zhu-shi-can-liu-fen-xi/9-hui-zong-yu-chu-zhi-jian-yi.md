# 任务 9: 汇总审计与处置建议

批次：【批次 3】 依赖批次 2

状态：未开始

目的：合并所有分片结论，证明候选全集零遗漏，并形成按风险和依赖排序的后续实施清单。

来源任务：无

预计会话范围：只整合既有报告、解决重复/冲突分类并新增最终报告，不重新实现或清理源码。

## 文件

- 新建：`.plans/2026-10-12-rust-da-kuai-zhu-shi-can-liu-fen-xi/reports/final-audit.md`
- 读取：`.plans/2026-10-12-rust-da-kuai-zhu-shi-can-liu-fen-xi/reports/00-candidates.tsv`
- 读取：`.plans/2026-10-12-rust-da-kuai-zhu-shi-can-liu-fen-xi/reports/10-domain-session-ttl.md`
- 读取：`.plans/2026-10-12-rust-da-kuai-zhu-shi-can-liu-fen-xi/reports/20-planner.md`
- 读取：`.plans/2026-10-12-rust-da-kuai-zhu-shi-can-liu-fen-xi/reports/30-executor.md`
- 读取：`.plans/2026-10-12-rust-da-kuai-zhu-shi-can-liu-fen-xi/reports/40-expression-statistics.md`
- 读取：`.plans/2026-10-12-rust-da-kuai-zhu-shi-can-liu-fen-xi/reports/50-ddl-table-store.md`
- 读取：`.plans/2026-10-12-rust-da-kuai-zhu-shi-can-liu-fen-xi/reports/60-br-lightning-dumpling.md`
- 读取：`.plans/2026-10-12-rust-da-kuai-zhu-shi-can-liu-fen-xi/reports/70-other-paths.md`

## 上下文

- 最终报告是用户需要的“都分析出来”的交付物，必须同时说明发现规则、覆盖边界、误报、真实残留、行为缺口和后续建议。
- 建议按 P0（安全/数据正确性）、P1（核心行为缺失）、P2（测试缺口/局部简化）、P3（已实现后的注释清理）排序；优先级必须有证据。

## 验证计划

- 行为：候选 TSV 的每个唯一范围在七份分片报告中恰好出现一次，最终统计与分片汇总一致。
- 验证命令：`awk -F '\t' 'NR > 1 {print $1 ":" $2 "-" $3}' .plans/2026-10-12-rust-da-kuai-zhu-shi-can-liu-fen-xi/reports/00-candidates.tsv | sort -u > /tmp/all-candidates && rg --no-filename '^候选：`' .plans/2026-10-12-rust-da-kuai-zhu-shi-can-liu-fen-xi/reports/{10-domain-session-ttl,20-planner,30-executor,40-expression-statistics,50-ddl-table-store,60-br-lightning-dumpling,70-other-paths}.md | sed -E 's/^候选：`([^`]*)`.*/\1/' | sort > /tmp/all-reviewed && diff -u /tmp/all-candidates <(uniq /tmp/all-reviewed) && test "$(wc -l < /tmp/all-reviewed | tr -d ' ')" = "$(uniq /tmp/all-reviewed | wc -l | tr -d ' ')"`
- 预期：差异为空且无重复。
- 模拟策略：不使用模拟。

## 步骤

1. 运行全集覆盖和重复检查；有遗漏或冲突时回到对应分片报告修正，不在最终报告隐藏问题。
2. 汇总五类分类、载体类型、顶层路径、风险级别和置信度统计。
3. 单列 `domain_sysvars.rs` 的逐回调结论，回应用户给出的具体示例。
4. 对真实残留按可独立实施边界聚类，写出建议的后续计划名称、涉及路径、行为验收和依赖顺序。
5. 对可清理档案列出删除前置证据；对合法说明误报解释保留理由；对未知项写明缺失证据和核查方式。
6. 记录验证限制：本轮未修改/编译/运行代码，结论来自静态源码、调用图、测试源码与历史证据。

## 验证

- 运行：上述全集核对命令；`git diff --check -- .plans/2026-10-12-rust-da-kuai-zhu-shi-can-liu-fen-xi/reports/final-audit.md`
- 预期：退出码为 0，候选零遗漏、零重复，汇总数字可从分片复算。
- 所需证据：全集/已审计数量、重复数 0、各分类和优先级计数、`domain_sysvars.rs` 逐回调结论、后续实施批次建议、明确验证限制。

## 完成

最终报告和全集核对通过后标记“已完成”，使用 `$git-commit` 提交最终审计产物；不修改生产源码或 `plan.md`。
