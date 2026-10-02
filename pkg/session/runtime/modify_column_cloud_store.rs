// Copyright 2026 AsterSQL.

//! Object-store transport shared by the native DDL sorting stages. Each worker
//! opens its own range reader/upload, while cancellation follows the subtask.
use astersql_ingestor_globalsort as sort;
use astersql_objstore as legacy;
use astersql_objstore_s3like as s3;
use astersql_objstore_storeapi as api;
use std::io::{Read, Write};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

fn failure(error: impl std::fmt::Display) -> sort::Error {
    sort::Error::InvalidData(error.to_string())
}
fn io_failure(error: impl std::fmt::Display) -> std::io::Error {
    std::io::Error::other(error.to_string())
}
enum Transport {
    Local(legacy::storage::StorageRef),
    Cloud(Arc<dyn api::Storage>),
}
pub(super) struct CloudStore {
    transport: Transport,
    local_context: legacy::storage::Context,
    context: api::Context,
}
impl CloudStore {
    pub(super) fn open(uri: &str, cancelled: Arc<AtomicBool>) -> Result<Arc<Self>, String> {
        let local_context = legacy::storage::Context::from_cancellation_flag(cancelled.clone());
        let context = api::Context::from_cancellation_flag(cancelled);
        let backend = legacy::parse::ParseBackend(uri, None).map_err(|error| error.to_string())?;
        let transport = match backend {
            legacy::parse::StorageBackend::S3(value) => {
                let mut backend = s3::backuppb::S3 {
                    Endpoint: value.endpoint,
                    Region: value.region,
                    Bucket: value.bucket,
                    Prefix: value.prefix,
                    StorageClass: value.storage_class,
                    Sse: value.sse,
                    SseKmsKeyId: value.sse_kms_key_id,
                    Acl: value.acl,
                    AccessKey: value.access_key,
                    SecretAccessKey: value.secret_access_key,
                    SessionToken: value.session_token,
                    ForcePathStyle: value.force_path_style,
                    RoleArn: value.role_arn,
                    ExternalId: value.external_id,
                    Provider: value.provider,
                    Profile: value.profile,
                    ..Default::default()
                };
                Transport::Cloud(Arc::new(
                    astersql_objstore_s3store::NewS3Storage(
                        &context,
                        &mut backend,
                        &api::Options::default(),
                    )
                    .map_err(|error| error.to_string())?,
                ))
            }
            backend => Transport::Local(
                legacy::storage::NewWithDefaultOpt(&local_context, &backend)
                    .map_err(|error| error.to_string())?,
            ),
        };
        Ok(Arc::new(Self {
            transport,
            local_context,
            context,
        }))
    }
}

enum Reader {
    Local(
        Box<dyn legacy::storage::ObjectReader>,
        legacy::storage::Context,
    ),
    Cloud(Box<dyn s3::objectio::Reader>, api::Context),
}
impl Read for Reader {
    fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
        match self {
            Self::Local(reader, context) => {
                context
                    .check_cancelled()
                    .map_err(|_| std::io::Error::other(sort::Error::Cancelled))?;
                reader.read(bytes)
            }
            Self::Cloud(reader, context) => {
                context
                    .check()
                    .map_err(|_| std::io::Error::other(sort::Error::Cancelled))?;
                reader.read(bytes).map_err(|error| {
                    if context.is_cancelled() {
                        std::io::Error::other(sort::Error::Cancelled)
                    } else {
                        error
                    }
                })
            }
        }
    }
}
impl Drop for Reader {
    fn drop(&mut self) {
        let result = match self {
            Self::Local(reader, _) => reader.close().map_err(io_failure),
            Self::Cloud(reader, _) => reader.close(),
        };
        if let Err(error) = result {
            eprintln!("DDL cloud reader close failed: {error}");
        }
    }
}
enum Upload {
    Local(
        Box<dyn legacy::storage::ObjectWriter>,
        legacy::storage::Context,
    ),
    Cloud(Box<dyn s3::objectio::Writer>, api::Context),
}
struct Writer(Option<Upload>);
impl Writer {
    fn close(&mut self) -> sort::Result<()> {
        match self.0.take() {
            Some(Upload::Local(mut writer, context)) => writer.close(&context).map_err(failure),
            Some(Upload::Cloud(mut writer, context)) => writer.close(&context).map_err(failure),
            None => Ok(()),
        }
    }
}
impl Write for Writer {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        match self
            .0
            .as_mut()
            .ok_or_else(|| std::io::Error::other("DDL cloud upload closed"))?
        {
            Upload::Local(writer, context) => {
                context
                    .check_cancelled()
                    .map_err(|_| std::io::Error::other(sort::Error::Cancelled))?;
                writer.write(context, bytes).map_err(io_failure)
            }
            Upload::Cloud(writer, context) => {
                context
                    .check()
                    .map_err(|_| std::io::Error::other(sort::Error::Cancelled))?;
                writer.write(context, bytes)
            }
        }
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
impl sort::ObjectWriter for Writer {
    fn finish(mut self: Box<Self>) -> sort::Result<()> {
        self.close()
    }
}
impl Drop for Writer {
    fn drop(&mut self) {
        // Go defers Close even when merging fails. Preserve the primary error;
        // durable cloud-task cleanup owns removal of incomplete task objects.
        if let Err(error) = self.close() {
            eprintln!("DDL cloud upload close failed: {error}");
        }
    }
}
impl sort::Storage for CloudStore {
    fn file_size(&self, path: &str) -> sort::Result<u64> {
        self.context.check().map_err(|_| sort::Error::Cancelled)?;
        let reader = match &self.transport {
            Transport::Local(store) => Reader::Local(
                store
                    .Open(&self.local_context, path, None)
                    .map_err(failure)?,
                self.local_context.clone(),
            ),
            Transport::Cloud(store) => Reader::Cloud(
                store.Open(&self.context, path, None).map_err(failure)?,
                self.context.clone(),
            ),
        };
        let size = match &reader {
            Reader::Local(reader, _) => reader.get_file_size().map_err(failure)?,
            Reader::Cloud(reader, _) => reader.file_size().map_err(failure)?,
        };
        u64::try_from(size).map_err(failure)
    }

    fn record_format(&self) -> sort::RecordFormat {
        sort::RecordFormat::GoBigEndian64
    }
    fn open(&self, path: &str) -> sort::Result<Box<dyn Read>> {
        self.open_at(path, 0)
    }
    fn open_at(&self, path: &str, offset: u64) -> sort::Result<Box<dyn Read>> {
        self.context.check().map_err(|_| sort::Error::Cancelled)?;
        let offset = i64::try_from(offset).map_err(failure)?;
        let reader = match &self.transport {
            Transport::Local(store) => Reader::Local(
                store
                    .Open(
                        &self.local_context,
                        path,
                        Some(&legacy::storage::ReaderOption {
                            start_offset: Some(offset),
                            end_offset: None,
                        }),
                    )
                    .map_err(failure)?,
                self.local_context.clone(),
            ),
            Transport::Cloud(store) => Reader::Cloud(
                store
                    .Open(
                        &self.context,
                        path,
                        Some(&api::ReaderOption {
                            StartOffset: Some(offset),
                            ..Default::default()
                        }),
                    )
                    .map_err(failure)?,
                self.context.clone(),
            ),
        };
        Ok(Box::new(reader))
    }
    fn create(&self, path: &str) -> sort::Result<Box<dyn sort::ObjectWriter + '_>> {
        self.context.check().map_err(|_| sort::Error::Cancelled)?;
        let writer = match &self.transport {
            Transport::Local(store) => Upload::Local(
                store
                    .Create(&self.local_context, path, None)
                    .map_err(failure)?,
                self.local_context.clone(),
            ),
            Transport::Cloud(store) => Upload::Cloud(
                store.Create(&self.context, path, None).map_err(failure)?,
                self.context.clone(),
            ),
        };
        Ok(Box::new(Writer(Some(writer))))
    }
    fn read(&self, path: &str) -> sort::Result<Vec<u8>> {
        self.context.check().map_err(|_| sort::Error::Cancelled)?;
        match &self.transport {
            Transport::Local(store) => store.ReadFile(&self.local_context, path).map_err(failure),
            Transport::Cloud(store) => store.ReadFile(&self.context, path).map_err(failure),
        }
    }
    fn write(&self, path: &str, bytes: Vec<u8>) -> sort::Result<()> {
        self.context.check().map_err(|_| sort::Error::Cancelled)?;
        match &self.transport {
            Transport::Local(store) => store
                .WriteFile(&self.local_context, path, &bytes)
                .map_err(failure),
            Transport::Cloud(store) => store
                .WriteFile(&self.context, path, &bytes)
                .map_err(failure),
        }
    }
    fn delete_files(&self, paths: &[String]) -> sort::Result<()> {
        self.context.check().map_err(|_| sort::Error::Cancelled)?;
        match &self.transport {
            Transport::Local(store) => store
                .DeleteFiles(&self.local_context, paths)
                .map_err(failure),
            Transport::Cloud(store) => store.DeleteFiles(&self.context, paths).map_err(failure),
        }
    }
    fn list_prefix(&self, prefix: &str) -> sort::Result<Vec<String>> {
        self.context.check().map_err(|_| sort::Error::Cancelled)?;
        let mut paths = Vec::new();
        match &self.transport {
            Transport::Local(store) => store
                .WalkDir(
                    &self.local_context,
                    Some(&legacy::storage::WalkOption {
                        obj_prefix: prefix.into(),
                        ..Default::default()
                    }),
                    &mut |path, _| {
                        paths.push(path.into());
                        Ok(())
                    },
                )
                .map_err(failure)?,
            Transport::Cloud(store) => store
                .WalkDir(
                    &self.context,
                    Some(&api::WalkOption {
                        ObjPrefix: prefix.into(),
                        ..Default::default()
                    }),
                    &mut |path, _| {
                        paths.push(path.into());
                        Ok(())
                    },
                )
                .map_err(failure)?,
        }
        Ok(paths)
    }
}
impl astersql_ingestor_simplesst::writer::WriterSink for CloudStore {
    fn write_file(&self, path: &str, bytes: &[u8]) -> Result<(), String> {
        use sort::Storage;
        // Each flush is already bounded by the simplesst writer's memory budget.
        let mut writer = self.create(path).map_err(|error| error.to_string())?;
        writer.write_all(bytes).map_err(|error| error.to_string())?;
        writer.finish().map_err(|error| error.to_string())
    }
}
