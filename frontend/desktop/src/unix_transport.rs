use std::sync::atomic::Ordering;
use std::sync::Arc;

pub struct UnixTransport {
    lines: tokio::io::Lines<tokio::io::BufReader<tokio::net::unix::OwnedReadHalf>>,
    write: tokio::net::unix::OwnedWriteHalf,

    seat: String,

    dead: Arc<std::sync::atomic::AtomicBool>,
}

impl UnixTransport {
    pub async fn connect(
        path: &str,
        seat: String,
        dead: Arc<std::sync::atomic::AtomicBool>,
    ) -> Result<Self, String> {
        tracing::info!(target: "ahp_wire", seat = %seat, path, "unix connect");
        let stream = match tokio::net::UnixStream::connect(path).await {
            Ok(stream) => stream,
            Err(error) => {
                tracing::error!(target: "ahp_wire", seat = %seat, path, %error, "unix connect failed");
                return Err(format!("agent host unreachable at unix:{path}: {error}"));
            }
        };
        let (read, write) = stream.into_split();
        Ok(Self {
            lines: tokio::io::AsyncBufReadExt::lines(tokio::io::BufReader::new(read)),
            write,
            seat,
            dead,
        })
    }
}

impl ahp::Transport for UnixTransport {
    async fn send(
        &mut self,
        msg: ahp::transport::TransportMessage,
    ) -> Result<(), ahp::TransportError> {
        use ahp::transport::TransportMessage;
        use tokio::io::AsyncWriteExt;
        let text = match msg {
            TransportMessage::Text(text) => text,
            TransportMessage::Parsed(message) => serde_json::to_string(&message)
                .map_err(|error| ahp::TransportError::Protocol(error.to_string()))?,
            TransportMessage::Binary(bytes) => String::from_utf8(bytes)
                .map_err(|error| ahp::TransportError::Protocol(error.to_string()))?,
        };
        tracing::info!(target: "ahp_wire", seat = %self.seat, line = %host_discovery::logging::brief(&text), "->");
        let dead = |error: std::io::Error| {
            self.dead.store(true, Ordering::Relaxed);
            tracing::error!(target: "ahp_wire", seat = %self.seat, %error, "write error");
            ahp::TransportError::Io(error.to_string())
        };
        self.write.write_all(text.as_bytes()).await.map_err(dead)?;
        self.write.write_all(b"\n").await.map_err(dead)?;
        self.write.flush().await.map_err(dead)
    }

    async fn recv(
        &mut self,
    ) -> Result<Option<ahp::transport::TransportMessage>, ahp::TransportError> {
        match self.lines.next_line().await {
            Ok(Some(line)) => {
                tracing::info!(target: "ahp_wire", seat = %self.seat, line = %host_discovery::logging::brief(&line), "<-");
                Ok(Some(ahp::transport::TransportMessage::Text(line)))
            }
            Ok(None) => {
                self.dead.store(true, Ordering::Relaxed);
                tracing::warn!(target: "ahp_wire", seat = %self.seat, "EOF — the host closed the connection");
                Ok(None)
            }
            Err(error) => {
                self.dead.store(true, Ordering::Relaxed);
                tracing::error!(target: "ahp_wire", seat = %self.seat, %error, "read error");
                Err(ahp::TransportError::Io(error.to_string()))
            }
        }
    }
}
