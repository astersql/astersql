// Copyright 2026 AsterSQL.

// `pkg` 根 crate 公开表面的接线冒烟测试。
//
// 确认 parser/mysql、独立 parser 配置模块以及 util 子模块经 facade 正确再导出。

/// 验证根导出能访问 parser::mysql 错误码与 NewErr。
#[test]
fn pkg_root_exports_parser_mysql() {
    assert_eq!(crate::parser::mysql::errcode::ErrNoDB, 1046);

    let err = crate::parser::mysql::error::NewErr(1046, vec![]);
    assert_eq!(err.Code, 1046);
}

/// 验证 charset / SQLMode / 类型工具等 mysql 公开表面仍可从根路径使用。
#[test]
fn pkg_root_keeps_parser_mysql_public_surface() {
    assert_eq!(crate::parser::mysql::charset::DefaultCharset, "utf8mb4");
    assert!(crate::parser::mysql::r#const::DefaultSQLMode.contains("STRICT_TRANS_TABLES"));
    assert!(crate::parser::mysql::util::IsIntegerType(3));

    let err = crate::parser::mysql::error::NewErrf(1046, "%s", &[], vec!["missing db".into()]);
    assert_eq!(err.Code, 1046);
}

/// 验证 duration / util 与 kerneltype 名称等独立模块已接线。
#[test]
fn independent_parser_config_modules_are_wired() {
    assert_eq!(
        crate::parser::duration::ParseDuration("0")
            .unwrap()
            .as_nanos(),
        0
    );
    assert_eq!(crate::parser::util::UnescapeChar(b'n'), vec![b'\n']);
    let expected_kernel_name = if crate::config::kerneltype::IsNextGen() {
        "Next Generation"
    } else {
        "Classic"
    };
    assert_eq!(crate::config::kerneltype::Name(), expected_kernel_name);
}

/// 验证 util 下 slice / texttree / disjointset / queue / format / naming 等模块可调用。
#[test]
fn independent_util_modules_are_wired() {
    assert!(crate::util::slice::AllOf(&[2, 4], |value| value % 2 == 0));
    assert_eq!(
        crate::util::texttree::PrettyIdentifier("root", "", false),
        "root"
    );

    let mut set = crate::util::disjointset::NewIntSet(2);
    set.Union(0, 1);
    assert_eq!(set.FindRoot(0), set.FindRoot(1));

    let queue = crate::util::queue::NewQueue::<i32>(1);
    assert!(queue.IsEmpty());

    let _formatter = crate::util::format::IndentFormatter(Vec::new(), "  ");
    assert!(crate::util::naming::Check("util_module").is_ok());
}
