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

use crate::show_slow_queries::{ShowSlowExec, ShowSlowSource, SlowQueryInfo};
use std::convert::Infallible;

#[derive(Default)]
struct EmptySource {
    opened: bool,
}

impl ShowSlowSource for EmptySource {
    type Request = ();
    type Error = Infallible;

    fn open(&mut self) -> Result<(), Self::Error> {
        self.opened = true;
        Ok(())
    }

    fn show_slow_query(&mut self, _request: &Self::Request) -> Vec<SlowQueryInfo> {
        Vec::new()
    }

    fn max_chunk_size(&self) -> usize {
        32
    }
}

#[test]
fn open_preserves_cursor_like_go_executor() {
    let mut executor = ShowSlowExec {
        source: EmptySource::default(),
        show_slow: (),
        result: Vec::new(),
        cursor: 7,
    };

    executor.Open(()).unwrap();

    assert!(executor.source.opened);
    assert_eq!(executor.cursor, 7);
}
