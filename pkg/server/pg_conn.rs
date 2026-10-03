// Copyright 2026 AsterSQL.
//! Independent PostgreSQL 3.0/3.2 negotiation and connection lifecycle.
//! The canonical driver currently supports only insecure root with an empty
//! password. No trust fallback or PostgreSQL-specific execution semantics exist.
use crate::conn::{
    AUTH_NATIVE_PASSWORD, AuthIdentity, AuthRequest, CancellationToken, ConnectionDomain,
    SessionDriver, TiDBContext,
};
use crate::pg_protocol::{MAX_STARTUP_LENGTH, PROTOCOL_VERSION_30, parse_startup};
use std::collections::{BTreeMap, HashMap};
use std::io::{self, Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::thread::{self, JoinHandle};
use std::time::Duration;

const SSL_REQUEST: u32 = 80877103;
const GSS_REQUEST: u32 = 80877104;
const CANCEL_REQUEST: u32 = 80877102;
const MAX_MESSAGE: usize = 1 << 20;

// The target backend's startup version determines the cancellation format;
// CancelRequest itself carries no protocol version.
fn cancel_key_length(protocol_version: u32) -> usize {
    if protocol_version == PROTOCOL_VERSION_30 {
        4
    } else {
        32
    }
}

struct Active {
    protocol_version: u32,
    key: Vec<u8>,
    context: Arc<dyn TiDBContext>,
    // Cancellation is accepted only during a command. An idle cancellation
    // must not poison the next command in the canonical context.
    executing: bool,
}
#[derive(Default)]
pub struct PgService {
    startup_epoch_micros: u128,
    stopped: AtomicBool,
    accept: Mutex<Option<JoinHandle<()>>>,
    workers: Mutex<Vec<JoinHandle<()>>>,
    sockets: Mutex<HashMap<u64, TcpStream>>,
    active: Mutex<HashMap<u32, Active>>,
}
impl PgService {
    #[cfg(test)]
    pub(crate) fn resource_counts(&self) -> (usize, usize, usize) {
        (
            self.active.lock().unwrap().len(),
            self.sockets.lock().unwrap().len(),
            self.workers.lock().unwrap().len(),
        )
    }

    pub fn start(
        listener: TcpListener,
        driver: Arc<dyn SessionDriver>,
        domain: Arc<dyn ConnectionDomain>,
        require_secure_transport: bool,
    ) -> io::Result<Arc<Self>> {
        listener.set_nonblocking(true)?;
        let service = Arc::new(Self {
            startup_epoch_micros: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_err(io::Error::other)?
                .as_micros(),
            ..Self::default()
        });
        let owner = service.clone();
        let worker = thread::Builder::new()
            .name("astersql-pg-accept".into())
            .spawn(move || {
                while !owner.stopped.load(Ordering::Acquire) {
                    match listener.accept() {
                        Ok((mut socket, _)) => {
                            let id = domain.next_connection_id();
                            let Ok(control) = socket.try_clone() else {
                                domain.release_connection_id(id);
                                continue;
                            };
                            owner.sockets.lock().unwrap().insert(id, control);
                            let connection_owner = owner.clone();
                            let driver = driver.clone();
                            let connection_domain = domain.clone();
                            let result = thread::Builder::new()
                                .name(format!("astersql-pg-{id}"))
                                .spawn(move || {
                                    let _ = socket.set_nonblocking(false);
                                    let _ = socket.set_read_timeout(Some(Duration::from_secs(10)));
                                    let _ = socket.set_write_timeout(Some(Duration::from_secs(10)));
                                    let result = connection_owner.negotiate(
                                        &mut socket,
                                        id,
                                        driver.as_ref(),
                                        require_secure_transport,
                                    );
                                    if let Err(error) = result {
                                        let _ = write_error(
                                            &mut socket,
                                            "FATAL",
                                            "08P01",
                                            &error.to_string(),
                                        );
                                    }
                                    if let Ok(pid) = u32::try_from(id) {
                                        if let Some(active) =
                                            connection_owner.active.lock().unwrap().remove(&pid)
                                        {
                                            let _ = active.context.close();
                                        }
                                    }
                                    let _ = socket.shutdown(Shutdown::Both);
                                    connection_owner.sockets.lock().unwrap().remove(&id);
                                    connection_domain.release_connection_id(id);
                                });
                            match result {
                                Ok(worker) => {
                                    let mut workers = owner.workers.lock().unwrap();
                                    let mut i = 0;
                                    while i < workers.len() {
                                        if workers[i].is_finished() {
                                            let _ = workers.swap_remove(i).join();
                                        } else {
                                            i += 1;
                                        }
                                    }
                                    workers.push(worker);
                                }
                                Err(_) => {
                                    owner.sockets.lock().unwrap().remove(&id);
                                    domain.release_connection_id(id);
                                }
                            }
                        }
                        Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                            thread::sleep(Duration::from_millis(10))
                        }
                        Err(_) => break,
                    }
                }
            })?;
        *service.accept.lock().unwrap() = Some(worker);
        Ok(service)
    }
    pub fn close(&self) {
        if self.stopped.swap(true, Ordering::AcqRel) {
            return;
        }
        if let Some(worker) = self.accept.lock().unwrap().take() {
            let _ = worker.join();
        }
        for active in self.active.lock().unwrap().values() {
            active.context.cancel();
        }
        for socket in self.sockets.lock().unwrap().values() {
            let _ = socket.shutdown(Shutdown::Both);
        }
        for worker in std::mem::take(&mut *self.workers.lock().unwrap()) {
            let _ = worker.join();
        }
    }
    fn negotiate(
        &self,
        socket: &mut TcpStream,
        id: u64,
        driver: &dyn SessionDriver,
        secure: bool,
    ) -> io::Result<()> {
        // One negotiation request per mechanism; repeated negotiation cannot
        // keep an unauthenticated worker alive indefinitely.
        let mut ssl = false;
        let mut gss = false;
        let startup = loop {
            let packet = read_initial(socket)?;
            let code = u32::from_be_bytes(packet[4..8].try_into().unwrap());
            match code {
                SSL_REQUEST | GSS_REQUEST if packet.len() == 8 => {
                    let seen = if code == SSL_REQUEST {
                        &mut ssl
                    } else {
                        &mut gss
                    };
                    if *seen {
                        return Err(invalid("repeated encryption request"));
                    }
                    *seen = true;
                    socket.write_all(b"N")?;
                }
                CANCEL_REQUEST => {
                    if !(16..=268).contains(&packet.len()) {
                        return Err(invalid("invalid cancel key length"));
                    }
                    let pid = u32::from_be_bytes(packet[8..12].try_into().unwrap());
                    self.cancel(pid, &packet[12..]);
                    return Ok(());
                }
                _ => match parse_startup(&packet) {
                    Ok(startup) => break startup,
                    Err(error) => {
                        write_error(socket, "FATAL", "0A000", &error.to_string())?;
                        return Ok(());
                    }
                },
            }
        };
        if secure {
            write_error(
                socket,
                "FATAL",
                "08004",
                "PostgreSQL TLS is not supported; secure transport is required",
            )?;
            return Ok(());
        }
        let Some(user) = startup.parameters.get("user").filter(|s| !s.is_empty()) else {
            return Err(invalid("startup user is required"));
        };
        for (name, value) in &startup.parameters {
            let supported = matches!(name.as_str(), "user" | "database" | "application_name")
                || (name == "client_encoding"
                    && matches!(value.to_ascii_uppercase().as_str(), "UTF8" | "UTF-8"))
                // The text codec uses ISO dates. JDBC requests ISO at startup;
                // reject alternate formats rather than advertising unsupported output.
                || (name.eq_ignore_ascii_case("datestyle")
                    && matches!(value.to_ascii_uppercase().as_str(), "ISO" | "ISO, MDY"))
                // Positive extra_float_digits requests shortest round-trip output,
                // which is already the native float text encoding.
                || (name == "extra_float_digits" && matches!(value.as_str(), "1" | "2" | "3"))
                || (name.eq_ignore_ascii_case("timezone")
                    && !value.is_empty()
                    && value.bytes().all(|byte| {
                        byte.is_ascii_alphanumeric() || b"/_+-:.".contains(&byte)
                    }));
            if !supported {
                write_error(
                    socket,
                    "FATAL",
                    "0A000",
                    &format!("unsupported startup parameter {name}"),
                )?;
                return Ok(());
            }
        }
        let pid =
            u32::try_from(id).map_err(|_| invalid("connection id exceeds PostgreSQL width"))?;
        // Open without a database, then select it only after authentication.
        let context = match driver.open_ctx(id, 0, 45, "", None) {
            Ok(c) => c,
            Err(e) => {
                write_error(socket, "FATAL", "08006", &e.to_string())?;
                return Ok(());
            }
        };
        let result = (|| {
            let plugin = context
                .auth_plugin_for_user(user, &socket.peer_addr()?.ip().to_string())
                .map_err(|e| invalid(&e.to_string()))?;
            if plugin != AUTH_NATIVE_PASSWORD {
                write_error(
                    socket,
                    "FATAL",
                    "0A000",
                    "unsupported authentication plugin",
                )?;
                return Ok(false);
            }
            // The canonical driver explicitly validates empty native credentials.
            // libpq refuses an empty PasswordMessage after a password challenge;
            // authenticate first and emit AuthenticationOk only on real success.
            // SecureUnsupported and non-root identities still fail in the driver.
            let request = AuthRequest {
                identity: AuthIdentity {
                    username: user.clone(),
                    hostname: socket.peer_addr()?.ip().to_string(),
                    plugin,
                },
                auth_data: Vec::new(),
                salt: Vec::new(),
                tls_state: None,
                attributes: BTreeMap::new(),
            };
            if let Err(e) = context.authenticate(&request) {
                write_error(socket, "FATAL", "28P01", &e.to_string())?;
                return Ok(false);
            }
            if let Some(db) = startup.parameters.get("database").filter(|s| !s.is_empty()) {
                if let Err(e) = context.use_db(db, &CancellationToken::new()) {
                    write_error(socket, "FATAL", "3D000", &e.to_string())?;
                    return Ok(false);
                }
            }
            let time_zone = startup
                .parameters
                .iter()
                .find(|(name, _)| name.eq_ignore_ascii_case("timezone"))
                .map_or("UTC", |(_, value)| value.as_str());
            // Startup values above are restricted to timezone name characters,
            // so this literal cannot introduce SQL or alter session sql_mode.
            if let Err(error) = context.execute_query(
                &format!("SET time_zone = '{time_zone}'"),
                false,
                &CancellationToken::new(),
            ) {
                write_error(socket, "FATAL", "22023", &error.to_string())?;
                return Ok(false);
            }
            let key_length = cancel_key_length(startup.protocol_version);
            let mut key = vec![0; key_length];
            rustls::crypto::aws_lc_rs::default_provider()
                .secure_random
                .fill(&mut key)
                .map_err(|_| invalid("secure random unavailable"))?;
            self.active.lock().unwrap().insert(
                pid,
                Active {
                    protocol_version: startup.protocol_version,
                    key: key.clone(),
                    context: context.clone(),
                    executing: false,
                },
            );
            write_message(socket, b'R', &0u32.to_be_bytes())?;
            // PG protocol compatibility baseline; retain the product identity.
            for (name, value) in [
                ("client_encoding", "UTF8"),
                ("server_encoding", "UTF8"),
                ("DateStyle", "ISO, MDY"),
                ("TimeZone", time_zone),
                ("server_version", "18.0 (AsterSQL)"),
            ] {
                let mut body = name.as_bytes().to_vec();
                body.push(0);
                body.extend_from_slice(value.as_bytes());
                body.push(0);
                write_message(socket, b'S', &body)?;
            }
            let mut body = pid.to_be_bytes().to_vec();
            body.extend_from_slice(&key);
            write_message(socket, b'K', &body)?;
            write_message(
                socket,
                b'Z',
                if context.in_transaction() { b"T" } else { b"I" },
            )?;
            socket.set_read_timeout(Some(context.wait_timeout()))?;
            let mut extended = crate::pg_extended::Extended::new(self.startup_epoch_micros);
            loop {
                let (tag, body) = match read_message(socket) {
                    Ok(m) => m,
                    Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(true),
                    Err(e) => return Err(e),
                };
                if tag == b'X' && body.is_empty() {
                    return Ok(true);
                }
                if extended.handle(tag, &body, socket, &context, |statement, args, catalog| {
                    self.with_query(pid, |context| {
                        if let Some(catalog) = catalog {
                            return catalog.execute(context.as_ref());
                        }
                        context.execute_prepared_statement(
                            statement,
                            args,
                            &CancellationToken::new(),
                        )
                    })
                })? {
                    continue;
                }
                if tag == b'Q' {
                    extended.reset_unnamed(&context);
                    let sql = body
                        .strip_suffix(&[0])
                        .filter(|sql| !sql.contains(&0))
                        .and_then(|sql| std::str::from_utf8(sql).ok());
                    if let Some(sql) = sql {
                        let session_query = crate::pg_session::SessionQuery::parse(sql);
                        if let Ok(Some(query)) = &session_query {
                            match self.with_query(pid, |context| {
                                extended.session.execute(query, context.as_ref())
                            })? {
                                Ok(result) => crate::pg_result::write_result(
                                    socket,
                                    &result,
                                    query.command(),
                                )?,
                                Err(error) => write_error(
                                    socket,
                                    "ERROR",
                                    sqlstate(&error),
                                    &error.to_string(),
                                )?,
                            }
                            write_message(
                                socket,
                                b'Z',
                                if context.in_transaction() { b"T" } else { b"I" },
                            )?;
                            continue;
                        }
                        if let Err((state, message)) = session_query {
                            write_error(socket, "ERROR", state, &message)?;
                            write_message(
                                socket,
                                b'Z',
                                if context.in_transaction() { b"T" } else { b"I" },
                            )?;
                            continue;
                        }
                        let catalog = crate::pg_catalog::CatalogQuery::parse(sql).map(|query| {
                            query.map(|mut query| {
                                query.current_schema = extended.session.schema().map(str::to_owned);
                                query
                            })
                        });
                        let parsed = if let Err(error) = &catalog {
                            Err(error.clone())
                        } else if catalog.as_ref().is_ok_and(|query| query.is_some()) {
                            Ok((std::borrow::Cow::Borrowed(sql), Some("SELECT")))
                        } else {
                            crate::pg_name::adapt(sql, context.as_ref(), &extended.session)
                                .and_then(|sql| {
                                    crate::pg_result::adapt_session_query(
                                        &sql,
                                        self.startup_epoch_micros,
                                    )
                                    .map(|s| s.into_owned())
                                })
                                .and_then(|sql| {
                                    crate::pg_result::command(&sql)
                                        .map(|command| (std::borrow::Cow::Owned(sql), command))
                                })
                        };
                        match parsed {
                            Ok((_, None)) => write_message(socket, b'I', &[])?,
                            Ok((sql, Some(command))) => {
                                let execution = self.with_query(pid, |context| {
                                    if let Some(catalog) =
                                        catalog.as_ref().ok().and_then(|query| query.as_ref())
                                    {
                                        return catalog
                                            .execute(context.as_ref())
                                            .map(|result| vec![result]);
                                    }
                                    context.execute_query(&sql, false, &CancellationToken::new())
                                })?;
                                match execution {
                                    Ok(results) => {
                                        let started = std::time::Instant::now();
                                        let response = if results.len() == 1 {
                                            crate::pg_result::write_result(
                                                socket,
                                                &results[0],
                                                command,
                                            )
                                        } else {
                                            write_error(
                                                socket,
                                                "ERROR",
                                                "0A000",
                                                "unsupported result multiplicity",
                                            )
                                        };
                                        for result in &results {
                                            if let Some(lifecycle) = &result.response_lifecycle {
                                                lifecycle.add_write_duration(started.elapsed());
                                                lifecycle.finish();
                                            }
                                        }
                                        if let Err(error) = response {
                                            if error.kind() == io::ErrorKind::InvalidData {
                                                write_error(
                                                    socket,
                                                    "ERROR",
                                                    "0A000",
                                                    &error.to_string(),
                                                )?;
                                            } else {
                                                return Err(error);
                                            }
                                        }
                                    }
                                    Err(error) => write_error(
                                        socket,
                                        "ERROR",
                                        sqlstate(&error),
                                        &error.to_string(),
                                    )?,
                                }
                                context.finish_protocol_response(Duration::ZERO);
                            }
                            Err((state, message)) => write_error(socket, "ERROR", state, &message)?,
                        }
                    } else {
                        write_error(
                            socket,
                            "ERROR",
                            "08P01",
                            "Query must contain one UTF-8 terminated string",
                        )?;
                    }
                    write_message(
                        socket,
                        b'Z',
                        if context.in_transaction() { b"T" } else { b"I" },
                    )?;
                    continue;
                }
                // Extended-query codecs are installed by subsequent tasks.
                write_error(
                    socket,
                    "ERROR",
                    "0A000",
                    "PostgreSQL command is not supported in this phase",
                )?;
                if tag == b'Q' || tag == b'S' {
                    write_message(
                        socket,
                        b'Z',
                        if context.in_transaction() { b"T" } else { b"I" },
                    )?;
                }
            }
        })();
        self.active.lock().unwrap().remove(&pid);
        let _ = context.close();
        result.map(|_| ())
    }
    pub(crate) fn cancel(&self, pid: u32, key: &[u8]) {
        let active = self.active.lock().unwrap();
        if let Some(entry) = active.get(&pid) {
            if key.len() == cancel_key_length(entry.protocol_version)
                && key.len() == entry.key.len()
                && key
                    .iter()
                    .zip(&entry.key)
                    .fold(0u8, |diff, (a, b)| diff | (a ^ b))
                    == 0
                && entry.executing
            {
                entry.context.cancel();
            }
        }
    }
    /// Runs a command under the same registry lock used by cancellation, so
    /// stale/idle CancelRequests cannot be carried into the next command.
    pub(crate) fn with_query<T>(
        &self,
        pid: u32,
        execute: impl FnOnce(&Arc<dyn TiDBContext>) -> T,
    ) -> io::Result<T> {
        let context = {
            let mut active = self.active.lock().unwrap();
            let entry = active
                .get_mut(&pid)
                .ok_or_else(|| invalid("unknown backend"))?;
            if entry.executing {
                return Err(invalid("backend already has an active command"));
            }
            entry.executing = true;
            entry.context.clone()
        };
        struct Completion<'a> {
            service: &'a PgService,
            pid: u32,
        }
        impl Drop for Completion<'_> {
            fn drop(&mut self) {
                if let Some(entry) = self.service.active.lock().unwrap().get_mut(&self.pid) {
                    entry.executing = false;
                    entry.context.finish_query_cancellation();
                }
            }
        }
        let _completion = Completion { service: self, pid };
        Ok(execute(&context))
    }
}
fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}
fn read_initial(socket: &mut TcpStream) -> io::Result<Vec<u8>> {
    let mut header = [0; 4];
    socket.read_exact(&mut header)?;
    let len = u32::from_be_bytes(header) as usize;
    if !(8..=MAX_STARTUP_LENGTH).contains(&len) {
        return Err(invalid("invalid startup length"));
    }
    let mut packet = vec![0; len];
    packet[..4].copy_from_slice(&header);
    socket.read_exact(&mut packet[4..])?;
    Ok(packet)
}
pub(crate) fn read_message(socket: &mut TcpStream) -> io::Result<(u8, Vec<u8>)> {
    let mut header = [0; 5];
    socket.read_exact(&mut header)?;
    let len = u32::from_be_bytes(header[1..].try_into().unwrap()) as usize;
    if !(4..=MAX_MESSAGE).contains(&len) {
        return Err(invalid("invalid message length"));
    }
    let mut body = vec![0; len - 4];
    socket.read_exact(&mut body)?;
    Ok((header[0], body))
}
pub(crate) fn write_message(socket: &mut TcpStream, tag: u8, body: &[u8]) -> io::Result<()> {
    socket.write_all(&[tag])?;
    socket.write_all(&((body.len() + 4) as u32).to_be_bytes())?;
    socket.write_all(body)
}
pub(crate) fn write_error(
    socket: &mut TcpStream,
    severity: &str,
    state: &str,
    message: &str,
) -> io::Result<()> {
    let mut body = Vec::new();
    for (tag, text) in [
        (b'S', severity),
        (b'V', severity),
        (b'C', state),
        (b'M', message),
    ] {
        body.push(tag);
        body.extend(text.bytes().filter(|b| *b != 0));
        body.push(0);
    }
    body.push(0);
    write_message(socket, b'E', &body)
}

/// Map known canonical engine errors at the PG boundary. Unknown errors retain
/// XX000; a SQLSTATE or arbitrary SQL text embedded in a message is not trusted.
pub(crate) fn sqlstate(error: &crate::conn::ConnError) -> &'static str {
    use crate::conn::ConnError;
    match error {
        ConnError::AccessDenied { .. } => "28000",
        ConnError::UnsupportedCommand(_) => "0A000",
        ConnError::ServerShutdown => "57P01",
        ConnError::MalformedPacket(_) | ConnError::UnsupportedProtocol => "08P01",
        ConnError::Session(message) => {
            if message == "PG oid conversion out of range"
                || message == "PG object ID exceeds the supported OID range"
            {
                "22003"
            } else if message == "invalid PG oid input" || message == "invalid PG regclass name" {
                "22P02"
            } else if message == "ambiguous PG index relation name" {
                "42725"
            } else if message.starts_with("[kv:1062]") || message.starts_with("Duplicate entry ") {
                "23505"
            } else if message.starts_with("Unknown column ") {
                "42703"
            } else if message.starts_with("Table ") && message.ends_with(" doesn't exist") {
                "42P01"
            } else if message.starts_with("Unknown database ") {
                "3D000"
            } else if message.contains("[executor:1317]")
                || message == "Query execution was interrupted"
            {
                "57014"
            } else {
                "XX000"
            }
        }
        _ => "XX000",
    }
}
