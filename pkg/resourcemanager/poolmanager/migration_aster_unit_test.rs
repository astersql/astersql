// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
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

// poolmanager 任务元数据与调度器的迁移对照单测。
//
// 校验 Meta 运行计数/通道语义，以及 TaskManager 分片注册、Overclock、Downclock
// 与 Go 侧边界一致。

#[cfg(test)]
mod tests {
    use super::super::{Meta, SignalChannel, TaskChannel, TaskManager};
    use std::sync::atomic::Ordering;

    /// Meta 应保留任务 ID、通道引用，并正确增减 running 计数。
    #[test]
    fn meta_preserves_identity_channels_and_running_count() {
        let exit = SignalChannel::bounded(1);
        let tasks = TaskChannel::new();
        let meta = Meta::NewMeta(17, exit.clone(), tasks.clone(), 2);

        assert_eq!(meta.TaskID(), 17);
        assert_eq!(meta.running.load(Ordering::SeqCst), 0);
        meta.IncTask();
        meta.IncTask();
        meta.DecTask();
        assert_eq!(meta.running.load(Ordering::SeqCst), 1);

        tasks.send(Box::new(|| {})).unwrap();
        assert!(meta.GetTaskCh().try_recv().unwrap().is_some());
        tasks.close();
        assert!(meta.GetTaskCh().is_closed());
        assert!(tasks.send(Box::new(|| {})).is_err());
        assert!(!meta.GetExitCh().is_closed());
    }

    /// 同 shard 多任务注册后 DeleteTask 只删除指定 ID。
    #[test]
    fn manager_registers_by_shard_and_deletes_tasks() {
        let manager = TaskManager::NewTaskManager(4);
        let first = Meta::new(1, TaskChannel::new(), 1);
        let same_shard = Meta::new(9, TaskChannel::new(), 1);
        manager.RegisterTask(first.clone());
        manager.RegisterTask(same_shard.clone());

        assert_eq!(manager.GetOriginConcurrency(), 4);
        assert_eq!(manager.task[1].stats.read().unwrap().len(), 2);
        manager.DeleteTask(1);
        assert!(!manager.task[1].stats.read().unwrap().contains_key(&1));
        assert!(manager.task[1].stats.read().unwrap().contains_key(&9));
    }

    /// Overclock 优先选择尚未达到初始并发上限的任务。
    #[test]
    fn overclock_prefers_a_task_below_initial_concurrency() {
        let manager = TaskManager::NewTaskManager(2);
        let saturated = Meta::new(1, TaskChannel::new(), 1);
        saturated.IncTask();
        let boostable = Meta::new(2, TaskChannel::new(), 2);
        boostable.IncTask();
        manager.RegisterTask(saturated);
        manager.RegisterTask(boostable.clone());

        let (id, selected) = manager.Overclock();
        assert_eq!(id, 2);
        assert_eq!(selected.unwrap().TaskID(), 2);
    }

    /// Downclock 向超频任务的 exit 通道发送信号且不阻塞。
    #[test]
    fn downclock_signals_an_overclocked_task_without_blocking() {
        let manager = TaskManager::NewTaskManager(1);
        let task = Meta::new(3, TaskChannel::new(), 1);
        task.IncTask();
        task.IncTask();
        let exit = task.GetExitCh();
        manager.RegisterTask(task);

        manager.Downclock();
        assert!(exit.try_recv().unwrap());
        manager.Downclock();
    }
}
