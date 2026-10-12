# 任务 2: 分析 Domain、Session 与 TTL 残留

批次：【批次 2】 依赖批次 1

状态：未开始

目的：逐块分析 `pkg/domain`、`pkg/session`、`pkg/sessionctx`、`pkg/sessiontxn` 与 `pkg/ttl` 下的候选残留。

来源任务：无

预计会话范围：路径边界明确，重点覆盖 Domain 回调草稿、Session 测试档案和 TTL SQL builder；只写一个分片报告。

## 文件

- 新建：`.plans/2026-10-12-rust-da-kuai-zhu-shi-can-liu-fen-xi/reports/10-domain-session-ttl.md`
- 读取：`.plans/2026-10-12-rust-da-kuai-zhu-shi-can-liu-fen-xi/reports/00-candidates.tsv`
- 参考：`pkg/domain/domain_sysvars.rs`

## 上下文

- 示例文件前半保存约 146 行 Go/Rust 混合草稿，后半已有 `DomainSysVars` 可执行实现；必须逐方法比较，不能因存在实现就直接判定整块可删。
- Session 区存在整段 Go 测试流程与 `_GO_DRAFT_ARCHIVE`，需区分已有真实回归覆盖和仅存场景文本。

## 验证计划

- 行为：清单中属于指定前缀的每个候选恰有一条判定，并有代码/测试证据。
- 验证命令：`awk -F '\t' '$1 ~ /^pkg\/(domain|session|sessionctx|sessiontxn|ttl)\// {print $1 ":" $2 "-" $3}' .plans/2026-10-12-rust-da-kuai-zhu-shi-can-liu-fen-xi/reports/00-candidates.tsv | sort -u > /tmp/expected-domain-session-ttl && rg '^候选：`' .plans/2026-10-12-rust-da-kuai-zhu-shi-can-liu-fen-xi/reports/10-domain-session-ttl.md | sed -E 's/^候选：`([^`]*)`.*/\1/' | sort -u > /tmp/actual-domain-session-ttl && diff -u /tmp/expected-domain-session-ttl /tmp/actual-domain-session-ttl`
- 预期：`diff` 为空。
- 模拟策略：不使用模拟；读取真实源码、对应 Go 文件、调用关系和测试。

## 步骤

1. 从候选 TSV 提取本任务路径，逐块读取完整范围及其前后可执行代码。
2. 用 RustCodeGraph 查询对应符号、调用方和测试；用同路径 Go 源或 Git 历史确认来源意图。
3. 对每块记录统一字段：候选、分类、Go 来源、Rust 对应、缺失行为、测试证据、风险、建议、置信度。
4. 对 `domain_sysvars.rs` 的注册、PD 动态选项、外部时间戳、资源管控、schema cache 与熔断阈值分别判断，不合并成笼统结论。
5. 对测试档案核对场景是否有可运行 Rust 测试；零测试或只断言字符串存在不能视为已迁移。
6. 运行覆盖率核对并自审误报分类。

## 验证

- 运行：上述 `awk`/`rg`/`diff` 命令；再运行 `git diff --check -- .plans/2026-10-12-rust-da-kuai-zhu-shi-can-liu-fen-xi/reports/10-domain-session-ttl.md`
- 预期：退出码为 0，无遗漏、无重复、无空泛建议。
- 所需证据：候选总数与五类分类计数；每个“已实现重复档案”至少一个实现符号和测试；每个“待实现”至少一个缺失的可观察行为。

## 完成

报告完成且覆盖核对通过后标记“已完成”，使用 `$git-commit` 提交本任务报告，不修改候选源码。
