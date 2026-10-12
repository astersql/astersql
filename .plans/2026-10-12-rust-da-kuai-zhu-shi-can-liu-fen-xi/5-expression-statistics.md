# 任务 5: 分析 Expression 与 Statistics 残留

批次：【批次 2】 依赖批次 1

状态：未开始

目的：分析 `pkg/expression/**` 与 `pkg/statistics/**` 中代码形注释、超长测试注释和 Go 草稿字符串。

来源任务：无

预计会话范围：两个相关计算子系统共享表达式/统计测试语境，写入独立报告且不触碰源码。

## 文件

- 新建：`.plans/2026-10-12-rust-da-kuai-zhu-shi-can-liu-fen-xi/reports/40-expression-statistics.md`
- 读取：`.plans/2026-10-12-rust-da-kuai-zhu-shi-can-liu-fen-xi/reports/00-candidates.tsv`

## 上下文

- Expression 测试中存在大量连续注释，可能是表驱动用例档案，也可能是合法测试数据说明。
- Statistics priority queue 与 storage 测试存在机械迁移/原始字符串候选，必须核对真实调度和存储行为是否可执行。

## 验证计划

- 行为：两个前缀的所有候选均有唯一、证据化分类。
- 验证命令：`awk -F '\t' '$1 ~ /^pkg\/(expression|statistics)\// {print $1 ":" $2 "-" $3}' .plans/2026-10-12-rust-da-kuai-zhu-shi-can-liu-fen-xi/reports/00-candidates.tsv | sort -u > /tmp/expected-expression-statistics && rg '^候选：`' .plans/2026-10-12-rust-da-kuai-zhu-shi-can-liu-fen-xi/reports/40-expression-statistics.md | sed -E 's/^候选：`([^`]*)`.*/\1/' | sort -u > /tmp/actual-expression-statistics && diff -u /tmp/expected-expression-statistics /tmp/actual-expression-statistics`
- 预期：`diff` 为空。
- 模拟策略：不使用模拟。

## 步骤

1. 读取目标 package 的 `doc.go`（存在时）并提取候选。
2. 检查 builtin/向量化表达式候选的签名接线、边界值、NULL/类型/排序规则语义及对应测试。
3. 检查 statistics 自动分析队列、DDL handler、storage 候选的状态转移与持久化测试。
4. 对长测试注释区分“被注释掉的用例”和“解释现有表驱动数据”，记录原始断言是否已在 Rust 中重建。
5. 按统一字段写报告并运行覆盖核对。

## 验证

- 运行：上述覆盖命令；`git diff --check -- .plans/2026-10-12-rust-da-kuai-zhu-shi-can-liu-fen-xi/reports/40-expression-statistics.md`
- 预期：退出码为 0。
- 所需证据：候选/分类计数、关键边界行为证据、测试是否真实执行的判断、覆盖差异为空。

## 完成

验证通过后标记“已完成”，使用 `$git-commit` 只提交本任务报告。
