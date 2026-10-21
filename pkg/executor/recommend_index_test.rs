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

use super::recommend_index::{IndexAdvisor, RecommendIndexExec, RecommendIndexResult};
use astersql_parser_ast::{RecommendIndexActionReturn, RecommendIndexActionRun};
use astersql_util_chunk::Chunk;
use std::collections::BTreeMap;

#[derive(Default)]
struct RecordingAdvisor {
    persist_calls: Vec<bool>,
}

impl IndexAdvisor for RecordingAdvisor {
    type Context = ();
    type Option = ();
    type Error = String;

    fn set_options(&mut self, _: &mut (), _: &[()]) -> Result<(), String> {
        Ok(())
    }

    fn all_options(&self) -> &[String] {
        &[]
    }

    fn get_options(
        &mut self,
        _: &mut (),
    ) -> Result<(BTreeMap<String, String>, BTreeMap<String, String>), String> {
        Ok((BTreeMap::new(), BTreeMap::new()))
    }

    fn advise_indexes<C>(
        &mut self,
        _: C,
        _: &mut (),
        _: Vec<String>,
        _: &[()],
        persist: bool,
    ) -> (Vec<RecommendIndexResult>, Result<(), String>) {
        self.persist_calls.push(persist);
        (Vec::new(), Ok(()))
    }

    fn unsupported_action(&self, action: &str) -> String {
        format!("unsupported action: {action}")
    }

    fn empty_sqls(&self) -> String {
        "empty SQLs".into()
    }
}

fn executor(action: &str) -> RecommendIndexExec<RecordingAdvisor> {
    RecommendIndexExec {
        advisor: RecordingAdvisor::default(),
        context: (),
        action: action.into(),
        sql: "select * from t where a=1".into(),
        advise_id: 0,
        options: Vec::new(),
        done: false,
    }
}

#[test]
fn recommend_index_return_result_does_not_persist() {
    let mut return_result = executor(RecommendIndexActionReturn);
    return_result.Next((), &mut Chunk::default()).unwrap();
    assert_eq!(return_result.advisor.persist_calls, [false]);

    let mut run = executor(RecommendIndexActionRun);
    run.Next((), &mut Chunk::default()).unwrap();
    assert_eq!(run.advisor.persist_calls, [true]);
}
