# 任务 7: 分析 BR、Lightning 与 Dumpling 残留

批次：【批次 2】 依赖批次 1

状态：未开始

目的：分析 `br/**`、`lightning/**` 与 `dumpling/**` 中大块代码注释、桩说明和测试档案。

来源任务：无

预计会话范围：三个数据迁移/备份顶层域，路径互斥且共享外部服务与持久化风险语境，输出一份报告。

## 文件

- 新建：`.plans/2026-10-12-rust-da-kuai-zhu-shi-can-liu-fen-xi/reports/60-br-lightning-dumpling.md`
- 读取：`.plans/2026-10-12-rust-da-kuai-zhu-shi-can-liu-fen-xi/reports/00-candidates.tsv`

## 上下文

- 这些路径可能把真实外部依赖简化成桩；分析重点是注释残留是否掩盖备份、恢复、导入、错误恢复或资源清理缺口。
- 合法协议/文件格式说明可能很长，必须与代码归档区分。

## 验证计划

- 行为：三个顶层前缀的所有候选均有唯一分类和风险依据。
- 验证命令：`awk -F '\t' '$1 ~ /^(br|lightning|dumpling)\// {print $1 ":" $2 "-" $3}' .plans/2026-10-12-rust-da-kuai-zhu-shi-can-liu-fen-xi/reports/00-candidates.tsv | sort -u > /tmp/expected-br-lightning-dumpling && rg '^候选：`' .plans/2026-10-12-rust-da-kuai-zhu-shi-can-liu-fen-xi/reports/60-br-lightning-dumpling.md | sed -E 's/^候选：`([^`]*)`.*/\1/' | sort -u > /tmp/actual-br-lightning-dumpling && diff -u /tmp/expected-br-lightning-dumpling /tmp/actual-br-lightning-dumpling`
- 预期：`diff` 为空。
- 模拟策略：不使用模拟。

## 步骤

1. 提取候选并读取相关 package `doc.go`（存在时）。
2. 对每块核对 Go 来源、Rust 对应实现、调用接线和相邻测试。
3. 重点检查错误传播、并发取消、重试、持久化、临时文件/外部资源清理是否仅留在注释中。
4. 将纯格式/协议解释列为合法说明误报，将“仅保留形状”的外部依赖列为部分实现或未实现并描述可观察影响。
5. 按统一字段写报告并完成覆盖核对。

## 验证

- 运行：上述覆盖命令；`git diff --check -- .plans/2026-10-12-rust-da-kuai-zhu-shi-can-liu-fen-xi/reports/60-br-lightning-dumpling.md`
- 预期：退出码为 0。
- 所需证据：候选/分类计数、外部边界风险、行为测试证据、覆盖差异为空。

## 完成

验证通过后标记“已完成”，使用 `$git-commit` 只提交本任务报告。
