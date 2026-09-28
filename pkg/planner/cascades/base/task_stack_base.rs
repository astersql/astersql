// Copyright 2024 PingCAP, Inc.
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

// Cascades 优化任务（Task）及其栈容器（Stack）抽象。
//
// Task 表示探索、物理实现、统计派生、连接重排等可调度工作单元；
// Stack 以 LIFO（后进先出）方式存放待执行任务，由 Scheduler 驱动弹出执行。

// 本文件由 pkg/planner/cascades/base/task_stack_base.go 迁移而来，保留 Go 接口顺序。
// 本文件描述优化任务及其栈容器的抽象契约。

// Stack 对应 Go 的任务容器抽象；具体的 TaskStack 会以数组栈实现这些操作。
// Box<dyn Task> 表达 Go 接口值的动态分发和所有权转移，弹栈时再把任务所有权交还调用方。
pub trait Stack {
    // Push 对应 Go Push：把一个待执行任务压到栈顶。
    fn Push(&mut self, one: Box<dyn Task>);

    // Pop 对应 Go Pop：移除并返回栈顶任务；空栈时 Go 实现返回 nil。
    fn Pop(&mut self) -> Option<Box<dyn Task>>;

    // Empty 对应 Go Empty：判断容器当前是否没有任务。
    fn Empty(&self) -> bool;

    // Destroy 对应 Go Destroy：释放或重置栈内部持有的任务资源。
    fn Destroy(&mut self);
}

// Task 对应所有优化工作共有的 Go 接口，包括探索、物理实现、统计派生和连接重排等任务。
pub trait Task {
    // Execute 对应任务自身的执行逻辑；错误类型留作跨文件接线时映射 Go error。
    fn Execute(&mut self) -> Result<(), Box<dyn std::error::Error>>;

    // Desc 对应任务描述输出；写入器由调用方提供，方法本身不执行外部 IO。
    fn Desc(&self, w: &mut dyn util::StrBufferWriter);
}
