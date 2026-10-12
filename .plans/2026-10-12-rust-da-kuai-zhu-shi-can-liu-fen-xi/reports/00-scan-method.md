# Rust 大块注释残留候选扫描说明

## 扫描快照

- 规则版本：`comment-residue-v1`
- 扫描时间：`2026-10-12T09:05:10+0800`（Asia/Shanghai）
- 工作树提交：`60e6a2fae8869127ce7db5a71176ac00f0dc4d92`
- RustCodeGraph：索引可用，包含 11,395 个文件、6,959 个 Rust 文件，索引时间戳为 `1791735481989`
- 输入范围：`rg --files -g '*.rs'` 返回的全部 6,959 个 Rust 文件
- 输出：`00-candidates.tsv`，按路径、起始行和结束行稳定排序

RustCodeGraph 用于确认仓库索引及 Rust 文件覆盖规模；注释和字符串不是代码图的完整语义节点，因此候选发现直接读取工作树文本。

## 可复现命令

扫描在仓库根目录执行。生成器是任务执行期间的临时 Python 3 helper，SHA-256 为
`8e47be4e736621aebcabcc0370d9db601af2ea1f192bd0a3de0db270783f36b2`；它只读取 `rg --files -g '*.rs'`
列出的文件并覆盖写入 TSV，生成后删除。完整执行和统计命令如下：

```bash
~/.rustcodegraph/bin/rustcodegraph status
git log -1 --format='%H %cI'
date '+%Y-%m-%dT%H:%M:%S%z'
python3 .plans/2026-10-12-rust-da-kuai-zhu-shi-can-liu-fen-xi/reports/.scan_candidates.py
awk -F '\t' 'NR>1 {
  files[$1]=1
  split($1,a,"/"); tops[a[1]]++
  if ($4 ~ /line-comment/) lc++
  if ($4 ~ /block-comment/) bc++
  if ($4 ~ /raw-string/) rs++
  if ($5 ~ /migration-marker/) mm++
} END {
  printf "candidates=%d unique_files=%d line_comment=%d block_comment=%d raw_string=%d migration_marker=%d\n", NR-1,length(files),lc,bc,rs,mm
  for (k in tops) printf "%s=%d\n",k,tops[k]
}' .plans/2026-10-12-rust-da-kuai-zhu-shi-can-liu-fen-xi/reports/00-candidates.tsv
```

临时 helper 的确定性规则如下，足以按同一版本重建实现与结果：

1. 文件枚举固定使用 `rg --files -g '*.rs'`，按路径排序，以 UTF-8 读取，非法字节替换。
2. 连续行注释是相邻且都匹配 `^\s*//` 的最大区间。区间至少 20 行，且至少 5 行命中代码特征时纳入。
3. 块注释是非嵌套的最短 `/* ... */` 区间。区间至少 20 行、至少 5 行命中代码特征且至少包含一个声明特征时纳入。
4. 原始字符串线性识别 `r"..."`、`r#"..."#`、`br#"..."#` 和 `rb#"..."#`，支持 0 至 255 个 `#`。区间至少 20 行、至少 5 行命中代码特征且至少包含一个声明特征时纳入。
5. 代码特征为声明、控制流、调用形态或 `{}`/`;`；声明特征为 Go/Rust 的 `package`、`func`、`type`、`var`、`const`、`import`、`impl`、`fn`、`struct`、`enum`、`trait` 或测试属性。
6. 标记词不受 20 行阈值限制：`_GO_DRAFT_ARCHIVE`、`机械迁移占位`、`非可执行 Rust`、`不参与运行`，以及英文的 Go draft/archive、migration draft/placeholder、draft/placeholder code/implementation。
7. 标记命中若处于已识别载体内，就扩展到该最小载体；否则只记录命中行，载体记作 `marker`。
8. 同路径内重叠区间合并，载体与规则取并集；纯许可证块（只有 Apache/Copyright 文本且无声明或迁移标记）排除。
9. TSV 使用七列：路径、起始行、结束行、载体、命中规则、行数、首个非空规范化片段。片段中的 Tab 被替换为空格。

## 扫描结果

候选清单包含 580 个块、279 个唯一文件。载体计数允许同一合并块同时属于多种载体：

| 入口 | 候选块数 |
| --- | ---: |
| 连续行注释 | 443 |
| 块注释 | 20 |
| Rust 原始字符串 | 120 |
| 迁移标记规则 | 19 |

顶层路径分布：

| 顶层路径 | 候选块数 |
| --- | ---: |
| `pkg/` | 518 |
| `br/` | 36 |
| `lightning/` | 20 |
| `tools/` | 3 |
| `tests/` | 2 |
| `cmd/` | 1 |

来源示例 `pkg/domain/domain_sysvars.rs` 的第 21–166 行已进入清单。四类入口均有命中，因此无需记录“仓库无命中”的例外。

## 误报边界与已知盲区

- 清单故意保守保留 Rustdoc、算法说明、SQL/协议示例、生成说明和中文注释索引等代码形误报，后续分片必须人工分类，不能把候选直接当作可删除内容。
- 扫描是词法启发式，不解析 Rust token tree；宏可生成的注释、`include!` 外部文件、构建时生成文件及非 `.rs` 文件不在范围内。
- 块注释按 Rust 通常写法做非嵌套最短匹配；真正嵌套的块注释可能被切短，应在人工审计时结合邻近文本复核。
- 普通转义字符串不作为大段归档入口，以避免 SQL、JSON 和测试夹具造成大量噪声；明确迁移标记仍会捕获其所在行。
- “至少 5 行代码特征”按行计数，不按 token 计数；代码密集但少于 20 行的内容只有在命中迁移标记时才进入清单。
- 许可证过滤只移除纯许可证块；许可证与长篇代码形说明连续时仍保留，交由人工判断。

本报告只建立候选全集，不判断实现状态、缺失行为或可删除性。
