# 任务 1: 建立残留注释候选全集

批次：【批次 1】 无

状态：未开始

目的：用可复现规则扫描全仓 Rust 文件，生成后续分片共同使用的候选清单和扫描说明。

来源任务：用户给出的 `pkg/domain/domain_sysvars.rs` 大块迁移草稿示例。

预计会话范围：只读扫描 Rust 源码并新增两份审计输入文件，不判断所有候选的业务实现状态，适合一个聚焦会话。

## 文件

- 新建：`.plans/2026-10-12-rust-da-kuai-zhu-shi-can-liu-fen-xi/reports/00-scan-method.md`
- 新建：`.plans/2026-10-12-rust-da-kuai-zhu-shi-can-liu-fen-xi/reports/00-candidates.tsv`
- 参考：`pkg/domain/domain_sysvars.rs`

## 上下文

- 初步规则命中 119 个含 `package/func/type/var/const/import/impl/pub fn/pub struct` 等代码声明形行注释的 Rust 文件。
- 另有 `_GO_DRAFT_ARCHIVE`、`机械迁移占位`、`非可执行 Rust`、`不参与运行` 等标记，不能只扫描连续 `//`。
- 许可证、Rustdoc、算法说明、SQL 示例和生成说明属于常见误报；候选清单可包含误报，但必须写明命中规则，供分片任务判定。

## 验证计划

- 行为：扫描可重复，并覆盖行注释、块注释、原始字符串和迁移标记四类入口。
- 验证命令：`test -s .plans/2026-10-12-rust-da-kuai-zhu-shi-can-liu-fen-xi/reports/00-candidates.tsv && awk -F '\t' 'NF < 6 {bad++} END {exit bad != 0}' .plans/2026-10-12-rust-da-kuai-zhu-shi-can-liu-fen-xi/reports/00-candidates.tsv`
- 预期：清单非空，每行至少包含路径、起止行、载体、命中规则、规模和初步片段六列。
- 模拟策略：不使用模拟；直接扫描工作树。

## 步骤

1. 记录 RustCodeGraph 状态、扫描时间、工作树提交和规则版本。
2. 扫描连续代码形 `//` 注释；最低阈值采用 20 行且至少 5 行具有声明、控制流、调用或括号/分号等代码特征。
3. 扫描至少 20 行的 `/* ... */` 与 Rust 原始字符串，识别其中 Go/Rust 声明或测试代码。
4. 用迁移标记词补扫非连续残留，并把低于阈值但明确声明为草稿/占位/不可执行的内容纳入候选。
5. 合并重叠区间、去除纯许可证块，保留其他疑似误报供人工分类；按路径和行号稳定排序。
6. 在扫描说明中记录完整命令、阈值、已知盲区和初步分布，不把初步命中数当成固定验收数。

## 验证

- 运行：`git diff --check -- .plans/2026-10-12-rust-da-kuai-zhu-shi-can-liu-fen-xi/reports && test -s .plans/2026-10-12-rust-da-kuai-zhu-shi-can-liu-fen-xi/reports/00-candidates.tsv && awk -F '\t' 'NF < 6 {print NR; bad++} END {exit bad != 0}' .plans/2026-10-12-rust-da-kuai-zhu-shi-can-liu-fen-xi/reports/00-candidates.tsv`
- 预期：命令退出码均为 0，TSV 无缺列，说明文档包含四类扫描入口和误报边界。
- 所需证据：确切命令及退出码、候选块数、唯一文件数、各顶层路径分布、四类载体各自至少一个命中或“仓库无命中”的明确记录。

## 完成

完成时提交两份扫描产物；任务状态改为“已完成”，使用 `$git-commit` 只提交本任务文件与报告变更，然后按执行提示删除本任务文件。
