// Copyright 2026 AsterSQL.
// Copyright 2018 PingCAP, Inc.
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

// Cascades 旧版强制规则（Enforcer）单元测试。
//
// 验证 `GetEnforcerRules` 在空/非空排序需求下的返回，以及
// `OrderEnforcer::NewProperty` 清空 SortItems 的可观察行为。

// 本文件对应 pkg/planner/cascades/old/enforcer_rules_test.go。Go 版本用一次类型断言
// `enforcers[0].(*OrderEnforcer)` 来确认 `GetEnforcerRules` 在有排序需求时精确返回一个
// `OrderEnforcer`；Rust 的 `Enforcer` trait 没有声明 `std::any::Any` 父 trait（这属于
// `enforcer_rules.rs` 本身的签名，不在本任务 writes 清单内，不能为了测试改动它），因此这里
// 改用生产代码里唯一一种 enforcer 实现才具有的可观察行为（`NewProperty` 会清空
// `SortItems`）代替类型断言：只有 `OrderEnforcer` 会这样做，因此这个行为断言与 Go 的类型
// 断言在当前规则集合下是等价的。其余逻辑（空排序需求不产生 enforcer、非空排序需求正好产生
// 一个 enforcer）与 Go 完全一致，均通过真实的 `GetEnforcerRules` 驱动。

use astersql_expression as expression;
use astersql_planner_memo as memo;
use astersql_planner_property as property;

// test_get_enforcer_rules 对应 Go 的 TestGetEnforcerRules：验证空物理属性不产生 enforcer，
// 带排序列时产生恰好一个 enforcer。
#[test]
fn test_get_enforcer_rules() {
    let mut prop = property::PhysicalProperty::default();
    let group = memo::NewGroupWithSchema(None, &expression::NewSchema(Vec::new()));
    let enforcers = crate::GetEnforcerRules(&group.borrow(), &prop);
    assert!(enforcers.is_empty(), "Go require.Nil(enforcers)");

    let col = expression::Column::default();
    prop.SortItems.push(property::SortItem {
        Col: col,
        Desc: false,
    });
    let enforcers = crate::GetEnforcerRules(&group.borrow(), &prop);
    assert_eq!(enforcers.len(), 1, "Go require.Len(enforcers, 1)");
}

// test_new_properties 对应 Go 的 TestNewProperties：验证 OrderEnforcer.NewProperty 会清空
// SortItems，避免重复强制同一排序属性；这也是本文件用来在没有 Any 向下转型的情况下确认
// “这一个 enforcer 正是 OrderEnforcer”的可观察行为。
#[test]
fn test_new_properties() {
    let mut prop = property::PhysicalProperty::default();
    let col = expression::Column::default();
    let group = memo::NewGroupWithSchema(None, &expression::NewSchema(Vec::new()));
    prop.SortItems.push(property::SortItem {
        Col: col,
        Desc: false,
    });

    let enforcers = crate::GetEnforcerRules(&group.borrow(), &prop);
    assert_eq!(
        enforcers.len(),
        1,
        "Go require.NotNil + require.Len(enforcers, 1)"
    );

    let order_enforcer = enforcers[0];
    let new_prop = order_enforcer.NewProperty(&prop);
    assert!(
        new_prop.SortItems.is_empty(),
        "Go require.Nil(newProp.SortItems)"
    );
}
