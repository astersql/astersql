# 任务 8: 分析其余路径残留

批次：【批次 2】 依赖批次 1

状态：未开始

目的：分析未被任务 2–7 覆盖的全部候选，确保全仓扫描没有路径盲区。

来源任务：无

预计会话范围：由候选清单做集合差得到剩余路径，预计包含 dumpformat、ingestor、dxf、util、server、metrics、cmd、tests、tools 等零散模块。

## 文件

- 新建：`.plans/2026-10-12-rust-da-kuai-zhu-shi-can-liu-fen-xi/reports/70-other-paths.md`
- 读取：`.plans/2026-10-12-rust-da-kuai-zhu-shi-can-liu-fen-xi/reports/00-candidates.tsv`

## 上下文

- 本任务使用排除集合而非固定白名单，确保新发现路径自动落入本分片。
- 每个目标 package 在分析行为前读取 `doc.go`（存在时）；不同子系统分别成节，不能用统一“迁移占位”结论带过。

## 验证计划

- 行为：候选全集减去任务 2–7 的路径集合后，剩余候选全部且仅在本报告出现。
- 验证命令：`awk -F '\t' '$1 !~ /^pkg\/(domain|session|sessionctx|sessiontxn|ttl|planner|executor|expression|statistics|ddl|table|store|kv|structure|lock)\// && $1 !~ /^(br|lightning|dumpling)\// {print $1 ":" $2 "-" $3}' .plans/2026-10-12-rust-da-kuai-zhu-shi-can-liu-fen-xi/reports/00-candidates.tsv | sort -u > /tmp/expected-other && rg '^候选：`' .plans/2026-10-12-rust-da-kuai-zhu-shi-can-liu-fen-xi/reports/70-other-paths.md | sed -E 's/^候选：`([^`]*)`.*/\1/' | sort -u > /tmp/actual-other && diff -u /tmp/expected-other /tmp/actual-other`
- 预期：`diff` 为空。
- 模拟策略：不使用模拟。

## 步骤

1. 用排除表达式生成本任务候选，按顶层/二级路径分节。
2. 逐块读取上下文、Go 来源、Rust 实现、调用方和测试。
3. 对生成文件、格式说明、协议示例特别审查误报；对 stub、mock、归档测试明确其是否位于生产路径及可观察风险。
4. 按统一字段记录候选、分类、来源、Rust 对应、缺失行为、测试、风险、建议和置信度。
5. 完成覆盖核对并列出未知来源或需 Git 历史确认的项目。

## 验证

- 运行：上述覆盖命令；`git diff --check -- .plans/2026-10-12-rust-da-kuai-zhu-shi-can-liu-fen-xi/reports/70-other-paths.md`
- 预期：退出码为 0。
- 所需证据：剩余候选/分类计数、按路径分布、未知项清单、覆盖差异为空。

## 完成

验证通过后标记“已完成”，使用 `$git-commit` 只提交本任务报告。
