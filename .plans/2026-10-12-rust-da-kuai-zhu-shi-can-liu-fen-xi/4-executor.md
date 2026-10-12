# 任务 4: 分析 Executor 残留

批次：【批次 2】 依赖批次 1

状态：未开始

目的：逐块分析 `pkg/executor/**` 中执行器实现草稿、测试归档与代码形注释。

来源任务：无

预计会话范围：Executor 初扫命中约 31 个文件，按 join、aggregate、internal、sortexec 与其他执行器分节写单一报告。

## 文件

- 新建：`.plans/2026-10-12-rust-da-kuai-zhu-shi-can-liu-fen-xi/reports/30-executor.md`
- 读取：`.plans/2026-10-12-rust-da-kuai-zhu-shi-can-liu-fen-xi/reports/00-candidates.tsv`

## 上下文

- `hash_join_v1.rs`、`hash_join_v2.rs`、`joiner.rs` 等含千行级注释，需核对真正的数据流、spill、取消、错误传播和 join 语义。
- sortexec 下存在 `_GO_DRAFT_ARCHIVE`；字符串存在性测试不能代替执行器行为测试。

## 验证计划

- 行为：所有 `pkg/executor/**` 候选有唯一判定，join/aggregate/sort 候选包含行为和测试覆盖判断。
- 验证命令：`awk -F '\t' '$1 ~ /^pkg\/executor\// {print $1 ":" $2 "-" $3}' .plans/2026-10-12-rust-da-kuai-zhu-shi-can-liu-fen-xi/reports/00-candidates.tsv | sort -u > /tmp/expected-executor && rg '^候选：`' .plans/2026-10-12-rust-da-kuai-zhu-shi-can-liu-fen-xi/reports/30-executor.md | sed -E 's/^候选：`([^`]*)`.*/\1/' | sort -u > /tmp/actual-executor && diff -u /tmp/expected-executor /tmp/actual-executor`
- 预期：`diff` 为空。
- 模拟策略：不使用模拟。

## 步骤

1. 提取本分片候选并读取对应 Executor `doc.go`（若存在）。
2. 对 join 候选核对 build/probe、outer/semi/null、spill、并发取消与统计行为；对 aggregate 核对 partial/final/stream/spill。
3. 对 raw string 测试档案查找可运行 Rust 测试及其断言，不把归档完整性测试计为行为覆盖。
4. 用 RustCodeGraph 核对可执行类型是否被工厂/dispatch 接线，以及候选符号的调用方与测试。
5. 按统一字段记录分类、证据、风险和建议，并区分完全缺失与局部简化。
6. 完成覆盖核对和高风险排序。

## 验证

- 运行：上述覆盖命令；`git diff --check -- .plans/2026-10-12-rust-da-kuai-zhu-shi-can-liu-fen-xi/reports/30-executor.md`
- 预期：退出码为 0。
- 所需证据：候选与分类计数、接线证据、行为测试证据、覆盖差异为空。

## 完成

验证通过后标记“已完成”，使用 `$git-commit` 只提交本任务报告，不修改 Executor 源码。
