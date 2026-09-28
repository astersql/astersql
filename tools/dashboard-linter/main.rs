// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! Grafana dashboard JSON linter (Go package `main` / `tools/dashboard-linter`).
//! 这个文件对应 Go 版的单文件入口，实现“读取 dashboard JSON -> 校验字段 -> 检查 panel ID -> 检查 `$tidb_cluster` 模式”的完整流程。
//! 迁移时保留了接近 Go `main.go` 的符号布局，目的是让二进制入口、库调用和对照测试都能直接复用同一套规则。
//! 这里不引入额外抽象层，优先保证与 Go 的输出文案、panic 行为、递归遍历顺序和返回码语义一致。
//! 因此很多辅助函数虽然在 Rust 中还能继续拆分，但仍保持平铺结构，便于做逐段 parity 审查。

use std::collections::HashMap;

// 反序列化细节放在 `stubs` 中，`main.rs` 只保留规则编排与 CLI 流程。
mod stubs;

/// basicDashboard — top-level JSON shape (`panels` only).
/// Grafana dashboard 的顶层对象很大，但此 linter 只依赖 `panels`，因此故意只保留最小可用形状。
/// 这样既能减少迁移面，也能维持与 Go `json.Unmarshal` 一样的“只读取关心字段，其余字段忽略”语义。
#[derive(Clone, Debug, Default)]
pub struct BasicDashboard {
    /// 顶层 panel 列表；行面板内的嵌套 panel 仍通过各自的 `panels` 字段继续下钻。
    pub panels: Vec<Panel>,
}

/// panel — nested Grafana panel; JSON tags preserved in field docs.
/// 这里同时表示普通 panel 与 row panel。
/// row panel 自身主要承担分组职责，真正展示数据的子 panel 会出现在它的 `panels` 内。
/// `id` 用于重复检测与可用区间推导，是后续统计逻辑的主键。
/// `panels` 只承载 row panel 的一层子节点，和 Go 版的数据形状一致。
/// `panel_type` 决定走 row 分支还是普通 panel 分支。
/// `title`、`collapsed`、`datasource`、`grid_pos.h` 分别对应标题风格、row 折叠、模板数据源和统一高度约束。
#[derive(Clone, Debug, Default)]
pub struct Panel {
    pub id: i64,            // json:"id" (Go int on supported 64-bit targets)
    pub panels: Vec<Panel>, // json:"panels"
    pub panel_type: String, // json:"type"
    pub title: String,      // json:"title"
    pub collapsed: bool,    // json:"collapsed"
    pub datasource: String, // json:"datasource"
    pub grid_pos: GridPos,  // json:"gridPos"
}

/// GridPos — Go inline struct with `h`.
/// Go 版本把它写成匿名内联结构；Rust 提取为具名类型，仅为了让反序列化与测试更容易复用。
#[derive(Clone, Debug, Default)]
pub struct GridPos {
    /// panel 高度；仓库约定所有非 row panel 高度统一为 7。
    pub h: i64, // json:"h" (Go int on supported 64-bit targets)
}

/// Go `rowType` 常量；单独提出是为了让 row 分支判断与测试共享同一字面值。
pub const ROW_TYPE: &str = "row";

/// Process entry matching Go `main`. Non-zero codes call `process::exit`.
/// 真正的规则执行在 `run()` 中，`main()` 只负责把返回码转换成进程退出语义。
/// 这样测试可以直接断言返回码，而命令行入口仍与 Go 一样在失败时退出非零状态。
pub fn main() {
    let code = run(&std::env::args().collect::<Vec<_>>());
    if code != 0 {
        std::process::exit(code);
    }
}

/// Core of Go `main`, returning the process exit code (`0` = success).
///
/// Panics on file read / JSON parse failure (Go `panic(err)`).
/// 参数不足时打印用法并返回 `1`，其余 I/O 或 JSON 异常则刻意保留 panic，以匹配 Go 的失败面。
/// `run()` 不直接做具体 lint，而是负责串起“读取文件 -> 解析 -> 进入共享校验函数”的启动顺序。
pub fn run(args: &[String]) -> i32 {
    if args.len() < 2 {
        println!("Usage: dashboard-linter <path-to-dashboard-json>");
        return 1;
    }
    let file_name = &args[1];
    let content = std::fs::read(file_name).unwrap_or_else(|err| panic!("{err}"));
    lint_dashboard(file_name, &content)
}

/// Lint decoded dashboard bytes; shared by CLI and parity tests.
/// 这里是整份规则的汇总入口，也是最适合做 Go/Rust 对照测试的位置。
/// 它按固定顺序执行字段校验、ID 统计和 `$tidb_cluster` 模式检查，避免输出顺序漂移。
pub fn lint_dashboard(file_name: &str, content: &[u8]) -> i32 {
    let board = parse_dashboard_json(content);

    // 字段错误优先返回，保持与 Go 主流程一致，避免后续统计掩盖更直接的配置问题。
    if let Some(code) = check_dashboard_panel_fields(&board) {
        return code;
    }

    let mut all_ids: HashMap<i64, i64> = HashMap::with_capacity(1024);
    for p in &board.panels {
        *all_ids.entry(p.id).or_insert(0) += 1;
        // row panel 只允许一层嵌套，因此这里只统计一层子 panel，和 Go 数据模型保持一致。
        for sub in &p.panels {
            *all_ids.entry(sub.id).or_insert(0) += 1;
        }
    }

    let (duplicate_ids, available_range) = collect_id_stats(&all_ids);

    if !duplicate_ids.is_empty() {
        // 除了报重复项，还同时输出可用 ID 区间，方便维护者在同一轮修复中选取空闲编号。
        println!(
            "Duplicate panel IDs found(id:count map) in file {}: {}\navailable panel ID range: {}",
            file_name,
            format_go_int_map(&duplicate_ids),
            format_go_string_slice(&available_range)
        );
        return 1;
    }

    if let Some(idx) = index_of_any(content, &[".*$tidb_cluster", "$tidb_cluster.*"]) {
        // 这里只做字面量片段检查，不引入正则，原因是 Go 版本原本也是按字节子串扫描。
        let start = idx.saturating_sub(150);
        let end = std::cmp::min(idx + 50, content.len());
        let text = String::from_utf8_lossy(&content[start..end]);
        println!(
            "It is unnecessary to use pattern match for $tidb_cluster.\n\
See https://github.com/pingcap/tidb/pull/54135 for details.\n Around: {text}"
        );
        return 1;
    }
    0
}

/// Go `json.Unmarshal` into `basicDashboard` (panics on error like Go).
/// 对生产路径保留 panic 版本，避免把 Go 原本暴露给调用方的异常面静默改写成返回值。
pub fn parse_dashboard_json(content: &[u8]) -> BasicDashboard {
    stubs::unmarshal_dashboard(content).unwrap_or_else(|err| panic!("{err}"))
}

/// Fallible parse for tests (does not panic).
/// 测试和细粒度断言需要拿到错误对象本身，因此额外提供不 panic 的解析入口。
pub fn try_parse_dashboard_json(content: &[u8]) -> Result<BasicDashboard, String> {
    stubs::unmarshal_dashboard(content)
}

/// indexOfAny — Go `bytes.Index` loop; byte offset of first match.
/// 匹配顺序遵循传入切片顺序，返回第一个命中的字节偏移。
/// 这里故意不把多个模式合并成更复杂的搜索器，因为目标是重现 Go 的简单线性扫描。
pub fn index_of_any(content: &[u8], substrs: &[&str]) -> Option<usize> {
    for sub in substrs {
        let needle = sub.as_bytes();
        if needle.is_empty() {
            // 空模式与 Go `bytes.Index` 一样视为从开头命中。
            return Some(0);
        }
        if let Some(idx) = content
            .windows(needle.len())
            .position(|window| window == needle)
        {
            return Some(idx);
        }
    }
    None
}

/// Duplicate ID map + available ID range strings (Go main loop).
/// 这个辅助函数把 Go `main()` 中夹杂的统计逻辑抽出来，便于测试直接断言重复项和空闲区间。
/// 返回值分两部分：一部分给报错信息，一部分给人工修复时选择新 ID 参考。
pub fn collect_id_stats(all_ids: &HashMap<i64, i64>) -> (HashMap<i64, i64>, Vec<String>) {
    let mut duplicate_ids: HashMap<i64, i64> = HashMap::with_capacity(8);
    let mut used_ids: Vec<i64> = Vec::with_capacity(all_ids.len());
    for (id, count) in all_ids {
        if *count > 1 {
            duplicate_ids.insert(*id, *count);
        }
        used_ids.push(*id);
    }
    used_ids.sort();

    let mut available_range: Vec<String> = Vec::with_capacity(100);
    if !used_ids.is_empty() {
        // 以排序后的已用 ID 线性扫描缺口，输出格式保持接近 Go `fmt.Sprintf` 结果。
        let mut next_id = used_ids[0].wrapping_add(1);
        for id in used_ids.iter().skip(1) {
            let distance = id.wrapping_sub(next_id);
            if distance > 1 {
                available_range.push(format!("[{}, {})", next_id, id));
            } else if distance == 1 {
                available_range.push(format!("{next_id}"));
            }
            next_id = id.wrapping_add(1);
        }
        available_range.push(format!("[{next_id}, ∞)"));
    }
    (duplicate_ids, available_range)
}

/// checkDashboardPanelFields — print errors and return exit code `Some(1)`, or `None` if OK.
/// 这个版本负责贴近 CLI 行为，除了收集错误，还会把每条错误逐行打印出来。
/// 返回 `Option<i32>` 而不是直接退出，是为了让 `lint_dashboard()` 能统一控制整套规则的返回码。
pub fn check_dashboard_panel_fields(board: &BasicDashboard) -> Option<i32> {
    let mut errors: Vec<String> = Vec::with_capacity(8);
    for p in &board.panels {
        let errs = check_panel(p);
        if !errs.is_empty() {
            errors.extend(errs);
        }
    }
    if errors.is_empty() {
        return None;
    }
    for err in errors {
        println!("{err}");
    }
    Some(1)
}

/// Collect panel field errors without printing (for tests).
/// 与上面的 CLI 版本共享同一递归校验逻辑，但去掉打印副作用，便于测试做精确断言。
pub fn collect_panel_field_errors(board: &BasicDashboard) -> Vec<String> {
    let mut errors: Vec<String> = Vec::with_capacity(8);
    for p in &board.panels {
        let errs = check_panel(p);
        if !errs.is_empty() {
            errors.extend(errs);
        }
    }
    errors
}

/// checkPanel — recursive panel field validation.
/// row panel 和普通 panel 走两条规则分支。
/// 前者只检查“是否折叠”并递归子节点，后者检查 datasource、高度、标题为空和标题首字母风格。
pub fn check_panel(p: &Panel) -> Vec<String> {
    let mut errors: Vec<String> = Vec::with_capacity(8);
    if p.panel_type == ROW_TYPE {
        if !p.collapsed {
            errors.push(format!("row panel {} should be collapsed", p.id));
        }
        // row 自身不承载 datasource / 高度规则，但其子 panel 仍需要递归校验。
        for sub in &p.panels {
            let errs = check_panel(sub);
            if !errs.is_empty() {
                errors.extend(errs);
            }
        }
        return errors;
    }

    // tools like TiUP use our grafana dashboard json files as templates.
    // 要求模板变量而不是写死 datasource，才能在不同集群环境里二次渲染。
    if p.datasource != "${DS_TEST-CLUSTER}" {
        errors.push(format!(
            "panel {} has datasource {}, which is not ${{DS_TEST-CLUSTER}}",
            p.id, p.datasource
        ));
    }
    // 高度约束不是 Grafana 语法要求，而是仓库自己的视觉规范。
    if p.grid_pos.h != 7 {
        errors.push(format!(
            "we uses 7 as panel height to uniform UI appearance, panel {} has height {}",
            p.id, p.grid_pos.h
        ));
    }
    if p.title.is_empty() {
        errors.push(format!("panel {} has empty title", p.id));
    }

    // we capitalize every word in title, it doesn't follow english grammar, but
    // we already use it in many places, so we follow the existing style, and
    // check it here.
    // 这里遵循的是仓库历史 dashboard 的命名风格，而不是自然英语语法。
    for word in p.title.split(' ') {
        // ignore some punctuations, like '-'
        // 长度为 0 或 1 的片段常常只是分隔符或单字符缩写，不参与首字母大写检查。
        if word.len() <= 1 {
            continue;
        }
        if let Some(ch) = word.chars().next() {
            // Go: unicode.IsUpper / unicode.IsDigit on first rune.
            // 只检查每个单词的首个字符，这与 Go 循环里 `break` 后的行为完全一致。
            if !go_is_upper(ch) && !go_is_digit(ch) {
                errors.push(format!(
                    "panel {} first char of words in title {} should be all be upper case or digit",
                    p.id, p.title
                ));
            }
        }
    }
    errors
}

/// Go `unicode.IsUpper`: Unicode general category Lu. Rust's `is_uppercase`
/// additionally includes Unicode's Other_Uppercase property, so exclude those ranges.
fn go_is_upper(c: char) -> bool {
    c.is_uppercase()
        && !matches!(
            c,
            '\u{0345}'
                | '\u{2160}'..='\u{216F}'
                | '\u{24B6}'..='\u{24CF}'
                | '\u{1F130}'..='\u{1F149}'
                | '\u{1F150}'..='\u{1F169}'
                | '\u{1F170}'..='\u{1F189}'
                | '\u{1F1E6}'..='\u{1F1FF}'
        )
}

/// Go `unicode.IsDigit`: Unicode general category Nd.
fn go_is_digit(c: char) -> bool {
    matches!(
        c,
        '\u{0030}'..='\u{0039}' | '\u{0660}'..='\u{0669}' | '\u{06F0}'..='\u{06F9}'
            | '\u{07C0}'..='\u{07C9}' | '\u{0966}'..='\u{096F}' | '\u{09E6}'..='\u{09EF}'
            | '\u{0A66}'..='\u{0A6F}' | '\u{0AE6}'..='\u{0AEF}' | '\u{0B66}'..='\u{0B6F}'
            | '\u{0BE6}'..='\u{0BEF}' | '\u{0C66}'..='\u{0C6F}' | '\u{0CE6}'..='\u{0CEF}'
            | '\u{0D66}'..='\u{0D6F}' | '\u{0DE6}'..='\u{0DEF}' | '\u{0E50}'..='\u{0E59}'
            | '\u{0ED0}'..='\u{0ED9}' | '\u{0F20}'..='\u{0F29}' | '\u{1040}'..='\u{1049}'
            | '\u{1090}'..='\u{1099}' | '\u{17E0}'..='\u{17E9}' | '\u{1810}'..='\u{1819}'
            | '\u{1946}'..='\u{194F}' | '\u{19D0}'..='\u{19D9}' | '\u{1A80}'..='\u{1A89}'
            | '\u{1A90}'..='\u{1A99}' | '\u{1B50}'..='\u{1B59}' | '\u{1BB0}'..='\u{1BB9}'
            | '\u{1C40}'..='\u{1C49}' | '\u{1C50}'..='\u{1C59}' | '\u{A620}'..='\u{A629}'
            | '\u{A8D0}'..='\u{A8D9}' | '\u{A900}'..='\u{A909}' | '\u{A9D0}'..='\u{A9D9}'
            | '\u{A9F0}'..='\u{A9F9}' | '\u{AA50}'..='\u{AA59}' | '\u{ABF0}'..='\u{ABF9}'
            | '\u{FF10}'..='\u{FF19}' | '\u{104A0}'..='\u{104A9}' | '\u{10D30}'..='\u{10D39}'
            | '\u{11066}'..='\u{1106F}' | '\u{110F0}'..='\u{110F9}' | '\u{11136}'..='\u{1113F}'
            | '\u{111D0}'..='\u{111D9}' | '\u{112F0}'..='\u{112F9}' | '\u{11450}'..='\u{11459}'
            | '\u{114D0}'..='\u{114D9}' | '\u{11650}'..='\u{11659}' | '\u{116C0}'..='\u{116C9}'
            | '\u{11730}'..='\u{11739}' | '\u{118E0}'..='\u{118E9}' | '\u{11950}'..='\u{11959}'
            | '\u{11C50}'..='\u{11C59}' | '\u{11D50}'..='\u{11D59}' | '\u{11DA0}'..='\u{11DA9}'
            | '\u{11F50}'..='\u{11F59}' | '\u{16A60}'..='\u{16A69}' | '\u{16AC0}'..='\u{16AC9}'
            | '\u{16B50}'..='\u{16B59}' | '\u{1D7CE}'..='\u{1D7FF}' | '\u{1E140}'..='\u{1E149}'
            | '\u{1E2F0}'..='\u{1E2F9}' | '\u{1E4F0}'..='\u{1E4F9}' | '\u{1E950}'..='\u{1E959}'
            | '\u{1FBF0}'..='\u{1FBF9}'
    )
}

fn format_go_int_map(m: &HashMap<i64, i64>) -> String {
    // Go fmt %v for map[int]int: map[k:v k:v] (order unspecified).
    // 这里额外排序不是要改变 Go 语义，而是让 Rust 测试和人工比对得到稳定输出。
    let mut parts: Vec<String> = m.iter().map(|(k, v)| format!("{k}:{v}")).collect();
    parts.sort();
    format!("map[{}]", parts.join(" "))
}

fn format_go_string_slice(s: &[String]) -> String {
    // Go fmt %v for []string: [a b c]
    // 空格分隔而非带逗号，目的是复现 Go `%v` 打印切片时的视觉格式。
    format!("[{}]", s.join(" "))
}
