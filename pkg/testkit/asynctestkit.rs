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

// 异步 TestKit：在独立工作线程上串行执行 SQL。
//
// 通过 mpsc 命令通道把 Exec/Query/Barrier 投递给持有 [`TestKit`] 的 worker，
// 便于并发测试场景下从多线程安全地驱动同一会话侧逻辑。

use std::sync::{Arc, mpsc};
use std::thread::{self, JoinHandle};

use crate::db_driver::{Database, DbValue};
use crate::result::Result;
use crate::testkit::TestKit;
use crate::{TestError, TestResult};

/// worker 线程处理的命令枚举。
enum Command {
    /// 执行写/DDL 类 SQL，结果经 reply 回传。
    Execute {
        sql: String,
        args: Vec<DbValue>,
        reply: mpsc::Sender<TestResult>,
    },
    /// 执行查询并回传字符串化行结果。
    Query {
        sql: String,
        args: Vec<DbValue>,
        reply: mpsc::Sender<TestResult<Result>>,
    },
    /// 屏障：worker 处理到此时回 ACK，用于同步。
    Barrier(mpsc::Sender<()>),
    /// 关闭 worker 循环。
    Close,
}

/// 异步测试工具包：命令发送端与可选的 worker JoinHandle。
pub struct AsyncTestKit {
    /// 向 worker 发送命令的通道。
    commands: mpsc::Sender<Command>,
    /// worker 线程句柄；Drop 时 join。
    worker: Option<JoinHandle<()>>,
}

impl AsyncTestKit {
    /// 创建 worker 线程并绑定到给定 [`Database`]。
    pub fn new(database: Arc<dyn Database>) -> Self {
        let (commands, receiver) = mpsc::channel();
        let worker = thread::Builder::new()
            .name("testkit-worker".to_owned())
            .spawn(move || {
                let mut testkit = TestKit::new(database);
                // 串行消费命令，保证同一 TestKit 不被并发访问。
                while let Ok(command) = receiver.recv() {
                    match command {
                        Command::Execute { sql, args, reply } => {
                            let result = testkit.Exec(&sql, args).map(|_| ());
                            let _ = reply.send(result);
                        }
                        Command::Query { sql, args, reply } => {
                            let result = testkit
                                .Query(&sql, args)
                                .map(|rows| Result::new(rows.string_rows()));
                            let _ = reply.send(result);
                        }
                        Command::Barrier(reply) => {
                            let _ = reply.send(());
                        }
                        Command::Close => break,
                    }
                }
                testkit
                    .Session()
                    .close()
                    .expect("close async testkit session");
            })
            .expect("spawn async testkit worker");
        Self {
            commands,
            worker: Some(worker),
        }
    }

    /// 异步执行 SQL，阻塞等待 worker 回传结果。
    pub fn Exec(&self, sql: &str, args: Vec<DbValue>) -> TestResult {
        let (reply, result) = mpsc::channel();
        self.commands
            .send(Command::Execute {
                sql: sql.to_owned(),
                args,
                reply,
            })
            .map_err(|_| TestError::new("async testkit worker stopped"))?;
        result
            .recv()
            .map_err(|_| TestError::new("async testkit worker dropped response"))?
    }

    /// 异步查询，返回 [`Result`] 行集包装。
    pub fn Query(&self, sql: &str, args: Vec<DbValue>) -> TestResult<Result> {
        let (reply, result) = mpsc::channel();
        self.commands
            .send(Command::Query {
                sql: sql.to_owned(),
                args,
                reply,
            })
            .map_err(|_| TestError::new("async testkit worker stopped"))?;
        result
            .recv()
            .map_err(|_| TestError::new("async testkit worker dropped response"))?
    }

    /// 执行失败则 panic。
    pub fn MustExec(&self, sql: &str, args: Vec<DbValue>) {
        self.Exec(sql, args)
            .unwrap_or_else(|error| panic!("sql={sql:?}: {error}"));
    }

    /// 查询失败则 panic。
    pub fn MustQuery(&self, sql: &str, args: Vec<DbValue>) -> Result {
        self.Query(sql, args)
            .unwrap_or_else(|error| panic!("sql={sql:?}: {error}"))
    }

    /// 发送屏障并等待 worker 处理完此前所有命令。
    pub fn Sync(&self) {
        let (reply, result) = mpsc::channel();
        self.commands
            .send(Command::Barrier(reply))
            .expect("async testkit worker stopped");
        result.recv().expect("async testkit barrier dropped");
    }
}

impl Drop for AsyncTestKit {
    fn drop(&mut self) {
        // 通知关闭并等待线程退出，避免泄漏。
        let _ = self.commands.send(Command::Close);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

/// 构造 [`AsyncTestKit`] 的 Go 风格命名入口。
pub fn NewAsyncTestKit(database: Arc<dyn Database>) -> AsyncTestKit {
    AsyncTestKit::new(database)
}
