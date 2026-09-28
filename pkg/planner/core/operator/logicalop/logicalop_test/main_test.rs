// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

// Cascades 优化器测试夹具加载入口。
// Cascades 是基于规则与代价的查询优化框架；此处校验 JSON 夹具可被正确解析。

/// 加载 Cascades 套件的输入、期望输出与扩展（xut）三类 JSON 夹具。
pub fn GetCascadesSuiteData() -> [serde_json::Value; 3] {
    [
        serde_json::from_str(include_str!("testdata/cascades_suite_in.json"))
            .expect("valid input fixture"),
        serde_json::from_str(include_str!("testdata/cascades_suite_out.json"))
            .expect("valid output fixture"),
        serde_json::from_str(include_str!("testdata/cascades_suite_xut.json"))
            .expect("valid xut fixture"),
    ]
}

/// 冒烟测试：确认三份夹具均为非空对象或数组。
#[test]
fn TestMain() {
    let fixtures = GetCascadesSuiteData();
    // Go testdata.BookKeeper 为三份并行套件；Rust 入口也必须保持三份夹具同构。
    assert!(
        fixtures
            .iter()
            .all(|fixture| fixture.is_array() && !fixture.as_array().unwrap().is_empty())
    );
    let input = fixtures[0].as_array().unwrap();
    let output = fixtures[1].as_array().unwrap();
    let xut = fixtures[2].as_array().unwrap();
    assert_eq!(input.len(), output.len());
    assert_eq!(output.len(), xut.len());
    for ((input_case, output_case), xut_case) in input.iter().zip(output).zip(xut) {
        assert_eq!(input_case["name"].as_str(), output_case["Name"].as_str());
        assert_eq!(output_case["Name"].as_str(), xut_case["Name"].as_str());
        assert_eq!(
            input_case["cases"].as_array().unwrap().len(),
            output_case["Cases"].as_array().unwrap().len()
        );
        assert_eq!(
            output_case["Cases"].as_array().unwrap().len(),
            xut_case["Cases"].as_array().unwrap().len()
        );
    }
}
