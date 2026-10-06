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

//! Statement-local adaptive admission for ordered scans beneath `LIMIT`.

use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};

const ADAPTIVE_YIELD_WINDOW_SIZE: usize = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AdaptiveLimitMode {
    IndexJoin,
    DirectIndexLookup,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AdmissionStage {
    OuterRows,
    LookupHandles,
}

#[derive(Clone, Copy, Debug, Default)]
struct AdaptiveYieldWindow {
    inputs: [u64; ADAPTIVE_YIELD_WINDOW_SIZE],
    outputs: [u64; ADAPTIVE_YIELD_WINDOW_SIZE],
    next: usize,
}

impl AdaptiveYieldWindow {
    fn add(&mut self, input: u64, output: u64) {
        self.inputs[self.next] = input;
        self.outputs[self.next] = output;
        self.next = (self.next + 1) % ADAPTIVE_YIELD_WINDOW_SIZE;
    }

    fn totals(&self) -> (u64, u64) {
        self.inputs
            .iter()
            .zip(self.outputs.iter())
            .fold((0_u64, 0_u64), |(input, output), (&i, &o)| {
                (input.saturating_add(i), output.saturating_add(o))
            })
    }
}

#[derive(Debug, Default)]
struct AdmissionBlockStats {
    blocked_since: Option<Instant>,
    waiters: usize,
    blocked_time: Duration,
}

impl AdmissionBlockStats {
    fn begin(&mut self, now: Instant) {
        if self.waiters == 0 {
            self.blocked_since = Some(now);
        }
        self.waiters += 1;
    }

    fn end(&mut self, now: Instant) {
        if self.waiters == 0 {
            return;
        }
        self.waiters -= 1;
        if self.waiters == 0 {
            if let Some(start) = self.blocked_since.take() {
                self.blocked_time += now.saturating_duration_since(start);
            }
        }
    }

    fn finish(&mut self, now: Instant) {
        if self.waiters == 0 {
            return;
        }
        if let Some(start) = self.blocked_since.take() {
            self.blocked_time += now.saturating_duration_since(start);
        }
        self.waiters = 0;
    }

    fn elapsed(&self, now: Instant) -> Duration {
        self.blocked_time
            + self
                .blocked_since
                .map_or(Duration::ZERO, |start| now.saturating_duration_since(start))
    }
}

/// Immutable bounds for one statement-local adaptive LIMIT controller.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AdaptiveLimitConfig {
    pub demand_rows: u64,
    pub initial_outer_window: u64,
    pub max_outer_window: u64,
    pub initial_lookup_window: u64,
    pub max_lookup_window: u64,
    pub initial_lookup_batch_size: u64,
    pub max_lookup_batch_size: u64,
}

/// Point-in-time controller counters used by tests and runtime diagnostics.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AdaptiveLimitSnapshot {
    pub demand_rows: u64,
    pub output_rows: u64,
    pub outer_fetched: u64,
    pub outer_consumed: u64,
    pub outer_reserved: u64,
    pub outer_window: u64,
    pub outer_outstanding_at_stop: u64,
    pub lookup_reserved: u64,
    pub lookup_handles: u64,
    pub lookup_rows: u64,
    pub lookup_window: u64,
    pub lookup_batch_size: u64,
    pub lookup_physical_window: u64,
    pub lookup_outstanding_at_stop: u64,
    pub outer_admission_blocked: Duration,
    pub lookup_admission_blocked: Duration,
    pub stopped: bool,
}

#[derive(Debug)]
struct ControllerState {
    mode: AdaptiveLimitMode,
    demand_rows: u64,
    output_rows: u64,
    initial_outer_window: u64,
    max_outer_window: u64,
    outer_window: u64,
    outer_fetched: u64,
    outer_consumed: u64,
    outer_reserved: u64,
    outer_outstanding_at_stop: u64,
    pending_outer_output: u64,
    recent_outer_yield: AdaptiveYieldWindow,
    outer_no_output_rows: u64,
    outer_growth_barrier: u64,
    initial_lookup_window: u64,
    max_lookup_window: u64,
    lookup_window: u64,
    initial_lookup_batch_size: u64,
    max_lookup_batch_size: u64,
    lookup_batch_size: u64,
    lookup_reserved: u64,
    lookup_handles: u64,
    lookup_rows: u64,
    lookup_outstanding_at_stop: u64,
    recent_lookup_yield: AdaptiveYieldWindow,
    lookup_no_output_rows: u64,
    lookup_in_no_output_phase: bool,
    lookup_growth_progress: u64,
    outer_admission_blocked: AdmissionBlockStats,
    lookup_admission_blocked: AdmissionBlockStats,
    stopped: bool,
}

/// Bounds speculative outer rows and lookup handles for an early-stop LIMIT.
#[derive(Debug)]
pub struct AdaptiveLimitController {
    state: Mutex<ControllerState>,
    outer_changed: Condvar,
    lookup_changed: Condvar,
}

impl AdaptiveLimitController {
    pub fn NewAdaptiveLimitController(config: AdaptiveLimitConfig) -> Self {
        Self::new(config, AdaptiveLimitMode::IndexJoin)
    }

    pub fn NewAdaptiveLimitLookupController(config: AdaptiveLimitConfig) -> Self {
        Self::new(config, AdaptiveLimitMode::DirectIndexLookup)
    }

    fn new(config: AdaptiveLimitConfig, mode: AdaptiveLimitMode) -> Self {
        let (mut initial_outer_window, mut max_outer_window) =
            normalize_adaptive_window(config.initial_outer_window, config.max_outer_window);
        let (mut initial_lookup_window, max_lookup_window) =
            normalize_adaptive_window(config.initial_lookup_window, config.max_lookup_window);
        let max_lookup_batch_size = config.max_lookup_batch_size.max(1).min(max_lookup_window);
        let initial_lookup_batch_size = config
            .initial_lookup_batch_size
            .max(initial_lookup_window.min(max_lookup_batch_size))
            .min(max_lookup_batch_size);
        if config.demand_rows > 0 {
            initial_outer_window = initial_outer_window.min(config.demand_rows);
            initial_lookup_window = initial_lookup_window.min(config.demand_rows);
        }
        if mode == AdaptiveLimitMode::DirectIndexLookup {
            initial_outer_window = 0;
            max_outer_window = 0;
        }
        let mut state = ControllerState {
            mode,
            demand_rows: config.demand_rows,
            output_rows: 0,
            initial_outer_window,
            max_outer_window,
            outer_window: initial_outer_window,
            outer_fetched: 0,
            outer_consumed: 0,
            outer_reserved: 0,
            outer_outstanding_at_stop: 0,
            pending_outer_output: 0,
            recent_outer_yield: AdaptiveYieldWindow::default(),
            outer_no_output_rows: 0,
            outer_growth_barrier: 0,
            initial_lookup_window,
            max_lookup_window,
            lookup_window: initial_lookup_window,
            initial_lookup_batch_size,
            max_lookup_batch_size,
            lookup_batch_size: initial_lookup_batch_size,
            lookup_reserved: 0,
            lookup_handles: 0,
            lookup_rows: 0,
            lookup_outstanding_at_stop: 0,
            recent_lookup_yield: AdaptiveYieldWindow::default(),
            lookup_no_output_rows: 0,
            lookup_in_no_output_phase: false,
            lookup_growth_progress: 0,
            outer_admission_blocked: AdmissionBlockStats::default(),
            lookup_admission_blocked: AdmissionBlockStats::default(),
            stopped: false,
        };
        if config.demand_rows == 0 {
            stop_locked(&mut state);
        }
        Self {
            state: Mutex::new(state),
            outer_changed: Condvar::new(),
            lookup_changed: Condvar::new(),
        }
    }

    pub fn Reset(&self) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.output_rows = 0;
        state.outer_fetched = 0;
        state.outer_consumed = 0;
        state.outer_reserved = 0;
        state.outer_outstanding_at_stop = 0;
        state.pending_outer_output = 0;
        state.recent_outer_yield = AdaptiveYieldWindow::default();
        state.outer_no_output_rows = 0;
        state.outer_window = state.initial_outer_window;
        state.outer_growth_barrier = 0;
        state.lookup_reserved = 0;
        state.lookup_handles = 0;
        state.lookup_rows = 0;
        state.recent_lookup_yield = AdaptiveYieldWindow::default();
        state.lookup_no_output_rows = 0;
        state.lookup_in_no_output_phase = false;
        state.lookup_window = state.initial_lookup_window;
        state.lookup_batch_size = state.initial_lookup_batch_size;
        state.lookup_growth_progress = 0;
        state.lookup_outstanding_at_stop = 0;
        state.outer_admission_blocked = AdmissionBlockStats::default();
        state.lookup_admission_blocked = AdmissionBlockStats::default();
        state.stopped = false;
        if state.demand_rows == 0 {
            stop_locked(&mut state);
        }
        drop(state);
        self.outer_changed.notify_all();
        self.lookup_changed.notify_all();
    }

    pub fn ReserveOuter(&self, max_rows: usize) -> (usize, bool) {
        let direct = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .mode
            == AdaptiveLimitMode::DirectIndexLookup;
        if direct {
            return (0, false);
        }
        self.reserve(max_rows, AdmissionStage::OuterRows)
    }

    pub fn ReserveLookup(&self, max_handles: usize) -> (usize, bool) {
        self.reserve(max_handles, AdmissionStage::LookupHandles)
    }

    fn reserve(&self, max_units: usize, stage: AdmissionStage) -> (usize, bool) {
        if max_units == 0 {
            return (0, true);
        }
        let mut waiting = false;
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        loop {
            if state.stopped {
                if waiting {
                    block_stats_mut(&mut state, stage).end(Instant::now());
                }
                return (0, false);
            }
            let (window, outstanding) = match stage {
                AdmissionStage::OuterRows => (
                    state.outer_window,
                    state.outer_fetched.saturating_sub(state.outer_consumed) + state.outer_reserved,
                ),
                AdmissionStage::LookupHandles => {
                    (lookup_physical_window(&state), state.lookup_reserved)
                }
            };
            if outstanding < window {
                if waiting {
                    block_stats_mut(&mut state, stage).end(Instant::now());
                }
                let mut units = (max_units as u64).min(window - outstanding);
                match stage {
                    AdmissionStage::OuterRows => state.outer_reserved += units,
                    AdmissionStage::LookupHandles => {
                        units = units.min(state.lookup_batch_size);
                        state.lookup_reserved += units;
                    }
                }
                return (units as usize, true);
            }
            if !waiting {
                block_stats_mut(&mut state, stage).begin(Instant::now());
                waiting = true;
            }
            state = match stage {
                AdmissionStage::OuterRows => self.outer_changed.wait(state),
                AdmissionStage::LookupHandles => self.lookup_changed.wait(state),
            }
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
    }

    pub fn CommitOuter(&self, reserved: usize, fetched: usize) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let released = (reserved as u64).min(state.outer_reserved);
        state.outer_reserved -= released;
        state.outer_fetched = state
            .outer_fetched
            .saturating_add((fetched as u64).min(released));
        drop(state);
        self.outer_changed.notify_all();
    }

    pub fn ObserveJoinProgress(&self, consumed_rows: usize, output_rows: usize) {
        if consumed_rows == 0 && output_rows == 0 {
            return;
        }
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let previous = state.outer_consumed;
        state.outer_consumed = state
            .outer_fetched
            .min(state.outer_consumed.saturating_add(consumed_rows as u64));
        let consumed = state.outer_consumed - previous;
        state.output_rows = state.output_rows.saturating_add(output_rows as u64);
        if state.output_rows >= state.demand_rows {
            stop_locked(&mut state);
            drop(state);
            self.outer_changed.notify_all();
            self.lookup_changed.notify_all();
            return;
        }
        if consumed == 0 {
            state.pending_outer_output = state
                .pending_outer_output
                .saturating_add(output_rows as u64);
            return;
        }
        let paired_output = state
            .pending_outer_output
            .saturating_add(output_rows as u64);
        state.pending_outer_output = 0;
        if paired_output > 0 {
            state.recent_outer_yield.add(consumed, paired_output);
            state.outer_no_output_rows = 0;
            recompute_outer_window(&mut state);
            recompute_lookup_window(&mut state);
        } else {
            if state.outer_no_output_rows == 0 {
                state.recent_outer_yield = AdaptiveYieldWindow::default();
            }
            state.outer_no_output_rows = state.outer_no_output_rows.saturating_add(consumed);
            grow_outer_window_if_drained(&mut state);
        }
        drop(state);
        self.outer_changed.notify_all();
        self.lookup_changed.notify_all();
    }

    pub fn CompleteLookup(&self, reserved: usize, handles: usize, rows: usize) {
        if reserved == 0 {
            return;
        }
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.stopped || reserved as u64 > state.lookup_reserved {
            return;
        }
        state.lookup_reserved -= reserved as u64;
        state.lookup_handles = state.lookup_handles.saturating_add(handles as u64);
        state.lookup_rows = state.lookup_rows.saturating_add(rows as u64);
        if state.mode == AdaptiveLimitMode::DirectIndexLookup {
            state.output_rows = state.output_rows.saturating_add(rows as u64);
            if state.output_rows >= state.demand_rows {
                stop_locked(&mut state);
                drop(state);
                self.outer_changed.notify_all();
                self.lookup_changed.notify_all();
                return;
            }
        }
        if rows > 0 {
            state.recent_lookup_yield.add(handles as u64, rows as u64);
            state.lookup_no_output_rows = 0;
            state.lookup_in_no_output_phase = false;
            recompute_lookup_window(&mut state);
        } else {
            if !state.lookup_in_no_output_phase {
                state.recent_lookup_yield = AdaptiveYieldWindow::default();
            }
            state.lookup_in_no_output_phase = true;
            state.lookup_no_output_rows = state
                .lookup_no_output_rows
                .saturating_add((handles as u64).max(1));
            grow_lookup_window_if_drained(&mut state);
        }
        drop(state);
        self.lookup_changed.notify_all();
    }

    pub fn AbortLookup(&self, handles: usize) {
        if handles == 0 {
            return;
        }
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.lookup_reserved -= (handles as u64).min(state.lookup_reserved);
        drop(state);
        self.lookup_changed.notify_all();
    }

    pub fn SuggestedBatchSize(&self, ceiling: usize) -> usize {
        if ceiling < 1 {
            return 1;
        }
        let state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        (state.lookup_batch_size.min(ceiling as u64) as usize).max(1)
    }

    pub fn Stop(&self) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        stop_locked(&mut state);
        drop(state);
        self.outer_changed.notify_all();
        self.lookup_changed.notify_all();
    }

    pub fn Snapshot(&self) -> AdaptiveLimitSnapshot {
        let state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let now = Instant::now();
        AdaptiveLimitSnapshot {
            demand_rows: state.demand_rows,
            output_rows: state.output_rows,
            outer_fetched: state.outer_fetched,
            outer_consumed: state.outer_consumed,
            outer_reserved: state.outer_reserved,
            outer_window: state.outer_window,
            outer_outstanding_at_stop: state.outer_outstanding_at_stop,
            lookup_reserved: state.lookup_reserved,
            lookup_handles: state.lookup_handles,
            lookup_rows: state.lookup_rows,
            lookup_window: state.lookup_window,
            lookup_batch_size: state.lookup_batch_size,
            lookup_physical_window: lookup_physical_window(&state),
            lookup_outstanding_at_stop: state.lookup_outstanding_at_stop,
            outer_admission_blocked: state.outer_admission_blocked.elapsed(now),
            lookup_admission_blocked: state.lookup_admission_blocked.elapsed(now),
            stopped: state.stopped,
        }
    }
}

fn block_stats_mut(state: &mut ControllerState, stage: AdmissionStage) -> &mut AdmissionBlockStats {
    match stage {
        AdmissionStage::OuterRows => &mut state.outer_admission_blocked,
        AdmissionStage::LookupHandles => &mut state.lookup_admission_blocked,
    }
}

fn recompute_outer_window(state: &mut ControllerState) {
    let remaining = state.demand_rows - state.output_rows;
    let mut estimated = divide_and_round_up(
        remaining.saturating_mul(state.outer_consumed),
        state.output_rows,
    );
    let (recent_consumed, recent_output) = state.recent_outer_yield.totals();
    if recent_output > 0 {
        estimated = estimated.max(divide_and_round_up(
            remaining.saturating_mul(recent_consumed),
            recent_output,
        ));
    }
    let target = add_adaptive_window_headroom(estimated, remaining, state.demand_rows);
    let (window, grew) = adjust_adaptive_window(
        target,
        state.outer_window,
        1,
        state.max_outer_window,
        state.outer_consumed > state.outer_growth_barrier,
    );
    state.outer_window = window;
    if grew {
        state.outer_growth_barrier = state.outer_fetched;
    }
}

fn recompute_lookup_window(state: &mut ControllerState) {
    if state.mode == AdaptiveLimitMode::DirectIndexLookup {
        recompute_direct_lookup_window(state);
        return;
    }
    if state.lookup_rows == 0 || state.lookup_in_no_output_phase {
        return;
    }
    let lookup_buffered = state.lookup_rows.saturating_sub(state.outer_consumed);
    let outer_buffered = state.outer_fetched.saturating_sub(state.outer_consumed);
    let remaining_outer = state
        .outer_window
        .saturating_sub(lookup_buffered.max(outer_buffered));
    let mut target = divide_and_round_up(
        remaining_outer.saturating_mul(state.lookup_handles),
        state.lookup_rows,
    );
    let (recent_handles, recent_rows) = state.recent_lookup_yield.totals();
    if recent_rows > 0 {
        target = target.max(divide_and_round_up(
            remaining_outer.saturating_mul(recent_handles),
            recent_rows,
        ));
    }
    let (window, grew) = adjust_adaptive_window(
        target,
        state.lookup_window,
        state.initial_lookup_window,
        state.max_lookup_window,
        state.lookup_handles > state.lookup_growth_progress,
    );
    state.lookup_window = window;
    if grew {
        state.lookup_growth_progress = state.lookup_handles;
    }
}

fn recompute_direct_lookup_window(state: &mut ControllerState) {
    if state.lookup_rows == 0 || state.lookup_in_no_output_phase {
        return;
    }
    let remaining = state.demand_rows.saturating_sub(state.output_rows);
    let mut estimated = divide_and_round_up(
        remaining.saturating_mul(state.lookup_handles),
        state.lookup_rows,
    );
    let (recent_handles, recent_rows) = state.recent_lookup_yield.totals();
    if recent_rows > 0 {
        estimated = estimated.max(divide_and_round_up(
            remaining.saturating_mul(recent_handles),
            recent_rows,
        ));
    }
    let target = add_adaptive_window_headroom(estimated, remaining, state.demand_rows);
    let (window, grew) = adjust_adaptive_window(
        target,
        state.lookup_window,
        state.initial_lookup_window,
        state.max_lookup_window,
        state.lookup_handles > state.lookup_growth_progress,
    );
    state.lookup_window = window;
    if grew {
        state.lookup_growth_progress = state.lookup_handles;
    }
}

fn grow_outer_window_if_drained(state: &mut ControllerState) {
    let outstanding =
        state.outer_fetched.saturating_sub(state.outer_consumed) + state.outer_reserved;
    if outstanding != 0 || state.outer_no_output_rows < state.outer_window {
        return;
    }
    let next = grow_adaptive_window(state.outer_window, state.max_outer_window);
    if next > state.outer_window {
        state.outer_growth_barrier = state.outer_fetched;
    }
    state.outer_window = next;
    state.outer_no_output_rows = 0;
    if !state.lookup_in_no_output_phase {
        recompute_lookup_window(state);
    }
}

fn grow_lookup_window_if_drained(state: &mut ControllerState) {
    if state.lookup_reserved != 0 || state.lookup_no_output_rows < state.lookup_window {
        return;
    }
    let next = grow_adaptive_window(state.lookup_window, state.max_lookup_window);
    if next > state.lookup_window {
        state.lookup_growth_progress = state.lookup_handles;
    }
    state.lookup_window = next;
    state.lookup_batch_size =
        grow_adaptive_window(state.lookup_batch_size, state.max_lookup_batch_size);
    state.lookup_no_output_rows = 0;
}

fn lookup_physical_window(state: &ControllerState) -> u64 {
    if state.lookup_window == 0 || state.lookup_batch_size == 0 {
        return 0;
    }
    divide_and_round_up(state.lookup_window, state.lookup_batch_size)
        .saturating_mul(state.lookup_batch_size)
        .min(state.max_lookup_window)
}

fn stop_locked(state: &mut ControllerState) {
    if state.stopped {
        return;
    }
    state.stopped = true;
    let now = Instant::now();
    state.outer_admission_blocked.finish(now);
    state.lookup_admission_blocked.finish(now);
    state.outer_outstanding_at_stop =
        state.outer_fetched.saturating_sub(state.outer_consumed) + state.outer_reserved;
    state.lookup_outstanding_at_stop = state.lookup_reserved;
    state.lookup_reserved = 0;
    state.outer_reserved = 0;
    state.outer_window = 0;
    state.lookup_window = 0;
    state.lookup_batch_size = 0;
}

fn normalize_adaptive_window(initial: u64, maximum: u64) -> (u64, u64) {
    let initial = initial.max(1);
    (initial, maximum.max(initial))
}

fn grow_adaptive_window(window: u64, maximum: u64) -> u64 {
    window.saturating_mul(2).min(maximum)
}

fn add_adaptive_window_headroom(estimated: u64, remaining: u64, demand: u64) -> u64 {
    if remaining <= demand / 4 {
        estimated
    } else if remaining <= demand / 2 {
        divide_and_round_up(estimated.saturating_mul(9), 8)
    } else {
        divide_and_round_up(estimated.saturating_mul(5), 4)
    }
}

fn adjust_adaptive_window(
    target: u64,
    current: u64,
    minimum: u64,
    maximum: u64,
    can_grow: bool,
) -> (u64, bool) {
    let target = target.max(minimum).min(maximum);
    if target > current {
        if !can_grow {
            return (current, false);
        }
        return (target.min(grow_adaptive_window(current, maximum)), true);
    }
    (target, false)
}

fn divide_and_round_up(value: u64, divisor: u64) -> u64 {
    if divisor == 0 {
        0
    } else {
        value / divisor + u64::from(value % divisor != 0)
    }
}
