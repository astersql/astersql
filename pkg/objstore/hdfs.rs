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

// HDFS 对象存储后端：经 `hdfs dfs` 子进程完成写入与存在性检查，其余操作为 rawkv 备份受限。

use std::any::Any;
use std::ffi::OsString;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration;

use anyhow::{Context as AnyhowContext, Result, bail};

use crate::{objectio, storeapi};

/// HDFS 远程根路径封装（如 `hdfs://nn/user/backup`）。
#[derive(Clone, Debug)]
pub struct HDFSStorage {
    remote: String,
}

impl HDFSStorage {
    /// 用远程 URI/路径构造存储。
    pub fn new(remote: impl Into<String>) -> Self {
        Self {
            remote: remote.into(),
        }
    }

    /// 返回远程根路径。
    pub fn remote(&self) -> &str {
        &self.remote
    }

    /// 拼接远程根与逻辑文件名。
    fn file_path(&self, name: &str) -> String {
        format!("{}/{}", self.remote, name)
    }

    fn write_file(&self, name: &str, data: &[u8]) -> Result<()> {
        let path = self.file_path(name);
        let mut command = dfs_command(&["-put", "-", &path])?;
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command.spawn().context("failed to start hdfs dfs -put")?;
        child
            .stdin
            .take()
            .ok_or_else(|| anyhow::anyhow!("failed to open hdfs stdin"))?
            .write_all(data)?;
        let output = child.wait_with_output()?;
        if !output.status.success() {
            let mut combined = output.stdout;
            combined.extend_from_slice(&output.stderr);
            let message = String::from_utf8_lossy(&combined);
            bail!("{message}: {}", output.status);
        }
        Ok(())
    }

    fn file_exists(&self, name: &str) -> Result<bool> {
        let path = self.file_path(name);
        let output = dfs_command(&["-ls", &path])?
            .output()
            .context("failed to start hdfs dfs -ls")?;
        Ok(output.status.success())
    }
}

/// Go 风格构造函数名。
#[allow(non_snake_case)]
pub fn NewHDFSStorage(remote: String) -> HDFSStorage {
    HDFSStorage::new(remote)
}

/// 从 `HADOOP_HOME` 解析 `bin/hdfs` 可执行文件路径。
pub fn get_hdfs_bin() -> Result<PathBuf> {
    let home = std::env::var_os("HADOOP_HOME")
        .ok_or_else(|| anyhow::anyhow!("please specify environment variable HADOOP_HOME"))?;
    Ok(PathBuf::from(home).join("bin/hdfs"))
}

/// 可选：`HADOOP_LINUX_USER`，有则经 `sudo -u` 切换用户执行 hdfs。
pub fn get_linux_user() -> Option<OsString> {
    std::env::var_os("HADOOP_LINUX_USER")
}

/// 待启动的外部命令规格（程序 + 参数列表）。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CommandSpec {
    pub program: PathBuf,
    pub args: Vec<String>,
}

impl CommandSpec {
    /// 转为可 spawn 的 `std::process::Command`。
    fn command(&self) -> Command {
        let mut command = Command::new(&self.program);
        command.args(&self.args);
        command
    }
}

/// 构造 `hdfs dfs …`（或 `sudo -u USER hdfs dfs …`）命令规格。
pub fn dfs_command_spec(args: &[&str]) -> Result<CommandSpec> {
    let hdfs = get_hdfs_bin()?;
    let mut command_args = Vec::new();
    let program = if let Some(user) = get_linux_user() {
        command_args.push("-u".to_owned());
        command_args.push(user.to_string_lossy().into_owned());
        command_args.push(hdfs.to_string_lossy().into_owned());
        PathBuf::from("sudo")
    } else {
        hdfs
    };
    command_args.push("dfs".to_owned());
    command_args.extend(args.iter().map(|argument| (*argument).to_owned()));
    Ok(CommandSpec {
        program,
        args: command_args,
    })
}

/// 便捷：直接得到可执行的 `Command`。
pub fn dfs_command(args: &[&str]) -> Result<Command> {
    Ok(dfs_command_spec(args)?.command())
}

/// 当前 HDFS 后端仅支持 rawkv 备份相关能力时的统一错误。
fn unsupported_hdfs_operation() -> anyhow::Error {
    anyhow::anyhow!("currently HDFS backend only support rawkv backup")
}

impl storeapi::Storage for HDFSStorage {
    fn WriteFile(&self, _ctx: &objectio::Context, name: &str, data: &[u8]) -> Result<()> {
        self.write_file(name, data)
    }

    fn ReadFile(&self, _ctx: &objectio::Context, _name: &str) -> Result<Vec<u8>> {
        Err(unsupported_hdfs_operation())
    }

    fn FileExists(&self, _ctx: &objectio::Context, name: &str) -> Result<bool> {
        self.file_exists(name)
    }

    fn DeleteFile(&self, _ctx: &objectio::Context, _name: &str) -> Result<()> {
        Err(unsupported_hdfs_operation())
    }

    fn Open(
        &self,
        _ctx: &objectio::Context,
        _path: &str,
        _option: Option<&storeapi::ReaderOption>,
    ) -> Result<Box<dyn objectio::Reader>> {
        Err(unsupported_hdfs_operation())
    }

    fn DeleteFiles(&self, _ctx: &objectio::Context, _names: &[String]) -> Result<()> {
        Err(unsupported_hdfs_operation())
    }

    fn WalkDir(
        &self,
        _ctx: &objectio::Context,
        _option: Option<&storeapi::WalkOption>,
        _callback: &mut dyn FnMut(&str, i64) -> Result<()>,
    ) -> Result<()> {
        Err(unsupported_hdfs_operation())
    }

    fn URI(&self) -> String {
        self.remote.clone()
    }

    fn Create(
        &self,
        _ctx: &objectio::Context,
        _path: &str,
        _option: Option<&storeapi::WriterOption>,
    ) -> Result<Box<dyn objectio::Writer>> {
        Err(unsupported_hdfs_operation())
    }

    fn Rename(
        &self,
        _ctx: &objectio::Context,
        _old_file_name: &str,
        _new_file_name: &str,
    ) -> Result<()> {
        Err(unsupported_hdfs_operation())
    }

    fn PresignFile(
        &self,
        _ctx: &objectio::Context,
        _file_name: &str,
        _expire: Duration,
    ) -> Result<String> {
        bail!("HDFS backend does not support PresignFile")
    }

    fn Close(&self) {}
}

impl crate::storage::Storage for HDFSStorage {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn WriteFile(&self, _ctx: &crate::storage::Context, name: &str, data: &[u8]) -> Result<()> {
        self.write_file(name, data)
    }

    fn ReadFile(&self, _ctx: &crate::storage::Context, _name: &str) -> Result<Vec<u8>> {
        Err(unsupported_hdfs_operation())
    }

    fn FileExists(&self, _ctx: &crate::storage::Context, name: &str) -> Result<bool> {
        self.file_exists(name)
    }

    fn DeleteFile(&self, _ctx: &crate::storage::Context, _name: &str) -> Result<()> {
        Err(unsupported_hdfs_operation())
    }

    fn DeleteFiles(&self, _ctx: &crate::storage::Context, _names: &[String]) -> Result<()> {
        Err(unsupported_hdfs_operation())
    }

    fn Open(
        &self,
        _ctx: &crate::storage::Context,
        _name: &str,
        _option: Option<&crate::storage::ReaderOption>,
    ) -> Result<Box<dyn crate::storage::ObjectReader>> {
        Err(unsupported_hdfs_operation())
    }

    fn WalkDir(
        &self,
        _ctx: &crate::storage::Context,
        _option: Option<&crate::storage::WalkOption>,
        _callback: &mut dyn FnMut(&str, i64) -> Result<()>,
    ) -> Result<()> {
        Err(unsupported_hdfs_operation())
    }

    fn URI(&self) -> String {
        self.remote.clone()
    }

    fn Create(
        &self,
        _ctx: &crate::storage::Context,
        _name: &str,
        _option: Option<&crate::storage::WriterOption>,
    ) -> Result<Box<dyn crate::storage::ObjectWriter>> {
        Err(unsupported_hdfs_operation())
    }

    fn Rename(
        &self,
        _ctx: &crate::storage::Context,
        _old_name: &str,
        _new_name: &str,
    ) -> Result<()> {
        Err(unsupported_hdfs_operation())
    }

    fn PresignFile(
        &self,
        _ctx: &crate::storage::Context,
        _name: &str,
        _duration: Duration,
    ) -> Result<String> {
        bail!("HDFS backend does not support PresignFile")
    }

    fn Close(&self) {}
}
