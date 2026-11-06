use std::io::{Read, Write};
use std::path::Path;
use std::sync::{Arc, Mutex};

use portable_pty::{CommandBuilder, MasterPty, PtySize, native_pty_system};
use tokio::io::{AsyncBufRead, AsyncWrite};
use tokio::sync::mpsc;

use super::frames::{FrameError, read_line, send};
use crate::protocol::{ClientFrame, ServerFrame, decode};

const READ_CHUNK: usize = 8192;
const MIN_SIZE: u16 = 2;
const MAX_SIZE: u16 = 1000;

#[derive(Debug, thiserror::Error)]
pub enum TerminalError {
    #[error("терминал не открылся: {0}")]
    Open(String),
    #[error(transparent)]
    Frame(#[from] FrameError),
}

fn size(cols: u16, rows: u16) -> PtySize {
    PtySize {
        rows: rows.clamp(MIN_SIZE, MAX_SIZE),
        cols: cols.clamp(MIN_SIZE, MAX_SIZE),
        pixel_width: 0,
        pixel_height: 0,
    }
}

pub async fn session<R, W>(
    reader: &mut R,
    writer: &mut W,
    shell: &str,
    dir: &Path,
    cols: u16,
    rows: u16,
) -> Result<(), TerminalError>
where
    R: AsyncBufRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let pair = native_pty_system()
        .openpty(size(cols, rows))
        .map_err(|e| TerminalError::Open(e.to_string()))?;
    let mut cmd = CommandBuilder::new(shell);
    if dir.is_dir() {
        cmd.cwd(dir);
    }
    cmd.env("TERM", "xterm-256color");
    let mut child = pair
        .slave
        .spawn_command(cmd)
        .map_err(|e| TerminalError::Open(e.to_string()))?;
    drop(pair.slave);
    let mut pty_reader = pair
        .master
        .try_clone_reader()
        .map_err(|e| TerminalError::Open(e.to_string()))?;
    let mut pty_writer = pair
        .master
        .take_writer()
        .map_err(|e| TerminalError::Open(e.to_string()))?;
    let master: Arc<Mutex<Box<dyn MasterPty + Send>>> = Arc::new(Mutex::new(pair.master));

    let (out_tx, mut out_rx) = mpsc::channel::<Vec<u8>>(64);
    std::thread::spawn(move || {
        let mut buf = vec![0u8; READ_CHUNK];
        loop {
            match pty_reader.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if out_tx.blocking_send(buf[..n].to_vec()).is_err() {
                        break;
                    }
                }
            }
        }
    });
    let (in_tx, in_rx) = std::sync::mpsc::channel::<Vec<u8>>();
    std::thread::spawn(move || {
        while let Ok(data) = in_rx.recv() {
            if pty_writer
                .write_all(&data)
                .and_then(|()| pty_writer.flush())
                .is_err()
            {
                break;
            }
        }
    });

    let result = loop {
        tokio::select! {
            output = out_rx.recv() => match output {
                Some(data) => send(writer, &ServerFrame::Output { data }).await?,
                None => break Ok(()),
            },
            line = read_line(reader) => match line {
                Ok(Some(line)) => match decode::<ClientFrame>(&line) {
                    Ok(ClientFrame::Input { data }) => {
                        if in_tx.send(data).is_err() {
                            break Ok(());
                        }
                    }
                    Ok(ClientFrame::Resize { cols, rows }) => {
                        if let Ok(m) = master.lock() {
                            let _ = m.resize(size(cols, rows));
                        }
                    }
                    Err(_) => {}
                },
                Ok(None) => break Ok(()),
                Err(e) => break Err(e.into()),
            },
        }
    };

    let _ = child.kill();
    let code = tokio::task::spawn_blocking(move || child.wait().ok().map(|s| s.exit_code()))
        .await
        .ok()
        .flatten();
    let _ = send(writer, &ServerFrame::Exit { code }).await;
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::encode;
    use tokio::io::{AsyncWriteExt, BufReader, duplex};

    #[tokio::test]
    async fn shell_output_reaches_client_and_exit_is_reported() {
        let (client, server) = duplex(1 << 16);
        let (server_read, mut server_write) = tokio::io::split(server);
        let (client_read, mut client_write) = tokio::io::split(client);
        let dir = std::env::temp_dir();
        let session = tokio::spawn(async move {
            let mut reader = BufReader::new(server_read);
            session(&mut reader, &mut server_write, "sh", &dir, 80, 24).await
        });
        client_write
            .write_all(
                &encode(&ClientFrame::Resize {
                    cols: 100,
                    rows: 30,
                })
                .unwrap(),
            )
            .await
            .unwrap();
        client_write
            .write_all(
                &encode(&ClientFrame::Input {
                    data: b"echo nixyon-$((40+2))\n".to_vec(),
                })
                .unwrap(),
            )
            .await
            .unwrap();
        client_write
            .write_all(
                &encode(&ClientFrame::Input {
                    data: b"exit\n".to_vec(),
                })
                .unwrap(),
            )
            .await
            .unwrap();

        let mut reader = BufReader::new(client_read);
        let mut output = Vec::new();
        let mut exited = false;
        while let Ok(Some(line)) =
            tokio::time::timeout(std::time::Duration::from_secs(10), read_line(&mut reader))
                .await
                .unwrap()
        {
            match decode::<ServerFrame>(&line).unwrap() {
                ServerFrame::Output { data } => output.extend(data),
                ServerFrame::Exit { .. } => {
                    exited = true;
                    break;
                }
                other => panic!("неожиданный кадр {other:?}"),
            }
        }
        assert!(exited);
        assert!(String::from_utf8_lossy(&output).contains("nixyon-42"));
        drop(client_write);
        session.await.unwrap().unwrap();
    }

    #[test]
    fn terminal_size_is_clamped() {
        let s = size(0, 60_000);
        assert_eq!((s.cols, s.rows), (MIN_SIZE, MAX_SIZE));
    }
}
