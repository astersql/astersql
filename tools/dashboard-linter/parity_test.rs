// Copyright 2026 AsterSQL.

//! Parity tests for `tools/dashboard-linter` vs Go `main.go`.
//! 这个文件不是在验证 Rust 特有实现细节，而是在锁定 `main.go` 对外暴露的可观察契约。
//! 这里的“契约”包括返回码、panic 面、字段校验顺序、重复 ID 统计结果，以及 `$tidb_cluster` 模式的拒绝策略。
//! 测试入口按主题拆成四组，目的是把正常路径、边界条件、错误路径和资源清理路径分别钉住。
//! 这样当 `main.rs` 为了可读性重构时，只要这些测试仍然成立，就说明 Go/Rust 行为仍保持对齐。
//! 由于目标是 parity，本文件只直接调用公开辅助函数，不引入额外测试专用钩子去改变生产代码形状。
//! 临时文件相关断言也有意保持朴素做法，重点是验证 CLI 流程不会因为测试包装而改变真实 I/O 行为。
//! 对注释本身而言，也优先解释“为什么这里要这么测”，而不是把每一行断言机械翻译成中文。
//! 这样后续维护者在更新 Go 语义时，可以先读测试意图，再判断哪些断言需要同步调整。

use std::collections::HashMap;
use std::fs;

use crate::main::{
    BasicDashboard, GridPos, Panel, ROW_TYPE, check_panel, collect_id_stats,
    collect_panel_field_errors, index_of_any, lint_dashboard, run, try_parse_dashboard_json,
};

#[test]
/// 总入口一次性覆盖 `dashboard-linter` 的主要用户可见行为。
/// 这里不用参数化测试或表驱动重写，是为了保持与 Go 版主流程分段相同的阅读顺序。
/// 若其中任一子场景失败，调用栈会直接指向对应分组，方便判断是字段规则、ID 统计还是 CLI 行为漂移。
fn go_rust_public_contract_matches() {
    contract_normal_paths();
    contract_boundary();
    contract_error_paths();
    contract_resource_cleanup();
}

#[test]
fn go_json_unmarshal_contract_matches() {
    // encoding/json rejects a fractional number for a Go int instead of truncating it.
    assert!(try_parse_dashboard_json(br#"{"panels":[{"id":1.5}]}"#).is_err());
    // A type mismatch in a known field makes Unmarshal fail; it is not a zero value.
    assert!(try_parse_dashboard_json(br#"{"panels":"not-an-array"}"#).is_err());
    assert!(try_parse_dashboard_json(br#"{"panels":[42]}"#).is_err());
    // Go struct field matching is case-insensitive and later duplicate keys win.
    let board = try_parse_dashboard_json(
        br#"{"PANELS":[{"ID":1,"id":2,"TITLE":"Good Title","DATASOURCE":"${DS_TEST-CLUSTER}","GRIDPOS":{"H":7}}]}"#,
    )
    .unwrap();
    assert_eq!(board.panels.len(), 1);
    assert_eq!(board.panels[0].id, 2);
    assert_eq!(board.panels[0].title, "Good Title");
    assert_eq!(board.panels[0].grid_pos.h, 7);

    // encoding/json uses Unicode simple-folding for case-insensitive field matching.
    // U+017F LONG S folds with ASCII `s`, so this key still targets `panels` in Go.
    let board =
        try_parse_dashboard_json("{\"panelſ\":[{\"id\":7,\"title\":\"Folded Name\"}]}".as_bytes())
            .unwrap();
    assert_eq!(board.panels.len(), 1);
    assert_eq!(board.panels[0].id, 7);

    // encoding/json accepts surrogate pairs, but rejects non-JSON whitespace and
    // unescaped control characters in strings.
    let board =
        try_parse_dashboard_json(br#"{"panels":[{"title":"\uD83D\uDE00 Metrics"}]}"#).unwrap();
    assert_eq!(board.panels[0].title, "😀 Metrics");
    assert!(try_parse_dashboard_json("{\u{00a0}\"panels\":[]}".as_bytes()).is_err());
    assert!(try_parse_dashboard_json(b"{\"panels\":[{\"title\":\"bad\nvalue\"}]}").is_err());
}

#[test]
fn go_unicode_title_categories_match() {
    let valid_digit = Panel {
        id: 1,
        panel_type: "graph".into(),
        title: "𐒠 Metrics".into(), // U+104A0 OSMANYA DIGIT ZERO: Unicode Nd.
        datasource: "${DS_TEST-CLUSTER}".into(),
        grid_pos: GridPos { h: 7 },
        ..Panel::default()
    };
    assert!(check_panel(&valid_digit).is_empty());

    let not_go_upper = Panel {
        title: "Ⅰ Metrics".into(), // U+2160 is Nl, not Go's Lu upper category.
        ..valid_digit
    };
    assert!(
        check_panel(&not_go_upper)
            .iter()
            .any(|error| error.contains("upper case"))
    );
}

/// Normal: valid dashboard JSON passes field checks, unique IDs, no tidb_cluster glob.
/// 该场景覆盖“合法输入应该一路成功”的基线。
/// 它同时验证反序列化后的结构形状、字段 lint 结果，以及通过 `run()` 走真实文件入口时的返回码。
/// 这里故意构造一个 row panel 加一个普通 panel，确保顶层与一层嵌套子 panel 都能按 Go 逻辑解析。
fn contract_normal_paths() {
    let json = br#"{
      "panels": [
        {
          "id": 1,
          "type": "row",
          "title": "Overview",
          "collapsed": true,
          "panels": [
            {
              "id": 2,
              "type": "graph",
              "title": "QPS Total",
              "datasource": "${DS_TEST-CLUSTER}",
              "gridPos": { "h": 7 },
              "panels": []
            }
          ]
        },
        {
          "id": 3,
          "type": "stat",
          "title": "CPU Usage",
          "datasource": "${DS_TEST-CLUSTER}",
          "gridPos": { "h": 7 }
        }
      ]
    }"#;

    let board = try_parse_dashboard_json(json).expect("valid json");
    // 顶层仍然只保留两个 panel；row 的子 panel 不会被错误提升到顶层。
    assert_eq!(board.panels.len(), 2);
    // 这些断言锁定 JSON tag 到 Rust 字段名的映射，防止反序列化时字段漂移。
    assert_eq!(board.panels[0].panel_type, ROW_TYPE);
    assert!(board.panels[0].collapsed);
    assert_eq!(board.panels[0].panels[0].id, 2);
    assert_eq!(board.panels[1].grid_pos.h, 7);

    // 合法 dashboard 不应触发任何字段错误，也不应让共享 lint 入口返回失败码。
    assert!(collect_panel_field_errors(&board).is_empty());
    assert_eq!(lint_dashboard("ok.json", json), 0);

    // `run()` 额外覆盖真实文件读取路径，确认 CLI 外壳与直接调用 `lint_dashboard()` 一致。
    let tmp = tmpdir("dash-normal");
    let path = tmp.join("dash.json");
    fs::write(&path, json).unwrap();
    let code = run(&[
        "dashboard-linter".to_string(),
        path.to_string_lossy().into_owned(),
    ]);
    assert_eq!(code, 0);
    let _ = fs::remove_dir_all(&tmp);
}

/// Boundary: empty panels OK; single ID available range; short title words ignored; ID gaps.
/// 这个分组专门钉住“边界但合法”的输入，避免后续实现把宽松语义收紧。
/// 重点包括空 dashboard、单个已用 ID 的后继区间、标题分词中的短片段，以及多段 ID 缺口计算。
fn contract_boundary() {
    let empty = br#"{"panels":[]}"#;
    let board = try_parse_dashboard_json(empty).unwrap();
    // 空面板列表在 Go 中合法，Rust 也必须保持“直接通过”而不是报缺少内容。
    assert!(board.panels.is_empty());
    assert_eq!(lint_dashboard("empty.json", empty), 0);

    let mut all = HashMap::new();
    all.insert(5, 1);
    let (dups, ranges) = collect_id_stats(&all);
    // 只有一个已用 ID 时，不存在重复项，空闲区间直接从下一个编号延伸到无穷。
    assert!(dups.is_empty());
    assert_eq!(ranges, vec!["[6, ∞)".to_string()]);

    all.insert(8, 1);
    all.insert(9, 1);
    let (_, ranges) = collect_id_stats(&all);
    // used 5,8,9 → gap [6,8) and single unused between 8 and 9 none; then [10, ∞)
    // 这里验证的是 Go 原始循环的区间拼接格式，而不是更“数学化”的统一表示法。
    assert!(ranges.iter().any(|r| r == "[6, 8)"));
    assert!(ranges.iter().any(|r| r == "[10, ∞)"));

    // word length <= 1 skipped (punctuation '-')
    // 标题里的连字符分隔片段不参与首字母大写检查，这是历史 dashboard 命名习惯的一部分。
    let p = Panel {
        id: 1,
        panel_type: "graph".into(),
        title: "A - B".into(),
        datasource: "${DS_TEST-CLUSTER}".into(),
        grid_pos: GridPos { h: 7 },
        ..Panel::default()
    };
    assert!(check_panel(&p).is_empty());

    // index_of_any: first matching substr wins; byte offset
    // 这里特意分别构造两个模式先后命中的输入，锁定“按模式顺序返回首个命中”的语义。
    // 返回的是字节偏移而非字符下标，和 Go `bytes.Index` 的契约一致。
    let content = b"aaa.*$tidb_clusterbbb";
    assert_eq!(
        index_of_any(content, &[".*$tidb_cluster", "$tidb_cluster.*"]),
        Some(3)
    );
    let content2 = b"xx$tidb_cluster.*yy";
    assert_eq!(
        index_of_any(content2, &[".*$tidb_cluster", "$tidb_cluster.*"]),
        Some(2)
    );
    assert_eq!(
        index_of_any(b"clean", &[".*$tidb_cluster", "$tidb_cluster.*"]),
        None
    );
}

/// Error: panel fields, duplicate IDs, tidb_cluster pattern, usage, bad JSON.
/// 该分组覆盖所有预期失败的入口，确保 Rust 不会把 Go 的硬失败悄悄改成软错误。
/// 这里混合断言返回码与 panic，是因为原始 Go 工具对不同失败面采取的处理策略本来就不一致。
fn contract_error_paths() {
    // Usage
    // 缺少参数时是用户输入错误，因此返回码为 1，而不是 panic。
    assert_eq!(run(&["dashboard-linter".to_string()]), 1);

    // Uncollapsed row
    // row panel 的核心约束是必须折叠；这个错误优先于其子节点是否存在。
    let row = Panel {
        id: 10,
        panel_type: ROW_TYPE.into(),
        collapsed: false,
        ..Panel::default()
    };
    assert_eq!(
        check_panel(&row),
        vec!["row panel 10 should be collapsed".to_string()]
    );

    // Nested child errors under row
    // row 自身通过后，仍要递归校验子 panel 的 datasource、高度和标题风格。
    let nested = Panel {
        id: 1,
        panel_type: ROW_TYPE.into(),
        collapsed: true,
        panels: vec![Panel {
            id: 2,
            panel_type: "graph".into(),
            title: "bad title".into(),
            datasource: "prometheus".into(),
            grid_pos: GridPos { h: 5 },
            ..Panel::default()
        }],
        ..Panel::default()
    };
    let errs = check_panel(&nested);
    // 不要求逐字比较完整输出，但必须确认三个独立规则都被触发。
    assert!(errs.iter().any(|e| e.contains("datasource")));
    assert!(errs.iter().any(|e| e.contains("height")));
    assert!(errs.iter().any(|e| e.contains("upper case")));

    // Empty title
    // 空标题是单独规则，不能被标题大小写规则掩盖或替代。
    let empty_title = Panel {
        id: 3,
        panel_type: "graph".into(),
        title: String::new(),
        datasource: "${DS_TEST-CLUSTER}".into(),
        grid_pos: GridPos { h: 7 },
        ..Panel::default()
    };
    assert!(
        check_panel(&empty_title)
            .iter()
            .any(|e| e == "panel 3 has empty title")
    );

    // Duplicate IDs → exit 1 + available range
    // 这个 JSON 同时驱动共享 lint 入口和纯统计辅助函数，确保二者围绕同一份数据得出一致结论。
    let dup_json = br#"{
      "panels": [
        {
          "id": 1,
          "type": "graph",
          "title": "One",
          "datasource": "${DS_TEST-CLUSTER}",
          "gridPos": { "h": 7 }
        },
        {
          "id": 1,
          "type": "graph",
          "title": "Two",
          "datasource": "${DS_TEST-CLUSTER}",
          "gridPos": { "h": 7 }
        }
      ]
    }"#;
    assert_eq!(lint_dashboard("dup.json", dup_json), 1);

    let board = try_parse_dashboard_json(dup_json).unwrap();
    let mut all = HashMap::new();
    for p in &board.panels {
        *all.entry(p.id).or_insert(0) += 1;
    }
    let (dups, ranges) = collect_id_stats(&all);
    // 重复计数保持 Go 的 `id -> count` 形态，空闲区间从下一个可用 ID 开始。
    assert_eq!(dups.get(&1), Some(&2));
    assert_eq!(ranges, vec!["[2, ∞)".to_string()]);

    // tidb_cluster pattern
    // 这里禁止的是模板变量上的多余模式匹配，而不是所有出现 `$tidb_cluster` 的字符串。
    let bad_cluster = br#"{
      "panels": [{
        "id": 1,
        "type": "graph",
        "title": "Ok Title",
        "datasource": "${DS_TEST-CLUSTER}",
        "gridPos": { "h": 7 },
        "targets": [{ "expr": "up{tidb_cluster=~\".*$tidb_cluster\"}" }]
      }]
    }"#;
    assert_eq!(lint_dashboard("cluster.json", bad_cluster), 1);

    // Invalid JSON panics in parse_dashboard_json path used by lint — try_parse returns Err
    // 可恢复解析入口用于测试拿到错误对象；生产入口仍然保持 Go 式 panic。
    assert!(try_parse_dashboard_json(b"{not-json").is_err());

    // Missing file: run panics (Go panic) — catch via catch_unwind
    // 这里必须捕获 panic，而不是改成 `Result` 断言，否则就把 Go 的失败面改写掉了。
    let r = std::panic::catch_unwind(|| {
        run(&[
            "dashboard-linter".to_string(),
            "/no/such/dashboard-linter-file-xyz.json".to_string(),
        ])
    });
    assert!(r.is_err());
}

/// Resource cleanup: temp dashboard files removed after lint.
/// 该分组验证测试辅助流程不会留下脏临时文件，同时确认默认 dashboard 不携带隐藏错误。
/// 它不是在测试 linter 自动删除文件，而是在确保我们为 parity 测试搭的外层 I/O 包装足够干净可重复。
fn contract_resource_cleanup() {
    let tmp = tmpdir("dash-cleanup");
    let path = tmp.join("tmp-dash.json");
    let json = br#"{"panels":[]}"#;
    fs::write(&path, json).unwrap();
    assert_eq!(
        run(&[
            "dashboard-linter".to_string(),
            path.to_string_lossy().into_owned(),
        ]),
        0
    );
    // 文件删除由测试自己负责，目的是证明前面的成功路径没有持有额外句柄或阻碍清理。
    fs::remove_file(&path).unwrap();
    assert!(!path.exists());
    let _ = fs::remove_dir_all(&tmp);

    // Default board has no panels — no leak of nested structures
    // 默认值路径也要保持“空 dashboard 即无错误”，避免未来为默认构造引入额外占位 panel。
    let board = BasicDashboard::default();
    assert!(collect_panel_field_errors(&board).is_empty());
}

/// 测试临时目录按进程号命名，既减少并发冲突，也方便失败后人工定位残留路径。
/// 先尝试删除旧目录是为了让重复运行在同一进程内也保持幂等，不受前一次失败残留影响。
fn tmpdir(prefix: &str) -> std::path::PathBuf {
    let base = std::env::temp_dir().join(format!(
        "astersql-dashboard-linter-{}-{}",
        prefix,
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&base);
    fs::create_dir_all(&base).unwrap();
    base
}
