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

use crate::{MemTablePredicateExtractor, PhysicalPlan, PlanContext, types::NameSlice};

/// 编译期锁定 Go `types.NameSlice` 的可空字段名契约。
struct NameSliceExtractor;

impl MemTablePredicateExtractor for NameSliceExtractor {
    fn extract(
        &mut self,
        _ctx: &dyn PlanContext,
        _schema: &expression::Schema,
        _names: &NameSlice,
        predicates: &[expression::ExprBox],
    ) -> Vec<expression::ExprBox> {
        predicates.to_vec()
    }

    fn explain_info(&self, _plan: &dyn PhysicalPlan) -> String {
        String::new()
    }
}

#[test]
fn mem_table_extractor_accepts_nullable_name_slice() {
    let names = NameSlice(vec![None]);
    assert_eq!(names.0.len(), 1);
    assert!(names.0[0].is_none());
}
