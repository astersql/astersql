// Copyright 2026 AsterSQL.

use super::property_cols_prune::prepare_possible_properties_for;
use base_dependency::PossiblePropertiesInfo;
use std::cell::RefCell;

struct TestPlan {
    id: i32,
    children: Vec<TestPlan>,
}

#[test]
fn possible_properties_are_prepared_post_order_from_child_results() {
    let plan = TestPlan {
        id: 3,
        children: vec![
            TestPlan {
                id: 1,
                children: vec![],
            },
            TestPlan {
                id: 2,
                children: vec![],
            },
        ],
    };
    let visited = RefCell::new(Vec::new());

    let result = prepare_possible_properties_for(
        &plan,
        |node| node.children.iter().collect(),
        |node, children| {
            visited.borrow_mut().push(node.id);
            assert_eq!(children.len(), node.children.len());
            PossiblePropertiesInfo {
                orders: (node.id != 2).then(|| vec![vec![]]),
                has_tiflash: children.iter().all(|child| child.has_tiflash),
            }
        },
    );

    assert_eq!(*visited.borrow(), vec![1, 2, 3]);
    let orders = result.orders.expect("root keeps its prepared order");
    assert_eq!(orders.len(), 1);
    assert!(orders[0].is_empty());
    assert!(result.has_tiflash);
}
