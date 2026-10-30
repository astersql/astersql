// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
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

use astersql_parser_ast::RefreshMaterializedViewMode;
use std::time::SystemTime;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StepKind {
    Meta,
    FastMerge,
    CompleteDelete,
    CompleteInsert,
    CompleteDeltaApply,
    OutOfPlaceCreateShadow,
    OutOfPlaceLoadShadow,
    OutOfPlaceCutover,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ObserveStep {
    pub id: String,
    pub name: &'static str,
    pub kind: StepKind,
}

pub trait StepObserver {
    fn on_step_start(&mut self, step: &ObserveStep, at: SystemTime);
    fn on_step_end(&mut self, step: &ObserveStep, at: SystemTime, error: Option<&str>);
    fn on_step_plan_rows(&mut self, step: &ObserveStep, rows: Vec<Vec<String>>);
}

/// Rust's closed mode enum excludes the Go unknown-mode sentinel. Invalid AST
/// types must be rejected by RefreshMaterializedViewStmt::Mode before this call.
pub fn new_step_set(mode: RefreshMaterializedViewMode) -> Vec<ObserveStep> {
    use RefreshMaterializedViewMode as Mode;
    use StepKind::*;
    let mut steps = if mode == Mode::CompleteOutOfPlace {
        vec![
            ("INSERT_HIST_RUNNING", Meta),
            ("OUT_OF_PLACE_CREATE_SHADOW", OutOfPlaceCreateShadow),
            ("OUT_OF_PLACE_LOAD_SHADOW", OutOfPlaceLoadShadow),
            ("OUT_OF_PLACE_CUTOVER", OutOfPlaceCutover),
        ]
    } else {
        vec![
            ("TXN_BEGIN", Meta),
            ("LOCK_REFRESH_INFO", Meta),
            ("INSERT_HIST_RUNNING", Meta),
        ]
    };
    match mode {
        Mode::Fast => steps.push(("DATA_CHANGE_FAST_MERGE", FastMerge)),
        Mode::CompleteInPlace => {
            steps.push(("DATA_CHANGE_COMPLETE_DELETE", CompleteDelete));
            steps.push(("DATA_CHANGE_COMPLETE_INSERT", CompleteInsert));
        }
        Mode::CompleteDeltaApply => {
            steps.push(("DATA_CHANGE_COMPLETE_DELTA_APPLY", CompleteDeltaApply))
        }
        Mode::CompleteOutOfPlace => {}
    }
    if mode != Mode::CompleteOutOfPlace {
        steps.extend([("PERSIST_REFRESH_INFO", Meta), ("TXN_COMMIT", Meta)]);
    }
    steps.push(("FINALIZE_HIST", Meta));
    steps
        .into_iter()
        .enumerate()
        .map(|(index, (name, kind))| ObserveStep {
            id: format!("S{:02}", index + 1),
            name,
            kind,
        })
        .collect()
}

pub fn clone_plan_rows(rows: &[Vec<String>]) -> Vec<Vec<String>> {
    rows.to_vec()
}

/// End callbacks run on returned errors, as in Go. A panic still unwinds to the
/// refresh coordinator, which owns rollback and shadow cleanup.
pub fn observe_step<T>(
    observer: &mut Option<&mut dyn StepObserver>,
    step: &ObserveStep,
    run: impl FnOnce() -> Result<T, String>,
) -> Result<T, String> {
    if let Some(observer) = observer.as_deref_mut() {
        observer.on_step_start(step, SystemTime::now());
    }
    let result = run();
    if let Some(observer) = observer.as_deref_mut() {
        observer.on_step_end(
            step,
            SystemTime::now(),
            result.as_ref().err().map(String::as_str),
        );
    }
    result
}
