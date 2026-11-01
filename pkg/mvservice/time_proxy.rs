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

use crossbeam_channel::{Receiver, Sender, bounded};
use std::{
    sync::{Arc, Mutex, OnceLock},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
/// Signed nanoseconds since Unix epoch; supports times before the epoch.
pub type Timestamp = i128;
#[derive(Clone)]
pub enum Clock {
    Real,
    Mock(Arc<MockTimeModule>),
}
struct Waiter {
    deadline: Timestamp,
    tx: Sender<Timestamp>,
    timer: Option<(std::sync::Weak<Mutex<TimerState>>, u64)>,
}
impl Waiter {
    fn fire(&self, now: Timestamp) {
        if let Some((weak, seq)) = &self.timer {
            if let Some(state) = weak.upgrade() {
                let mut s = state.lock().unwrap();
                if s.active && s.seq == *seq {
                    s.active = false;
                    let _ = self.tx.try_send(now);
                }
            }
        } else {
            let _ = self.tx.try_send(now);
        }
    }
}
// Real deadlines use a monotonic clock, so wall-clock adjustments cannot
// extend task timeouts. One shared scheduler replaces Go runtime timers.
fn schedule_real(waiter: Waiter, delay: Duration) {
    type Scheduled = (std::time::Instant, Waiter);
    static SCHEDULER: OnceLock<Sender<Scheduled>> = OnceLock::new();
    SCHEDULER
        .get_or_init(|| {
            let (tx, rx) = crossbeam_channel::unbounded::<Scheduled>();
            std::thread::spawn(move || {
                let mut waits: Vec<Scheduled> = Vec::new();
                loop {
                    let now = std::time::Instant::now();
                    waits.retain(|(at, w)| {
                        if *at <= now {
                            w.fire(Clock::Real.now());
                            false
                        } else {
                            true
                        }
                    });
                    let received = match waits.iter().map(|(at, _)| *at).min() {
                        None => rx.recv().ok(),
                        Some(at) => rx
                            .recv_timeout(at.saturating_duration_since(std::time::Instant::now()))
                            .ok(),
                    };
                    if let Some(w) = received {
                        waits.push(w)
                    }
                }
            });
            tx
        })
        .send((std::time::Instant::now() + delay, waiter))
        .unwrap();
}
struct MockState {
    now: Timestamp,
    auto: bool,
    waiters: Vec<Waiter>,
}
pub struct MockTimeModule {
    state: Mutex<MockState>,
}
impl MockTimeModule {
    pub fn new(now: Timestamp) -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(MockState {
                now,
                auto: true,
                waiters: Vec::new(),
            }),
        })
    }
    pub fn now(&self) -> Timestamp {
        self.state.lock().unwrap().now
    }
    pub fn set_auto_advance_on_sleep(&self, v: bool) {
        self.state.lock().unwrap().auto = v
    }
    fn fire(s: &mut MockState) {
        let now = s.now;
        s.waiters.retain(|w| {
            if w.deadline <= now {
                w.fire(now);
                false
            } else {
                true
            }
        })
    }
    pub fn set(&self, now: Timestamp) {
        let mut s = self.state.lock().unwrap();
        s.now = now;
        Self::fire(&mut s)
    }
    pub fn advance(&self, d: Duration) {
        let mut s = self.state.lock().unwrap();
        s.now += d.as_nanos() as i128;
        Self::fire(&mut s)
    }
    pub fn pending(&self) -> usize {
        self.state.lock().unwrap().waiters.len()
    }
    pub fn after(&self, d: Duration) -> Receiver<Timestamp> {
        let (tx, rx) = bounded(1);
        let mut s = self.state.lock().unwrap();
        let deadline = s.now + d.as_nanos() as i128;
        if d.is_zero() {
            let _ = tx.send(s.now);
        } else {
            s.waiters.push(Waiter {
                deadline,
                tx,
                timer: None,
            })
        }
        rx
    }
    pub fn sleep(&self, d: Duration) {
        if d.is_zero() {
            return;
        }
        if self.state.lock().unwrap().auto {
            self.advance(d)
        } else {
            let _ = self.after(d).recv();
        }
    }
}
impl Clock {
    pub fn now(&self) -> Timestamp {
        match self {
            Self::Mock(m) => m.now(),
            Self::Real => match SystemTime::now().duration_since(UNIX_EPOCH) {
                Ok(d) => d.as_nanos() as i128,
                Err(e) => -(e.duration().as_nanos() as i128),
            },
        }
    }
    pub fn after(&self, d: Duration) -> Receiver<Timestamp> {
        match self {
            Self::Mock(m) => m.after(d),
            Self::Real => {
                let (tx, rx) = bounded(1);
                schedule_real(
                    Waiter {
                        deadline: self.now() + d.as_nanos() as i128,
                        tx,
                        timer: None,
                    },
                    d,
                );
                rx
            }
        }
    }
    pub fn sleep(&self, d: Duration) {
        match self {
            Self::Mock(m) => m.sleep(d),
            Self::Real => std::thread::sleep(d),
        }
    }
    pub fn since(&self, t: Timestamp) -> i128 {
        self.now() - t
    }
    pub fn until(&self, t: Timestamp) -> i128 {
        t - self.now()
    }
    pub fn timer(&self, d: Duration) -> Timer {
        Timer::new(self.clone(), d)
    }
}
pub fn unix(sec: i64, nsec: i64) -> Timestamp {
    sec as i128 * 1_000_000_000 + nsec as i128
}
pub fn unix_milli(ms: i64) -> Timestamp {
    ms as i128 * 1_000_000
}
fn active() -> &'static Mutex<Option<Arc<MockTimeModule>>> {
    static ACTIVE: OnceLock<Mutex<Option<Arc<MockTimeModule>>>> = OnceLock::new();
    ACTIVE.get_or_init(|| Mutex::new(None))
}
pub fn current_clock() -> Clock {
    active()
        .lock()
        .unwrap()
        .clone()
        .map_or(Clock::Real, Clock::Mock)
}
pub struct RestoreClock(Option<Arc<MockTimeModule>>);
impl Drop for RestoreClock {
    fn drop(&mut self) {
        *active().lock().unwrap() = self.0.take();
    }
}
pub fn install_mock_time_module_for_test(module: Option<Arc<MockTimeModule>>) -> RestoreClock {
    let m = module.expect("mock time module is nil");
    RestoreClock(active().lock().unwrap().replace(m))
}
struct TimerState {
    seq: u64,
    active: bool,
}
pub struct Timer {
    pub c: Receiver<Timestamp>,
    tx: Sender<Timestamp>,
    state: Arc<Mutex<TimerState>>,
    clock: Clock,
}
impl Timer {
    fn new(clock: Clock, d: Duration) -> Self {
        let (tx, c) = bounded(1);
        let t = Self {
            c,
            tx,
            state: Arc::new(Mutex::new(TimerState {
                seq: 0,
                active: false,
            })),
            clock,
        };
        t.reset(d);
        t
    }
    pub fn stop(&self) -> bool {
        let mut s = self.state.lock().unwrap();
        let was = s.active;
        s.seq += 1;
        s.active = false;
        was
    }
    pub fn reset(&self, d: Duration) -> bool {
        let mut s = self.state.lock().unwrap();
        let was = s.active;
        s.seq += 1;
        let seq = s.seq;
        s.active = true;
        drop(s);
        let tx = self.tx.clone();
        let waiter = Waiter {
            deadline: self.clock.now() + d.as_nanos() as i128,
            tx,
            timer: Some((Arc::downgrade(&self.state), seq)),
        };
        match &self.clock {
            Clock::Real => schedule_real(waiter, d),
            Clock::Mock(m) => {
                let mut s = m.state.lock().unwrap();
                if waiter.deadline <= s.now {
                    waiter.fire(s.now)
                } else {
                    s.waiters.push(waiter)
                }
            }
        }
        was
    }
}
impl Drop for Timer {
    fn drop(&mut self) {
        self.stop();
    }
}
