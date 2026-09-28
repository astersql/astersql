// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc.
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

//! Adversarial parity regressions for `tracing.go`.

use std::io::{self, Write};
use std::sync::{Arc, Mutex};
use std::time::{Duration, UNIX_EPOCH};

use crate::{
    Context, MemoryStore, SpanRec, Tabby, Trace, TracerFinishSpan, dfsTree,
    format_clock_micros_with_offset_for_test, format_go_duration,
};

#[derive(Clone, Default)]
struct SharedWriter(Arc<Mutex<Vec<u8>>>);

impl Write for SharedWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl SharedWriter {
    fn contents(&self) -> String {
        String::from_utf8(
            self.0
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .clone(),
        )
        .expect("trace output must be UTF-8")
    }
}

fn trace(name: &str, start_micros: u64, duration_micros: u64, sub: Vec<Trace>) -> Trace {
    let start = UNIX_EPOCH + Duration::from_micros(start_micros);
    Trace {
        Span: SpanRec {
            name: name.to_string(),
            start,
            end: start + Duration::from_micros(duration_micros),
        },
        Sub: sub,
    }
}

#[test]
fn duration_keeps_go_zero_second_and_minute_components() {
    assert_eq!(format_go_duration(Duration::from_secs(60)), "1m0s");
    assert_eq!(format_go_duration(Duration::from_secs(3600)), "1h0m0s");
    assert_eq!(
        format_go_duration(Duration::from_millis(3_600_500)),
        "1h0m0.5s"
    );
}

#[test]
fn tabby_uses_go_tabwriter_rune_width_for_tree_glyphs() {
    let output = SharedWriter::default();
    let mut tabby = Tabby::NewCustom(Box::new(output.clone()));
    tabby.AddLine("jobA", "a", "x");
    tabby.AddLine("  └─jobB", "b", "y");
    tabby.Print();

    assert_eq!(output.contents(), "jobA      a  x\n  └─jobB  b  y\n");
}

#[test]
fn dfs_tree_sorts_children_in_place_like_go() {
    let late = trace("late", 20, 1, vec![]);
    let early = trace("early", 10, 1, vec![]);
    let mut root = trace("root", 0, 30, vec![late, early]);
    let output = SharedWriter::default();
    let mut tabby = Tabby::NewCustom(Box::new(output));

    dfsTree(&mut root, "", false, &mut tabby);

    let names: Vec<&str> = root
        .Sub
        .iter()
        .map(|child| child.Span.name.as_str())
        .collect();
    assert_eq!(names, ["early", "late"]);
}

#[test]
fn clock_column_applies_go_local_numeric_offset() {
    assert_eq!(
        format_clock_micros_with_offset_for_test(UNIX_EPOCH, 8 * 60 * 60),
        "08:00:00.000000"
    );
    assert_eq!(
        format_clock_micros_with_offset_for_test(UNIX_EPOCH, -(5 * 60 + 30) * 60),
        "18:30:00.000000"
    );
}

#[test]
#[should_panic(expected = "active span")]
fn finish_without_active_span_matches_go_nil_span_panic() {
    TracerFinishSpan(Context::Background(), MemoryStore::NewMemoryStore());
}
