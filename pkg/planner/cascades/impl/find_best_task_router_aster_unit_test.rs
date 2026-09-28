// Copyright 2026 AsterSQL.

// `find_best_task_router` 的 Aster 单元测试。
//
// 用叶子/一元假逻辑与物理计划验证：路由身份检查、默认路由构造任务代价，
// 以及二次安装 handler 被拒绝。

use crate::impl_and_cost::{BasicTask, PhysicalPlan, PhysicalProperty, Result, Task, TaskType};
use crate::{
    ExhaustPhysicalPlans4GroupExpression, FindBestTask4GroupExpression, InspectLogicalPlanRoute,
    InstallCascadesFindBestTaskRouter, InstallDefaultCascadesFindBestTaskRouter,
    InstallFindBestTaskRouterError, LogicalPlan, LogicalPlanRoute, RouteGroup, RoutePhysicalPlan,
    RoutedGroupExpression,
};
use std::any::{Any, TypeId};
use std::sync::{Arc, Mutex};

/// 测试用叶子物理计划，固定返回 `cost_value`。
#[derive(Clone, Debug)]
struct LeafPlan {
    cost_value: f64,
}

impl PhysicalPlan for LeafPlan {
    fn resolve_indices(&mut self) -> Result<()> {
        Ok(())
    }

    fn cost(&self, _: TaskType) -> Result<f64> {
        Ok(self.cost_value)
    }
}

/// 无子节点的逻辑计划，穷举出一个 `LeafPhysical`。
#[derive(Clone, Debug)]
struct LeafLogical {
    name: &'static str,
    cost_value: f64,
}

impl LogicalPlan for LeafLogical {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn plan_type(&self) -> &str {
        self.name
    }

    fn exhaust_physical_plans(
        &self,
        _: &PhysicalProperty,
    ) -> Result<Vec<Box<dyn RoutePhysicalPlan>>> {
        Ok(vec![Box::new(LeafPhysical {
            cost_value: self.cost_value,
        })])
    }
}

/// 叶子物理实现：无子需求属性，挂接为空孩子 Task。
#[derive(Clone, Debug)]
struct LeafPhysical {
    cost_value: f64,
}

impl PhysicalPlan for LeafPhysical {
    fn resolve_indices(&mut self) -> Result<()> {
        Ok(())
    }

    fn cost(&self, _: TaskType) -> Result<f64> {
        Ok(self.cost_value)
    }
}

impl RoutePhysicalPlan for LeafPhysical {
    fn child_required_properties(&self) -> &[PhysicalProperty] {
        &[]
    }

    fn attach_to_task(self: Box<Self>, children: Vec<Box<dyn Task>>) -> Result<Box<dyn Task>> {
        assert!(children.is_empty());
        let cost_value = self.cost_value;
        Ok(Box::new(BasicTask::new(
            Box::new(LeafPlan { cost_value }),
            cost_value,
            move |_| Box::new(LeafPlan { cost_value }),
        )))
    }
}

/// 一元逻辑计划：要求一个子物理属性，本地代价为 `cost_value`。
#[derive(Clone, Debug)]
struct UnaryLogical {
    name: &'static str,
    cost_value: f64,
    child_property: PhysicalProperty,
}

impl LogicalPlan for UnaryLogical {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn plan_type(&self) -> &str {
        self.name
    }

    fn exhaust_physical_plans(
        &self,
        _: &PhysicalProperty,
    ) -> Result<Vec<Box<dyn RoutePhysicalPlan>>> {
        Ok(vec![Box::new(UnaryPhysical {
            cost_value: self.cost_value,
            child_property: self.child_property.clone(),
        })])
    }
}

/// 一元物理实现：向子 Group 下传 `child_property`。
#[derive(Clone, Debug)]
struct UnaryPhysical {
    cost_value: f64,
    child_property: PhysicalProperty,
}

impl PhysicalPlan for UnaryPhysical {
    fn resolve_indices(&mut self) -> Result<()> {
        Ok(())
    }

    fn cost(&self, _: TaskType) -> Result<f64> {
        Ok(self.cost_value)
    }
}

impl RoutePhysicalPlan for UnaryPhysical {
    fn child_required_properties(&self) -> &[PhysicalProperty] {
        std::slice::from_ref(&self.child_property)
    }

    fn attach_to_task(self: Box<Self>, children: Vec<Box<dyn Task>>) -> Result<Box<dyn Task>> {
        assert_eq!(children.len(), 1);
        let cost_value = self.cost_value;
        Ok(Box::new(BasicTask::new(
            Box::new(LeafPlan { cost_value }),
            cost_value,
            move |_| Box::new(LeafPlan { cost_value }),
        )))
    }
}

/// 构造 Root 任务类型、期望行数为 1 的物理属性。
fn root_property() -> PhysicalProperty {
    PhysicalProperty {
        task_type: TaskType::Root,
        expected_count: 1.0,
    }
}

/// 验证 `InspectLogicalPlanRoute` 报告具体逻辑类型身份。
#[test]
fn inspect_route_reports_concrete_logical_identity() {
    let logical = LeafLogical {
        name: "leaf",
        cost_value: 1.0,
    };
    let route = InspectLogicalPlanRoute(&logical);
    assert!(!route.is_group_expression);
    assert_eq!(route.wrapped_type_id, TypeId::of::<LeafLogical>());
    assert_eq!(route.wrapped_plan_type, "leaf");
}

/// 默认路由可为叶子与一元算子构造任务，并核对代价与穷举候选数。
#[test]
fn default_router_builds_leaf_and_unary_tasks() {
    let _ = InstallDefaultCascadesFindBestTaskRouter();

    let leaf = RoutedGroupExpression::new(
        Box::new(LeafLogical {
            name: "table_scan",
            cost_value: 2.0,
        }),
        Vec::new(),
    );
    let property = root_property();
    let route = LogicalPlanRoute {
        is_group_expression: true,
        wrapped_type_id: TypeId::of::<LeafLogical>(),
        wrapped_plan_type: "table_scan".to_owned(),
    };
    let task = FindBestTask4GroupExpression(&leaf, &property, &route).expect("leaf task");
    let plan = task.plan().expect("leaf task has a plan");
    assert_eq!(plan.cost(TaskType::Root).unwrap(), 2.0);

    // 一元 Limit 风格算子：子 Group 含叶子，验证穷举与找最优任务。
    let child_group = Arc::new(Mutex::new(RouteGroup::new(vec![Arc::new(
        RoutedGroupExpression::new(
            Box::new(LeafLogical {
                name: "child",
                cost_value: 1.0,
            }),
            Vec::new(),
        ),
    )])));
    let unary = RoutedGroupExpression::new(
        Box::new(UnaryLogical {
            name: "limit",
            cost_value: 0.5,
            child_property: property.clone(),
        }),
        vec![child_group],
    );
    let candidates = ExhaustPhysicalPlans4GroupExpression(&unary, &property)
        .expect("unary operator is supported");
    assert_eq!(candidates.len(), 1);
    let unary_task = FindBestTask4GroupExpression(&unary, &property, &route).expect("unary task");
    let unary_plan = unary_task.plan().expect("unary task has a plan");
    assert_eq!(unary_plan.cost(TaskType::Root).unwrap(), 0.5);
}

/// 同一进程内第二次安装 handler 必须失败。
#[test]
fn install_rejects_a_second_handler() {
    let first = InstallCascadesFindBestTaskRouter(|expression, property, route| {
        FindBestTask4GroupExpression(expression, property, route)
    });
    // The default-router test may already have installed a handler in this process.
    // 默认路由测试可能已在本进程安装过 handler。
    match first {
        Ok(()) | Err(InstallFindBestTaskRouterError::HandlerAlreadyInstalled) => {}
    }
    let second = InstallCascadesFindBestTaskRouter(|expression, property, route| {
        FindBestTask4GroupExpression(expression, property, route)
    });
    assert_eq!(
        second,
        Err(InstallFindBestTaskRouterError::HandlerAlreadyInstalled)
    );
}
