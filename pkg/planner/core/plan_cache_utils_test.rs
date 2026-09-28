// Copyright 2026 AsterSQL.
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

use std::sync::Mutex;

use super::*;

#[derive(Default)]
struct ObserveMarkersRuntime {
    markers_seen_by_build: Mutex<Vec<PlanCacheParamMarker>>,
}

impl PlanCachePrepareRuntime for ObserveMarkersRuntime {
    fn Preprocess(&self, _: &PlanCachePrepareInput) -> Result<(), PlanCacheError> {
        Ok(())
    }

    fn Build(
        &self,
        input: &PlanCachePrepareInput,
    ) -> Result<Box<dyn base_dependency::Plan>, PlanCacheError> {
        *self.markers_seen_by_build.lock().unwrap() = input.markers.clone();
        Err(PlanCacheError::new("stop after observing markers"))
    }

    fn CheckPreparedPrivileges(&self, _: &PlanCacheStmt) -> Result<(), PlanCacheError> {
        Ok(())
    }

    fn AppendWarning(&self, _: &str) {}
}

#[test]
fn non_prepared_parameters_keep_values_when_sorted_before_build() {
    let runtime = ObserveMarkersRuntime::default();
    let input = PlanCachePrepareInput {
        is_prepared_statement: false,
        markers: vec![
            PlanCacheParamMarker {
                offset: 20,
                datum: Some(crate::Datum::Int(22)),
                in_execute: true,
                ..Default::default()
            },
            PlanCacheParamMarker {
                offset: 10,
                datum: Some(crate::Datum::Int(11)),
                in_execute: true,
                ..Default::default()
            },
        ],
        ..Default::default()
    };

    assert!(GeneratePlanCacheStmtWithAST(&runtime, input).is_err());
    let markers = runtime.markers_seen_by_build.lock().unwrap();
    assert_eq!(markers[0].offset, 10);
    assert_eq!(markers[0].order, 0);
    assert_eq!(markers[0].datum, Some(crate::Datum::Int(11)));
    assert!(markers[0].in_execute);
    assert_eq!(markers[1].datum, Some(crate::Datum::Int(22)));
}
