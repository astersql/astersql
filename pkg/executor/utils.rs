// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
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

// 执行器通用工具。
//
// 包含集合字符串编解码、DML 子计划 Chunk 容量估算、批量取回辅助、
// 用户认证密码编码，以及轻量 worker 线程池。

#![allow(non_camel_case_types, non_snake_case)]

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, OnceLock};

use astersql_extension::AuthPlugin;
use astersql_parser_ast::UserSpec;
use astersql_parser_auth::caching_sha2::NewHashPassword;
use astersql_parser_auth::mysql_native_password::EncodePassword;
use astersql_parser_mysql::r#const::{
    AuthCachingSha2Password, AuthLDAPSASL, AuthLDAPSimple, AuthNativePassword, AuthSocket,
    AuthTiDBSM3Password, PWDHashLen, SHAPWDHashLen, SM3PWDHashLen,
};
use astersql_types::datum::FieldType;
use astersql_util_chunk::{self as chunk, Chunk};

/// Analyze 进度测试挂钩占位（对应 Go 测试注入点）。
pub static AnalyzeProgressTest: () = ();

/// 将逗号分隔字符串解析为集合；空串返回 None。
pub fn SetFromString(value: &str) -> Option<Vec<String>> {
    (!value.is_empty()).then(|| value.split(',').map(str::to_owned).collect())
}

/// 集合序列化为逗号分隔字符串。
pub fn setToString(set: &[String]) -> String {
    set.join(",")
}

/// 向集合追加元素（已存在则跳过，保序）。
pub fn addToSet(mut set: Vec<String>, value: String) -> Vec<String> {
    if !set.contains(&value) {
        set.push(value);
    }
    set
}

/// 从集合删除首个匹配元素。
pub fn deleteFromSet(mut set: Vec<String>, value: &str) -> Vec<String> {
    if let Some(index) = set.iter().position(|candidate| candidate == value) {
        set.remove(index);
    }
    set
}

/// DML 子计划 Chunk 目标字节数（约 256KiB），用于估算初始行容量。
pub const dmlChildChunkTargetBytes: usize = 256 * 1024;

/// 构造 DML 子 Chunk 所需的执行器能力。
pub trait DMLChildChunkExecutor {
    fn max_chunk_size(&self) -> usize;
    fn new_chunk_with_capacity(
        &self,
        fields: &[FieldType],
        initial_capacity: usize,
        maximum_capacity: usize,
    ) -> Chunk;
}

/// 按字段宽度与上限估算容量，再向执行器申请新 Chunk。
pub fn newDMLChildChunk(
    executor: &impl DMLChildChunkExecutor,
    fields: &[FieldType],
    maximum_initial_capacity: usize,
) -> Chunk {
    let maximum_chunk_size = executor.max_chunk_size();
    let initial_capacity =
        estimateDMLChildChunkInitCap(fields, maximum_chunk_size, maximum_initial_capacity);
    executor.new_chunk_with_capacity(fields, initial_capacity, maximum_chunk_size)
}

/// 估算 DML 子 Chunk 初始行数：目标字节 / 行宽，并受 max chunk 与调用方上限约束。
pub fn estimateDMLChildChunkInitCap(
    fields: &[FieldType],
    maximum_chunk_size: usize,
    maximum_initial_capacity: usize,
) -> usize {
    // 任一上限为 0 时退回零容量常量，避免除零或无意义分配。
    if maximum_chunk_size == 0 || maximum_initial_capacity == 0 {
        return chunk::ZeroCapacity;
    }
    // 按列类型估算单行字节宽度。
    let row_width = fields.iter().map(chunk::EstimateTypeWidth).sum::<usize>();
    if row_width == 0 {
        return maximum_chunk_size.min(maximum_initial_capacity);
    }
    1.max(
        maximum_chunk_size
            .min(maximum_initial_capacity)
            .min(dmlChildChunkTargetBytes / row_width),
    )
}

/// 按固定 batch_size 分批遍历 `[0, total_rows)` 的辅助状态机。
pub struct batchRetrieverHelper {
    pub retrieved: bool,
    pub retrieved_idx: usize,
    pub batch_size: usize,
    pub total_rows: usize,
}

impl batchRetrieverHelper {
    /// 取下一批 `[start, end)` 交给回调；出错或耗尽后标记 retrieved。
    pub fn nextBatch<E>(
        &mut self,
        mut retrieve_range: impl FnMut(usize, usize) -> Result<(), E>,
    ) -> Result<(), E> {
        if self.retrieved_idx >= self.total_rows {
            self.retrieved = true;
        }
        if self.retrieved {
            return Ok(());
        }
        let start = self.retrieved_idx;
        let end = (self.retrieved_idx + self.batch_size).min(self.total_rows);
        if let Err(error) = retrieve_range(start, end) {
            self.retrieved = true;
            return Err(error);
        }
        self.retrieved_idx = end;
        if self.retrieved_idx == self.total_rows {
            self.retrieved = true;
        }
        Ok(())
    }
}

/// 按注册认证插件或默认插件编码/校验用户密码哈希。
pub fn encodePasswordWithPlugin(
    user: &UserSpec,
    auth_plugin: Option<&AuthPlugin>,
    default_plugin: &str,
) -> (String, bool) {
    let Some(option) = user.AuthOpt.as_ref() else {
        return (String::new(), true);
    };
    if let Some(plugin) = auth_plugin {
        // 明文密码：按插件生成存储哈希。
        if option.ByAuthString {
            return plugin
                .GenerateAuthString
                .as_ref()
                .expect("registered auth plugin must have GenerateAuthString")(
                option.AuthString.clone(),
            );
        }
        let valid = plugin
            .ValidateAuthString
            .as_ref()
            .expect("registered auth plugin must have ValidateAuthString")(
            option.HashString.clone(),
        );
        return if valid {
            (option.HashString.clone(), true)
        } else {
            (String::new(), false)
        };
    }
    encodedPassword(user, default_plugin)
}

/// 内置插件路径：按 AuthString 生成哈希，或校验已有 HashString 长度格式。
pub fn encodedPassword(user: &UserSpec, default_plugin: &str) -> (String, bool) {
    let Some(option) = user.AuthOpt.as_ref() else {
        return (String::new(), true);
    };
    let auth_plugin = if option.AuthPlugin.is_empty() {
        default_plugin
    } else {
        &option.AuthPlugin
    };

    if option.ByAuthString {
        return match auth_plugin {
            AuthCachingSha2Password | AuthTiDBSM3Password => {
                (NewHashPassword(&option.AuthString, auth_plugin), true)
            }
            AuthSocket => (String::new(), true),
            _ => (EncodePassword(&option.AuthString), true),
        };
    }
    // LDAP 类插件直接透传 HashString。
    if matches!(auth_plugin, AuthLDAPSimple | AuthLDAPSASL) {
        return (option.HashString.clone(), true);
    }
    if option.HashString.is_empty() {
        return (String::new(), true);
    }

    // 已有哈希：校验长度与前缀是否符合插件规范。
    let valid = match auth_plugin {
        AuthCachingSha2Password => option.HashString.len() == SHAPWDHashLen,
        AuthTiDBSM3Password => option.HashString.len() == SM3PWDHashLen,
        "" | AuthNativePassword => {
            option.HashString.len() == PWDHashLen + 1 && option.HashString.starts_with('*')
        }
        AuthSocket => true,
        _ => false,
    };
    if valid {
        (option.HashString.clone(), true)
    } else {
        (String::new(), false)
    }
}

/// worker 池任务闭包类型。
type WorkerFn = Box<dyn FnOnce() + Send + 'static>;

/// 可回收的任务槽：执行后放回全局池复用 Box。
struct workerTask {
    function: Option<WorkerFn>,
}

/// 全局空闲任务槽池，减少反复分配。
fn global_task_pool() -> &'static Mutex<Vec<workerTask>> {
    static POOL: OnceLock<Mutex<Vec<workerTask>>> = OnceLock::new();
    POOL.get_or_init(|| Mutex::new(Vec::new()))
}

/// 单个 workerPool 的队列与在飞 worker 计数。
struct WorkerPoolState {
    queue: VecDeque<workerTask>,
    workers: u32,
}

/// 轻量任务池：按需起线程，完成后将任务槽归还全局池。
pub struct workerPool {
    state: Arc<Mutex<WorkerPoolState>>,
    need_spawn: Option<Arc<dyn Fn(u32, u32) -> bool + Send + Sync>>,
}

impl workerPool {
    /// 创建线程池；`need_spawn` 决定是否在已有 worker 时再扩容。
    pub fn new(need_spawn: Option<Arc<dyn Fn(u32, u32) -> bool + Send + Sync>>) -> Self {
        Self {
            state: Arc::new(Mutex::new(WorkerPoolState {
                queue: VecDeque::new(),
                workers: 0,
            })),
            need_spawn,
        }
    }

    /// 提交任务：复用槽位入队，必要时 spawn 新 worker。
    pub fn submit(&self, function: impl FnOnce() + Send + 'static) {
        let mut task = global_task_pool()
            .lock()
            .expect("global executor task pool poisoned")
            .pop()
            .unwrap_or(workerTask { function: None });
        task.function = Some(Box::new(function));

        let spawn = {
            let mut state = self.state.lock().expect("executor worker pool poisoned");
            state.queue.push_back(task);
            let tasks = state.queue.len() as u32;
            let spawn = state.workers == 0
                || self.need_spawn.is_none()
                || self
                    .need_spawn
                    .as_ref()
                    .is_some_and(|predicate| predicate(state.workers, tasks));
            if spawn {
                state.workers += 1;
            }
            spawn
        };
        if spawn {
            let state = Arc::clone(&self.state);
            std::thread::spawn(move || run_worker(state));
        }
    }
}

/// worker 主循环：取任务执行，队列空则退出并减少 workers 计数。
fn run_worker(state: Arc<Mutex<WorkerPoolState>>) {
    loop {
        let task = {
            let mut state = state.lock().expect("executor worker pool poisoned");
            let Some(task) = state.queue.pop_front() else {
                state.workers -= 1;
                return;
            };
            task
        };
        let mut task = task;
        if let Some(function) = task.function.take() {
            function();
        }
        global_task_pool()
            .lock()
            .expect("global executor task pool poisoned")
            .push(task);
    }
}

#[inline(never)]
/// 通过栈上大数组促使运行时扩栈（对应 Go growWorkerStack 测试/兼容钩子）。
pub fn growWorkerStack16K() {
    let data = [0_u8; 8192];
    std::hint::black_box(&data);
}
