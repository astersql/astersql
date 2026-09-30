// Copyright 2026 AsterSQL.
// Copyright 2021 TiKV Authors
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

use std::time::Duration;

use kvproto::kvrpcpb;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PoolTaskDetails {
    pub task_count: u64,
    pub poll_count: u64,
    pub max_poll_count: u64,
    pub min_poll_count: u64,
    pub dispatch_count: u64,
    pub max_dispatch_count: u64,
    pub min_dispatch_count: u64,
    pub total_wall_time: Duration,
    pub task_wall_time_sample_count: u64,
    pub max_task_wall_time: Duration,
    pub min_task_wall_time: Duration,
    pub total_queue_wait_time: Duration,
    pub max_queue_wait_time: Duration,
    pub min_queue_wait_time: Duration,
    pub total_wake_wait_time: Duration,
    pub max_wake_wait_time: Duration,
    pub min_wake_wait_time: Duration,
    pub fair_queue_sample_count: u64,
    pub total_fair_queue_waited_task_slices: u64,
    pub max_fair_queue_waited_task_slices: u64,
    pub min_fair_queue_waited_task_slices: u64,
    pub poll_cpu_time: Duration,
    pub max_poll_cpu_time: Duration,
    pub min_poll_cpu_time: Duration,
    pub poll_wall_time: Duration,
    pub max_poll_wall_time: Duration,
    pub min_poll_wall_time: Duration,
}

fn merge_min<T: Ord + Copy>(current: T, sample: T, had_samples: bool) -> T {
    if had_samples {
        current.min(sample)
    } else {
        sample
    }
}

impl PoolTaskDetails {
    pub fn merge_from_pb(&mut self, details: &kvrpcpb::PoolTaskDetails) {
        let had_poll_samples = self.poll_count > 0;
        let had_queue_wait_samples = self.total_queue_wait_time > Duration::ZERO;
        let had_wake_wait_samples = self.total_wake_wait_time > Duration::ZERO;
        let had_fair_queue_samples = self.fair_queue_sample_count > 0;
        let had_task_wall_samples = self.task_wall_time_sample_count > 0;
        let had_tasks = self.task_count > 0;

        self.task_count += 1;
        let poll_count = details.get_poll_count();
        self.poll_count += poll_count;
        self.max_poll_count = self.max_poll_count.max(poll_count);
        self.min_poll_count = merge_min(self.min_poll_count, poll_count, had_tasks);
        let dispatch_count = details.get_dispatch_count();
        self.dispatch_count += dispatch_count;
        self.max_dispatch_count = self.max_dispatch_count.max(dispatch_count);
        self.min_dispatch_count = merge_min(self.min_dispatch_count, dispatch_count, had_tasks);

        let wall = Duration::from_nanos(details.get_total_wall_nanos());
        self.total_wall_time += wall;
        if !wall.is_zero() {
            self.task_wall_time_sample_count += 1;
            self.max_task_wall_time = self.max_task_wall_time.max(wall);
            self.min_task_wall_time =
                merge_min(self.min_task_wall_time, wall, had_task_wall_samples);
        }

        let queue = Duration::from_nanos(details.get_total_queue_wait_nanos());
        self.total_queue_wait_time += queue;
        self.max_queue_wait_time = self
            .max_queue_wait_time
            .max(Duration::from_nanos(details.get_max_queue_wait_nanos()));
        if !queue.is_zero() {
            self.min_queue_wait_time = merge_min(
                self.min_queue_wait_time,
                Duration::from_nanos(details.get_min_queue_wait_nanos()),
                had_queue_wait_samples,
            );
        }

        let wake = Duration::from_nanos(details.get_total_wake_wait_nanos());
        self.total_wake_wait_time += wake;
        self.max_wake_wait_time = self
            .max_wake_wait_time
            .max(Duration::from_nanos(details.get_max_wake_wait_nanos()));
        if !wake.is_zero() {
            self.min_wake_wait_time = merge_min(
                self.min_wake_wait_time,
                Duration::from_nanos(details.get_min_wake_wait_nanos()),
                had_wake_wait_samples,
            );
        }

        if details.get_fair_queue_enabled() {
            self.fair_queue_sample_count += dispatch_count;
            self.total_fair_queue_waited_task_slices +=
                details.get_total_fair_queue_waited_task_slices();
            self.max_fair_queue_waited_task_slices = self
                .max_fair_queue_waited_task_slices
                .max(details.get_max_fair_queue_waited_task_slices());
            self.min_fair_queue_waited_task_slices = merge_min(
                self.min_fair_queue_waited_task_slices,
                details.get_min_fair_queue_waited_task_slices(),
                had_fair_queue_samples,
            );
        }

        self.poll_cpu_time += Duration::from_nanos(details.get_poll_cpu_nanos());
        self.max_poll_cpu_time = self
            .max_poll_cpu_time
            .max(Duration::from_nanos(details.get_max_poll_cpu_nanos()));
        self.poll_wall_time += Duration::from_nanos(details.get_poll_wall_nanos());
        self.max_poll_wall_time = self
            .max_poll_wall_time
            .max(Duration::from_nanos(details.get_max_poll_wall_nanos()));
        if poll_count > 0 {
            self.min_poll_cpu_time = merge_min(
                self.min_poll_cpu_time,
                Duration::from_nanos(details.get_min_poll_cpu_nanos()),
                had_poll_samples,
            );
            self.min_poll_wall_time = merge_min(
                self.min_poll_wall_time,
                Duration::from_nanos(details.get_min_poll_wall_nanos()),
                had_poll_samples,
            );
        }
    }
}
