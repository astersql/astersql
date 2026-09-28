// Copyright 2023 PingCAP, Inc.
// Licensed under the Apache License, Version 2.0.
// Copyright 2026 AsterSQL.

// 存储过程 AST restore 行为的单元测试。
//
// 覆盖声明/块/DROP、多类型 CREATE PROCEDURE、循环与游标、
// HANDLER/IF/CASE/标签等 SQL 文本还原。

use crate::procedure::*;

/// 构造原始 SQL 占位节点，便于组装过程体。
fn raw(value: &str) -> Box<dyn ProcedureNode> {
    Box::new(RawProcedureNode(value.into()))
}

struct FailingProcedureNode;

impl ProcedureNode for FailingProcedureNode {
    fn restore(&self) -> RestoreResult {
        Err("sentinel".into())
    }
}

/// 抽样覆盖 Raw/Decl/Block/Drop 的 restore 输出。
#[test]
fn test_procedure_visitor_cover() {
    let nodes: Vec<Box<dyn ProcedureNode>> = vec![
        raw("SELECT 1"),
        Box::new(ProcedureDecl {
            decl_names: vec!["a".into()],
            decl_type: "INT(11)".into(),
            decl_default: None,
        }),
        Box::new(ProcedureBlock {
            procedure_vars: vec![],
            procedure_proc_stmts: vec![],
        }),
        Box::new(DropProcedureStmt {
            if_exists: false,
            procedure_name: "proc_2".into(),
        }),
    ];
    assert_eq!(
        nodes
            .iter()
            .map(|node| node.restore().unwrap())
            .collect::<Vec<_>>(),
        [
            "SELECT 1",
            "DECLARE `a` INT(11)",
            "BEGIN  END",
            "DROP PROCEDURE `proc_2`"
        ]
    );
}

/// DECLARE DEFAULT 的子表达式失败时保留 Go Restore 的错误上下文。
#[test]
fn procedure_decl_annotates_default_restore_error_like_go() {
    let declaration = ProcedureDecl {
        decl_names: vec!["a".into()],
        decl_type: "INT(11)".into(),
        decl_default: Some(Box::new(FailingProcedureNode)),
    };

    assert_eq!(
        declaration.restore().unwrap_err(),
        "An error occur while restore expr: sentinel"
    );
}

/// 多种参数类型下 CREATE PROCEDURE … BEGIN … END 文本形态。
#[test]
fn test_procedure() {
    for kind in [
        "INT(11)",
        "BIGINT(20)",
        "VARCHAR(100)",
        "DECIMAL(30,2)",
        "DOUBLE",
        "FLOAT",
        "CHAR(10)",
        "BINARY(1)",
        "VARBINARY(30)",
        "BLOB",
        "TEXT",
        "ENUM('1','2')",
        "SET('1','2')",
    ] {
        let stmt = ProcedureInfo {
            if_not_exists: true,
            procedure_name: "proc_2".into(),
            procedure_param: vec![StoreParameter::new(MODE_IN, "id", kind)],
            procedure_body: Box::new(ProcedureBlock {
                procedure_vars: vec![],
                procedure_proc_stmts: vec![raw("SELECT 1")],
            }),
            procedure_param_str: String::new(),
        };
        let sql = stmt.restore().unwrap();
        assert!(sql.contains(kind));
        assert!(sql.ends_with("BEGIN SELECT 1; END"));
    }
}

/// SHOW CREATE PROCEDURE 命令常量与 DROP IF EXISTS 形态。
#[test]
fn test_show_create_procedure() {
    assert_eq!(
        crate::sem::ShowCreateProcedureCommand,
        "SHOW CREATE PROCEDURE"
    );
    assert_eq!(
        DropProcedureStmt {
            if_exists: false,
            procedure_name: "proc_2".into()
        }
        .restore()
        .unwrap(),
        "DROP PROCEDURE `proc_2`"
    );
    assert_eq!(
        DropProcedureStmt {
            if_exists: true,
            procedure_name: "proc_2".into()
        }
        .restore()
        .unwrap(),
        "DROP PROCEDURE IF EXISTS `proc_2`"
    );
}

/// WHILE/REPEAT/游标 OPEN/FETCH/CLOSE 的 restore 间距。
#[test]
fn test_procedure_visitor() {
    assert_eq!(
        ProcedureWhileStmt {
            condition: raw("`id`<10"),
            body: vec![raw("SET @@SESSION.`id`=`id`+1"), raw("SELECT 1")]
        }
        .restore()
        .unwrap(),
        "WHILE `id`<10 DO SET @@SESSION.`id`=`id`+1;SELECT 1;END WHILE"
    );
    assert_eq!(
        ProcedureRepeatStmt {
            body: vec![raw("SET `id`=`id`+1"), raw("SELECT 1")],
            condition: raw("`id`<10")
        }
        .restore()
        .unwrap(),
        "REPEAT SET `id`=`id`+1;SELECT 1;UNTIL `id`<10 END REPEAT"
    );
    assert_eq!(
        ProcedureCursor {
            cur_name: "TEST1".into(),
            select_string: raw("SELECT 1")
        }
        .restore()
        .unwrap(),
        "DECLARE TEST1 CURSOR FOR SELECT 1"
    );
    assert_eq!(
        ProcedureOpenCur {
            cur_name: "TEST1".into()
        }
        .restore()
        .unwrap(),
        "OPEN TEST1"
    );
    assert_eq!(
        ProcedureFetchInto {
            cur_name: "TEST1".into(),
            variables: vec!["A".into()]
        }
        .restore()
        .unwrap(),
        "FETCH TEST1 INTO A"
    );
    assert_eq!(
        ProcedureCloseCur {
            cur_name: "TEST1".into()
        }
        .restore()
        .unwrap(),
        "CLOSE TEST1"
    );
}

/// HANDLER、IF/ELSE、搜索 CASE 与标签首尾名一致时的还原。
#[test]
fn test_procedure_restore() {
    let handler = ProcedureErrorControl {
        control_handle: PROCEDUR_EXIT,
        error_con: vec![
            Box::new(ProcedureErrorCon {
                error_con: PROCEDUR_SQLWARNING,
            }),
            Box::new(ProcedureErrorCon {
                error_con: PROCEDUR_NOT_FOUND,
            }),
            Box::new(ProcedureErrorCon {
                error_con: PROCEDUR_SQLEXCEPTION,
            }),
        ],
        operate: raw("SELECT 1"),
    };
    assert_eq!(
        handler.restore().unwrap(),
        "DECLARE EXIT HANDLER FOR SQLWARNING, NOT FOUND, SQLEXCEPTION SELECT 1"
    );
    let if_stmt = ProcedureIfInfo {
        if_body: Box::new(ProcedureIfBlock {
            if_expr: raw("`i`>1"),
            procedure_if_stmts: vec![raw("SELECT 2")],
            procedure_else_stmt: Some(Box::new(ProcedureElseBlock {
                procedure_if_stmts: vec![raw("SELECT 5")],
            })),
        }),
    };
    assert_eq!(
        if_stmt.restore().unwrap(),
        "IF `i`>1 THEN SELECT 2;ELSE SELECT 5;END IF"
    );
    let case = SearchCaseStmt {
        when_cases: vec![SearchWhenThenStmt {
            expr: raw("`i`=1"),
            procedure_stmts: vec![raw("SELECT 1")],
        }],
        else_cases: Some(vec![raw("SELECT 3")]),
    };
    assert_eq!(
        case.restore().unwrap(),
        "CASE WHEN `i`=1 THEN SELECT 1; ELSE SELECT 3; END CASE"
    );
    assert_eq!(
        ProcedureLabel::new("labelname", "labelname", "BEGIN SELECT 1; END")
            .restore()
            .unwrap(),
        "`labelname`: BEGIN SELECT 1; END `labelname`"
    );
}

/// 覆盖 Go procedure.go 中其余 restore 节点及可选分支。
#[test]
fn procedure_remaining_restore_nodes_match_go() {
    let simple_case = SimpleCaseStmt {
        condition: raw("NOW()"),
        when_cases: vec![SimpleWhenThenStmt {
            expr: raw("_UTF8MB4'1980-10-01'"),
            procedure_stmts: vec![raw("SELECT 1")],
        }],
        else_cases: Some(vec![raw("SELECT 2")]),
    };
    assert_eq!(
        simple_case.restore().unwrap(),
        "CASE NOW() WHEN _UTF8MB4'1980-10-01' THEN SELECT 1; ELSE SELECT 2; END CASE"
    );

    let else_if = ProcedureElseIfBlock {
        procedure_if_stmt: Box::new(ProcedureIfBlock {
            if_expr: raw("`i`=3"),
            procedure_if_stmts: vec![raw("SELECT 4")],
            procedure_else_stmt: None,
        }),
    };
    assert_eq!(else_if.restore().unwrap(), "ELSEIF `i`=3 THEN SELECT 4;");

    assert_eq!(
        ProcedureErrorVal { error_num: 1211 }.restore().unwrap(),
        "1211"
    );
    assert_eq!(
        ProcedureErrorState {
            code_status: "x'dw".into()
        }
        .restore()
        .unwrap(),
        "SQLSTATE 'x''dw'"
    );
    assert_eq!(
        ProcedureErrorCon {
            error_con: PROCEDUR_END
        }
        .restore()
        .unwrap(),
        ""
    );
    assert_eq!(
        ProcedureErrorControl {
            control_handle: 999,
            error_con: vec![],
            operate: raw("SELECT 1"),
        }
        .restore()
        .unwrap(),
        "DECLARE HANDLER FOR  SELECT 1"
    );
}
