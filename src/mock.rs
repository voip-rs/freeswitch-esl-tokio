//! Scripted FreeSWITCH peer for integration tests (`test-util` feature).
//!
//! The test plays the switch one step at a time: accept a connection, read the
//! next command, answer it however the case needs, or not at all. Framing
//! follows `mod_event_socket.c`; header values are percent-encoded more widely
//! than the switch does, which every ESL decoder accepts.
//!
//! ```
//! use freeswitch_esl_tokio::mock::MockEslServer;
//! use freeswitch_esl_tokio::{EslClient, DEFAULT_ESL_PASSWORD};
//!
//! # #[tokio::main(flavor = "current_thread")]
//! # async fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let server = MockEslServer::start(DEFAULT_ESL_PASSWORD).await?;
//! let (accepted, connected) = tokio::join!(
//!     server.accept(),
//!     EslClient::connect("localhost", server.port(), DEFAULT_ESL_PASSWORD),
//! );
//! let mut switch = accepted?;
//! let (client, _events) = connected?;
//!
//! let (reply, command) = tokio::join!(client.api("status"), async {
//!     let command = switch.read_command().await?;
//!     switch.reply_api("UP 0 years, 0 days\n").await?;
//!     Ok::<_, std::io::Error>(command)
//! });
//! assert_eq!(command?, "api status\n\n");
//! assert_eq!(reply?.body(), Some("UP 0 years, 0 days\n"));
//! # Ok(())
//! # }
//! ```

use std::io;
use std::net::SocketAddr;

use percent_encoding::{percent_encode, NON_ALPHANUMERIC};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream, ToSocketAddrs};

use crate::constants::UNDEF_VALUE;
use crate::LogLevel;

const GOODBYE: &str = "Disconnected, goodbye.\nSee you at ClueCon! http://www.cluecon.com/\n";

/// Listening side: accepts connections and runs the `auth` handshake.
#[derive(Debug)]
pub struct MockEslServer {
    listener: TcpListener,
    addr: SocketAddr,
    password: String,
}

impl MockEslServer {
    /// Bind an ephemeral port on the dual-stack wildcard; dial it as
    /// `localhost` and [`port`](Self::port).
    pub async fn start(password: impl Into<String>) -> io::Result<Self> {
        Self::bind("[::]:0", password).await
    }

    /// Bind a caller-chosen address, e.g. `127.0.0.1:0` on a host without IPv6.
    pub async fn bind(addr: impl ToSocketAddrs, password: impl Into<String>) -> io::Result<Self> {
        let listener = TcpListener::bind(addr).await?;
        let addr = listener.local_addr()?;
        Ok(Self {
            listener,
            addr,
            password: password.into(),
        })
    }

    /// Bound address.
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// Bound port.
    pub fn port(&self) -> u16 {
        self.addr
            .port()
    }

    /// Accept one connection and answer its `auth` command.
    ///
    /// A wrong password, or any other first command, gets `-ERR invalid` and
    /// the disconnect notice, the connection closes, and this returns
    /// [`io::ErrorKind::PermissionDenied`].
    pub async fn accept(&self) -> io::Result<MockConnection> {
        let (stream, _peer) = self
            .listener
            .accept()
            .await?;
        let mut conn = MockConnection::from_stream(stream);
        conn.send_raw("Content-Type: auth/request\n\n")
            .await?;
        let command = conn
            .read_command()
            .await?;
        let password = command
            .strip_prefix("auth ")
            .and_then(|rest| rest.strip_suffix("\n\n"))
            .map(|pass| pass.trim_end_matches('\r'));
        if password
            == Some(
                self.password
                    .as_str(),
            )
        {
            conn.reply_ok()
                .await?;
            return Ok(conn);
        }
        conn.reply_err("invalid")
            .await?;
        conn.send_disconnect_notice(GOODBYE)
            .await?;
        Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "client failed the auth handshake",
        ))
    }
}

/// The switch's end of one accepted connection.
#[derive(Debug)]
pub struct MockConnection {
    stream: BufReader<TcpStream>,
}

impl MockConnection {
    /// Wrap a stream accepted elsewhere, skipping the handshake.
    pub fn from_stream(stream: TcpStream) -> Self {
        Self {
            stream: BufReader::new(stream),
        }
    }

    /// Write bytes verbatim.
    pub async fn send_raw(&mut self, data: &str) -> io::Result<()> {
        self.stream
            .get_mut()
            .write_all(data.as_bytes())
            .await
    }

    /// Next command, through its terminating blank line.
    ///
    /// [`io::ErrorKind::UnexpectedEof`] when the client closes first.
    pub async fn read_command(&mut self) -> io::Result<String> {
        let mut command = String::new();
        while !command.ends_with("\n\n") {
            if self
                .stream
                .read_line(&mut command)
                .await?
                == 0
            {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "client closed before completing a command",
                ));
            }
        }
        Ok(command)
    }

    /// `command/reply` with `+OK accepted`.
    pub async fn reply_ok(&mut self) -> io::Result<()> {
        self.reply_raw_text("+OK accepted")
            .await
    }

    /// `command/reply` with `+OK <text>`.
    pub async fn reply_ok_text(&mut self, text: &str) -> io::Result<()> {
        self.reply_raw_text(&format!("+OK {text}"))
            .await
    }

    /// `command/reply` with `-ERR <text>`.
    pub async fn reply_err(&mut self, text: &str) -> io::Result<()> {
        self.reply_raw_text(&format!("-ERR {text}"))
            .await
    }

    /// `command/reply` with `Reply-Text` exactly as given.
    pub async fn reply_raw_text(&mut self, text: &str) -> io::Result<()> {
        self.send_raw(&format!(
            "Content-Type: command/reply\nReply-Text: {text}\n\n"
        ))
        .await
    }

    /// `api/response` carrying `body`, whether `+OK`, `-ERR` or `-USAGE`.
    pub async fn reply_api(&mut self, body: &str) -> io::Result<()> {
        self.send_raw(&format!(
            "Content-Type: api/response\nContent-Length: {}\n\n{body}",
            body.len()
        ))
        .await
    }

    /// Accept a `bgapi` under `job_uuid`.
    pub async fn reply_bgapi(&mut self, job_uuid: &str) -> io::Result<()> {
        self.send_raw(&format!(
            "Content-Type: command/reply\nReply-Text: +OK Job-UUID: {job_uuid}\nJob-UUID: {job_uuid}\n\n"
        ))
        .await
    }

    /// `text/event-plain` event without a body.
    pub async fn send_event_plain<K, V>(
        &mut self,
        event_name: &str,
        headers: impl IntoIterator<Item = (K, V)>,
    ) -> io::Result<()>
    where
        K: AsRef<str>,
        V: AsRef<str>,
    {
        let mut inner = serialize_headers(event_name, headers);
        inner.push('\n');
        self.send_event_frame(&inner)
            .await
    }

    /// `text/event-plain` event with an inner body, as `BACKGROUND_JOB` carries.
    pub async fn send_event_plain_with_body<K, V>(
        &mut self,
        event_name: &str,
        headers: impl IntoIterator<Item = (K, V)>,
        event_body: &str,
    ) -> io::Result<()>
    where
        K: AsRef<str>,
        V: AsRef<str>,
    {
        let mut inner = serialize_headers(event_name, headers);
        inner.push_str(&format!(
            "Content-Length: {}\n\n{event_body}",
            event_body.len()
        ));
        self.send_event_frame(&inner)
            .await
    }

    /// `BACKGROUND_JOB` event completing `job` with `body`.
    pub async fn send_background_job(
        &mut self,
        job: &MockBackgroundJob,
        body: &str,
    ) -> io::Result<()> {
        let mut headers = vec![
            (
                "Job-UUID",
                job.job_uuid
                    .as_str(),
            ),
            (
                "Job-Owner-UUID",
                job.owner_uuid
                    .as_str(),
            ),
            (
                "Job-Command",
                job.command
                    .as_str(),
            ),
        ];
        if let Some(arg) = &job.arg {
            headers.push(("Job-Command-Arg", arg));
        }
        self.send_event_plain_with_body("BACKGROUND_JOB", headers, body)
            .await
    }

    /// `log/data` message; `Log-Func`, `Log-Line` and `User-Data` are fixed.
    pub async fn send_log(&mut self, level: LogLevel, file: &str, text: &str) -> io::Result<()> {
        self.send_raw(&format!(
            "Content-Type: log/data\nContent-Length: {}\nLog-Level: {}\nText-Channel: 0\nLog-File: {file}\nLog-Func: mock\nLog-Line: 1\nUser-Data: \n\n{text}",
            text.len(),
            level as i8
        ))
        .await
    }

    /// `HEARTBEAT` event with plausible headers.
    pub async fn send_heartbeat(&mut self) -> io::Result<()> {
        self.send_event_plain(
            "HEARTBEAT",
            [
                ("Core-UUID", "test-core-uuid"),
                ("FreeSWITCH-Hostname", "test-host"),
                ("Event-Info", "System Ready"),
                ("Up-Time", "0 years, 0 days, 1 hour, 23 minutes"),
                ("Session-Count", "5"),
                ("Max-Sessions", "1000"),
                ("Heartbeat-Interval", "20"),
            ],
        )
        .await
    }

    /// Disconnect notice as an inbound listener sends it.
    pub async fn send_disconnect_notice(&mut self, message: &str) -> io::Result<()> {
        self.send_raw(&format!(
            "Content-Type: text/disconnect-notice\nContent-Length: {}\n\n{message}",
            message.len()
        ))
        .await
    }

    /// Outbound `connect` reply: channel data serialized flat, no envelope.
    pub async fn send_connect_response<K, V>(
        &mut self,
        channel_headers: impl IntoIterator<Item = (K, V)>,
    ) -> io::Result<()>
    where
        K: AsRef<str>,
        V: AsRef<str>,
    {
        let mut data = String::new();
        let protocol_headers = [
            ("Content-Type", "command/reply"),
            ("Reply-Text", "+OK"),
            ("Socket-Mode", "async"),
            ("Control", "full"),
        ];
        for (key, value) in channel_headers {
            push_header(&mut data, key.as_ref(), value.as_ref());
        }
        for (key, value) in protocol_headers {
            push_header(&mut data, key, value);
        }
        data.push('\n');
        self.send_raw(&data)
            .await
    }

    /// Close the connection.
    pub async fn drop_connection(self) {
        drop(self.stream);
    }

    async fn send_event_frame(&mut self, inner: &str) -> io::Result<()> {
        self.send_raw(&format!(
            "Content-Length: {}\nContent-Type: text/event-plain\n\n{inner}",
            inner.len()
        ))
        .await
    }
}

/// Header set of a `BACKGROUND_JOB` event.
#[derive(Debug, Clone)]
pub struct MockBackgroundJob {
    job_uuid: String,
    owner_uuid: String,
    command: String,
    arg: Option<String>,
}

impl MockBackgroundJob {
    /// Job for `command` with no argument and a nil owner UUID.
    pub fn new(job_uuid: impl Into<String>, command: impl Into<String>) -> Self {
        Self {
            job_uuid: job_uuid.into(),
            owner_uuid: "00000000-0000-0000-0000-000000000000".into(),
            command: command.into(),
            arg: None,
        }
    }

    /// `Job-Command-Arg`.
    pub fn with_arg(mut self, arg: impl Into<String>) -> Self {
        self.arg = Some(arg.into());
        self
    }
}

fn serialize_headers<K, V>(event_name: &str, headers: impl IntoIterator<Item = (K, V)>) -> String
where
    K: AsRef<str>,
    V: AsRef<str>,
{
    let mut out = String::new();
    push_header(&mut out, "Event-Name", event_name);
    for (key, value) in headers {
        push_header(&mut out, key.as_ref(), value.as_ref());
    }
    out
}

fn push_header(out: &mut String, key: &str, value: &str) {
    if value.is_empty() {
        out.push_str(&format!("{key}: {UNDEF_VALUE}\n"));
    } else {
        out.push_str(&format!(
            "{key}: {}\n",
            percent_encode(value.as_bytes(), NON_ALPHANUMERIC)
        ));
    }
}
