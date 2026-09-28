// Copyright 2026 AsterSQL.

use crate::impl_and_cost::{
    BasicTask, Error, PhysicalPlan, PhysicalProperty, Result, Task, TaskType,
};
use crate::{
    FindBestTask4GroupExpression, LogicalPlan, LogicalPlanRoute, RouteGroup, RoutePhysicalPlan,
    RoutedGroupExpression,
};
use std::any::{Any, TypeId};
use std::sync::{Arc, Mutex};

#[derive(Clone)]
struct LeafLogical;

impl LogicalPlan for LeafLogical {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn plan_type(&self) -> &str {
        "leaf"
    }

    fn exhaust_physical_plans(
        &self,
        _: &PhysicalProperty,
    ) -> Result<Vec<Box<dyn RoutePhysicalPlan>>> {
        Ok(vec![Box::new(LeafPhysical)])
    }
}

#[derive(Clone)]
struct FailingLogical;

impl LogicalPlan for FailingLogical {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn plan_type(&self) -> &str {
        "failing"
    }

    fn exhaust_physical_plans(
        &self,
        _: &PhysicalProperty,
    ) -> Result<Vec<Box<dyn RoutePhysicalPlan>>> {
        Err(Error::new("child enumeration failed"))
    }
}

#[derive(Clone)]
struct UnaryLogical {
    child_property: PhysicalProperty,
}

impl LogicalPlan for UnaryLogical {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn plan_type(&self) -> &str {
        "unary"
    }

    fn exhaust_physical_plans(
        &self,
        _: &PhysicalProperty,
    ) -> Result<Vec<Box<dyn RoutePhysicalPlan>>> {
        Ok(vec![Box::new(UnaryPhysical {
            child_property: self.child_property.clone(),
        })])
    }
}

#[derive(Clone)]
struct LeafPhysical;

impl PhysicalPlan for LeafPhysical {
    fn resolve_indices(&mut self) -> Result<()> {
        Ok(())
    }

    fn cost(&self, _: TaskType) -> Result<f64> {
        Ok(1.0)
    }
}

impl RoutePhysicalPlan for LeafPhysical {
    fn child_required_properties(&self) -> &[PhysicalProperty] {
        &[]
    }

    fn attach_to_task(self: Box<Self>, children: Vec<Box<dyn Task>>) -> Result<Box<dyn Task>> {
        assert!(children.is_empty());
        Ok(Box::new(BasicTask::new(
            Box::new(LeafPhysical),
            1.0,
            |_| Box::new(LeafPhysical),
        )))
    }
}

#[derive(Clone)]
struct UnaryPhysical {
    child_property: PhysicalProperty,
}

impl PhysicalPlan for UnaryPhysical {
    fn resolve_indices(&mut self) -> Result<()> {
        Ok(())
    }

    fn cost(&self, _: TaskType) -> Result<f64> {
        Ok(1.0)
    }
}

impl RoutePhysicalPlan for UnaryPhysical {
    fn child_required_properties(&self) -> &[PhysicalProperty] {
        std::slice::from_ref(&self.child_property)
    }

    fn attach_to_task(self: Box<Self>, children: Vec<Box<dyn Task>>) -> Result<Box<dyn Task>> {
        assert_eq!(children.len(), 1);
        Ok(Box::new(BasicTask::new(
            Box::new(LeafPhysical),
            1.0,
            |_| Box::new(LeafPhysical),
        )))
    }
}

fn root_property() -> PhysicalProperty {
    PhysicalProperty {
        task_type: TaskType::Root,
        expected_count: 1.0,
    }
}

fn route() -> LogicalPlanRoute {
    LogicalPlanRoute {
        is_group_expression: true,
        wrapped_type_id: TypeId::of::<UnaryLogical>(),
        wrapped_plan_type: "unary".to_owned(),
    }
}

fn assert_child_error_is_preserved(child_expressions: Vec<Arc<RoutedGroupExpression>>) {
    let property = root_property();
    let child = Arc::new(Mutex::new(RouteGroup::new(child_expressions)));
    let parent = RoutedGroupExpression::new(
        Box::new(UnaryLogical {
            child_property: property.clone(),
        }),
        vec![child],
    );

    match FindBestTask4GroupExpression(&parent, &property, &route()) {
        Ok(_) => panic!("Go parity requires the child error to stop group enumeration"),
        Err(error) => assert_eq!(error.to_string(), "child enumeration failed"),
    }
}

#[test]
fn child_error_stops_group_enumeration_before_or_after_a_valid_alternative() {
    let failing = || Arc::new(RoutedGroupExpression::new(Box::new(FailingLogical), vec![]));
    let valid = || Arc::new(RoutedGroupExpression::new(Box::new(LeafLogical), vec![]));

    assert_child_error_is_preserved(vec![failing(), valid()]);
    assert_child_error_is_preserved(vec![valid(), failing()]);
}
