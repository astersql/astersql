# 任务 3: 分析 Planner 残留

批次：【批次 2】 依赖批次 1

状态：未开始

目的：逐块分析 `pkg/planner/**` 中大块注释和归档代码，识别真实规划器行为缺口与可删除重复档案。

来源任务：无

预计会话范围：Planner 是初扫最大分组（保守规则命中 38 个文件），但路径单一、报告独立；按 rule、joinorder、physicalop、casetest、indexadvisor 子节处理。

## 文件

- 新建：`.plans/2026-10-12-rust-da-kuai-zhu-shi-can-liu-fen-xi/reports/20-planner.md`
- 读取：`.plans/2026-10-12-rust-da-kuai-zhu-shi-can-liu-fen-xi/reports/00-candidates.tsv`

## 上下文

- `pkg/planner/core/rule/rule_partition_processor.rs` 初扫包含约 2200 行注释代码，是高风险候选。
- 需读取 Planner 对应 package 的 `doc.go`（存在时）和相邻 Rust 测试；规则名或类型存在不代表优化行为完整。

## 验证计划

- 行为：所有 `pkg/planner/**` 候选均被分类，且对优化规则/物理计划候选提供可观察计划行为或测试证据。
- 验证命令：`awk -F '\t' '$1 ~ /^pkg\/planner\// {print $1 ":" $2 "-" $3}' .plans/2026-10-12-rust-da-kuai-zhu-shi-can-liu-fen-xi/reports/00-candidates.tsv | sort -u > /tmp/expected-planner && rg '^候选：`' .plans/2026-10-12-rust-da-kuai-zhu-shi-can-liu-fen-xi/reports/20-planner.md | sed -E 's/^候选：`([^`]*)`.*/\1/' | sort -u > /tmp/actual-planner && diff -u /tmp/expected-planner /tmp/actual-planner`
- 预期：`diff` 为空。
- 模拟策略：不使用模拟。

## 步骤

1. 阅读 Planner `doc.go`（若存在）并提取本分片候选。
2. 按优化规则、join order、物理算子、case test、index advisor 分组检查。
3. 用 RustCodeGraph 对关键类型/函数做调用与测试影响查询；与同路径 Go 实现逐段核对重要分支。
4. 记录统一字段，并特别标出只有类型壳、返回固定结果、未接入规则列表或只有注释测试的情况。
5. 对正常的大型算法说明或 SQL 示例标为合法说明误报，并写明排除依据。
6. 完成覆盖核对与风险排序。

## 验证

- 运行：上述覆盖命令；`git diff --check -- .plans/2026-10-12-rust-da-kuai-zhu-shi-can-liu-fen-xi/reports/20-planner.md`
- 预期：退出码为 0。
- 所需证据：候选/分类计数，高风险规则清单，逐项符号与测试证据，覆盖差异为空。

## 完成

验证通过后标记“已完成”，使用 `$git-commit` 只提交本任务报告，不修改 Planner 源码。
