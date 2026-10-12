# 任务 6: 分析 DDL、Table、Store 与基础存储残留

批次：【批次 2】 依赖批次 1

状态：未开始

目的：分析 `pkg/ddl`、`pkg/table`、`pkg/store`、`pkg/kv`、`pkg/structure` 与 `pkg/lock` 下的候选残留。

来源任务：无

预计会话范围：覆盖 schema/表/存储相关路径，需额外遵守 DDL 阅读契约，但只写一个分片报告。

## 文件

- 新建：`.plans/2026-10-12-rust-da-kuai-zhu-shi-can-liu-fen-xi/reports/50-ddl-table-store.md`
- 读取：`.plans/2026-10-12-rust-da-kuai-zhu-shi-can-liu-fen-xi/reports/00-candidates.tsv`
- 参考：`docs/agents/ddl/README.md`

## 上下文

- 在分析任何 DDL 行为前必须阅读 `docs/agents/ddl/README.md`，并把其中描述当入口、以代码和测试验证事实。
- Store/Coprocessor 测试存在明确“外部依赖占位调用”和不可执行 Go 参考文本，是潜在高风险测试覆盖缺口。

## 验证计划

- 行为：指定六个前缀的候选均有唯一分类；DDL 判断附执行框架证据，存储判断附真实行为或测试缺口证据。
- 验证命令：`awk -F '\t' '$1 ~ /^pkg\/(ddl|table|store|kv|structure|lock)\// {print $1 ":" $2 "-" $3}' .plans/2026-10-12-rust-da-kuai-zhu-shi-can-liu-fen-xi/reports/00-candidates.tsv | sort -u > /tmp/expected-ddl-table-store && rg '^候选：`' .plans/2026-10-12-rust-da-kuai-zhu-shi-can-liu-fen-xi/reports/50-ddl-table-store.md | sed -E 's/^候选：`([^`]*)`.*/\1/' | sort -u > /tmp/actual-ddl-table-store && diff -u /tmp/expected-ddl-table-store /tmp/actual-ddl-table-store`
- 预期：`diff` 为空。
- 模拟策略：不使用模拟。

## 步骤

1. 阅读 DDL 入口文档和相关 package `doc.go`，提取分片候选。
2. 对 DDL/placement 候选核对状态转换、回滚、兼容路径与实际测试。
3. 对 table/partition 草稿核对裁剪、交换、重组、约束等场景是否真实执行。
4. 对 store/copr、mockstore、KV、structure、lock 候选核对网络/存储边界是否只是占位，测试是否验证真实返回和错误路径。
5. 发现 DDL 文档与代码漂移时仅在报告中指出，不修改文档。
6. 按统一字段写报告并运行覆盖核对。

## 验证

- 运行：上述覆盖命令；`git diff --check -- .plans/2026-10-12-rust-da-kuai-zhu-shi-can-liu-fen-xi/reports/50-ddl-table-store.md`
- 预期：退出码为 0。
- 所需证据：候选/分类计数、DDL 入口阅读记录、测试/调用证据、文档漂移清单（如有）、覆盖差异为空。

## 完成

验证通过后标记“已完成”，使用 `$git-commit` 只提交本任务报告。
