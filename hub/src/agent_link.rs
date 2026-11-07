use std::time::Duration;

use nixyon_agent::protocol::{
    ErrorCode, MAX_LINE_BYTES, Op, QualityTake, Request, ServerFrame, decode, encode,
};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const REPLY_TIMEOUT: Duration = Duration::from_secs(10);

/// Что делать в задании: план проверки качества или записи для прогона без разметки.
#[derive(Debug, Clone, Default)]
pub struct Job {
    pub plan: Vec<QualityTake>,
    pub records: Vec<String>,
    pub training: Vec<String>,
    /// Номер прогона GitHub Actions для команд CI.
    pub ci_run: String,
}

#[derive(Clone)]
pub struct AgentLink {
    addr: String,
    token: String,
}

#[derive(Debug, thiserror::Error)]
pub enum LinkError {
    #[error("агент недоступен: {0}")]
    Unavailable(String),
    #[error("агент ответил ошибкой {0:?}: {1}")]
    Agent(ErrorCode, String),
    #[error("агент ответил непонятно")]
    Protocol,
}

pub struct Session {
    pub reader: FrameReader,
    pub writer: OwnedWriteHalf,
}

pub struct FrameReader {
    inner: BufReader<OwnedReadHalf>,
    line: Vec<u8>,
}

impl FrameReader {
    pub async fn next(&mut self) -> Result<Option<ServerFrame>, LinkError> {
        self.line.clear();
        let read = (&mut self.inner)
            .take(MAX_LINE_BYTES as u64 + 1)
            .read_until(b'\n', &mut self.line)
            .await
            .map_err(|e| LinkError::Unavailable(e.to_string()))?;
        if read == 0 {
            return Ok(None);
        }
        if self.line.len() > MAX_LINE_BYTES {
            return Err(LinkError::Protocol);
        }
        decode(self.line.trim_ascii_end())
            .map(Some)
            .map_err(|_| LinkError::Protocol)
    }
}

impl AgentLink {
    pub fn new(addr: String, token: String) -> Self {
        Self { addr, token }
    }

    pub async fn open(
        &self,
        op: Op,
        user: &str,
        cols: u16,
        rows: u16,
    ) -> Result<Session, LinkError> {
        self.open_with(op, user, cols, rows, Job::default()).await
    }

    async fn open_with(
        &self,
        op: Op,
        user: &str,
        cols: u16,
        rows: u16,
        job: Job,
    ) -> Result<Session, LinkError> {
        let stream = tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect(&self.addr))
            .await
            .map_err(|_| LinkError::Unavailable("время ожидания вышло".into()))?
            .map_err(|e| LinkError::Unavailable(e.to_string()))?;
        let (read, mut writer) = stream.into_split();
        let request = Request {
            token: self.token.clone(),
            op,
            user: user.into(),
            cols,
            rows,
            plan: job.plan,
            records: job.records,
            training: job.training,
            ci_run: job.ci_run,
        };
        let line = encode(&request).map_err(|_| LinkError::Protocol)?;
        writer
            .write_all(&line)
            .await
            .map_err(|e| LinkError::Unavailable(e.to_string()))?;
        Ok(Session {
            reader: FrameReader {
                inner: BufReader::new(read),
                line: Vec::new(),
            },
            writer,
        })
    }

    pub async fn single(&self, op: Op, user: &str) -> Result<ServerFrame, LinkError> {
        self.single_with(op, user, Job::default()).await
    }

    /// Запрос с тем, что делать в задании, и один ответ.
    pub async fn single_with(
        &self,
        op: Op,
        user: &str,
        job: Job,
    ) -> Result<ServerFrame, LinkError> {
        let mut session = self.open_with(op, user, 0, 0, job).await?;
        let frame = tokio::time::timeout(REPLY_TIMEOUT, session.reader.next())
            .await
            .map_err(|_| LinkError::Unavailable("время ожидания вышло".into()))??
            .ok_or(LinkError::Protocol)?;
        match frame {
            ServerFrame::Error { code, message } => Err(LinkError::Agent(code, message)),
            other => Ok(other),
        }
    }
}
