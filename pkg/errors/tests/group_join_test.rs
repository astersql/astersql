// Copyright 2026 AsterSQL.

// 错误组（ErrorGroup）与 Join / WalkDeep 行为测试。
//
// 覆盖 `Errors` 对单错误与自定义组的展开、`WalkDeep` 对消息链与嵌套 Join
// 的深度优先遍历顺序与短路，以及 `Join` 过滤空项、拼接多错误文本的语义。

use std::error::Error as StdError;
use std::fmt;

use astersql_errors::{ErrorGroup, Errors, Join, New, SharedError, WalkDeep, WithMessage};

/// 测试用错误组：实现 `ErrorGroup::Errors` 返回子错误列表。
#[derive(Debug)]
struct TestGroup(Vec<SharedError>);

impl fmt::Display for TestGroup {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("test group")
    }
}

impl StdError for TestGroup {}

impl ErrorGroup for TestGroup {
    fn Errors(&self) -> Vec<SharedError> {
        self.0.clone()
    }
}

/// 将错误列表转为 Display 字符串列表，便于断言。
fn texts(errors: Vec<SharedError>) -> Vec<String> {
    errors.into_iter().map(|error| error.to_string()).collect()
}

/// 对照 Go：`Errors`/`WalkDeep`/`Join` 的展开、遍历顺序与空输入处理。
#[test]
fn walk_deep_and_join_match_go() {
    let plain = New("plain");
    let singleton = Errors(&plain);
    assert_eq!(singleton.len(), 1);
    assert!(singleton[0].ptr_eq(&plain));

    // 自定义 ErrorGroup：Errors 应返回组内子错误而非组本身。
    let custom_child = New("custom child");
    let custom_group = SharedError::new_group(TestGroup(vec![custom_child.clone()]));
    let custom_children = Errors(&custom_group);
    assert_eq!(custom_children.len(), 1);
    assert!(custom_children[0].ptr_eq(&custom_child));

    assert!(!WalkDeep(None, |_| true));

    // 单因消息链：WalkDeep 从外到内收集 Display。
    let leaf = New("leaf");
    let chain = WithMessage(Some(leaf.clone()), "middle").unwrap();
    let chain = WithMessage(Some(chain), "outer").unwrap();
    let mut chain_order = Vec::new();
    assert!(!WalkDeep(Some(&chain), |error| {
        chain_order.push(error.to_string());
        false
    }));
    assert_eq!(chain_order, ["outer: middle: leaf", "middle: leaf", "leaf"]);

    // 嵌套 Join 组：深度优先，先走消息包装再走组内子错误。
    let child_a = WithMessage(Some(New("a-leaf")), "a").unwrap();
    let nested = Join(&[Some(New("b1")), None, Some(New("b2"))]).unwrap();
    let root_group = Join(&[Some(child_a), Some(nested)]).unwrap();
    let root = WithMessage(Some(root_group), "root").unwrap();

    let mut order = Vec::new();
    assert!(!WalkDeep(Some(&root), |error| {
        order.push(error.to_string());
        false
    }));
    assert_eq!(
        order,
        [
            "root: a: a-leaf\nb1\nb2",
            "a: a-leaf\nb1\nb2",
            "a: a-leaf",
            "a-leaf",
            "b1\nb2",
            "b1",
            "b2",
        ]
    );

    // 回调返回 true 时短路，不再访问后续兄弟（如 b2）。
    let mut short_order = Vec::new();
    assert!(WalkDeep(Some(&root), |error| {
        short_order.push(error.to_string());
        error.to_string() == "b1"
    }));
    assert_eq!(short_order.last().map(String::as_str), Some("b1"));
    assert!(!short_order.iter().any(|error| error == "b2"));

    assert!(Join(&[]).is_none());
    assert!(Join(&[None]).is_none());
    assert!(Join(&[None, None]).is_none());

    let err1 = New("err1");
    let err2 = New("err2");
    let joined = Join(&[Some(err1.clone()), None, Some(err2.clone())]).unwrap();
    assert_eq!(joined.to_string(), "err1\nerr2");
    let children = Errors(&joined);
    assert_eq!(texts(children.clone()), ["err1", "err2"]);
    assert!(children[0].ptr_eq(&err1));
    assert!(children[1].ptr_eq(&err2));
}
