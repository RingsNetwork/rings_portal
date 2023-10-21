use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde::Deserialize;
use serde::Serialize;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;

use crate::backend::types::BackendMessage;
use crate::backend::types::MessageType;
use crate::error::TunnelDefeat;
use crate::prelude::rings_core::dht::Did;
use crate::prelude::rings_core::prelude::uuid::Uuid;
use crate::prelude::Message;
use crate::prelude::PayloadSender;
use crate::prelude::Swarm;

pub type TunnelId = Uuid;

#[derive(Deserialize, Serialize, Debug, Clone)]
pub enum TunnelMessage {
    TcpDial { tid: TunnelId, service: String },
    TcpClose { tid: TunnelId, reason: TunnelDefeat },
    TcpPackage { tid: TunnelId, body: Bytes },
}

pub struct Tunnel {
    tid: TunnelId,
    remote_stream_tx: Option<mpsc::Sender<Bytes>>,
    listener_cancel_token: Option<CancellationToken>,
    listener: Option<tokio::task::JoinHandle<()>>,
}

pub struct TunnelListener {
    tid: TunnelId,
    local_stream: TcpStream,
    remote_stream_tx: mpsc::Sender<Bytes>,
    remote_stream_rx: mpsc::Receiver<Bytes>,
    swarm: Arc<Swarm>,
    peer_did: Did,
    cancel_token: CancellationToken,
}

impl Drop for Tunnel {
    fn drop(&mut self) {
        if let Some(cancel_token) = self.listener_cancel_token.take() {
            cancel_token.cancel();
        }

        if let Some(listener) = self.listener.take() {
            tokio::spawn(async move {
                tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;
                listener.abort();
            });
        }

        tracing::info!("Tunnel {} dropped", self.tid);
    }
}

impl Tunnel {
    pub fn new(tid: TunnelId) -> Self {
        Self {
            tid,
            remote_stream_tx: None,
            listener: None,
            listener_cancel_token: None,
        }
    }

    pub async fn send(&self, bytes: Bytes) {
        if let Some(ref tx) = self.remote_stream_tx {
            let _ = tx.send(bytes).await;
        } else {
            tracing::error!("Tunnel {} remote stream tx is none", self.tid);
        }
    }

    pub async fn listen(&mut self, local_stream: TcpStream, swarm: Arc<Swarm>, peer_did: Did) {
        if self.listener.is_some() {
            return;
        }

        let mut listener = TunnelListener::new(self.tid, local_stream, swarm, peer_did).await;
        let listener_cancel_token = listener.cancel_token();
        let remote_stream_tx = listener.remote_stream_tx.clone();
        let listener_handler = tokio::spawn(Box::pin(async move { listener.listen().await }));

        self.remote_stream_tx = Some(remote_stream_tx);
        self.listener = Some(listener_handler);
        self.listener_cancel_token = Some(listener_cancel_token);
    }
}

impl TunnelListener {
    async fn new(tid: TunnelId, local_stream: TcpStream, swarm: Arc<Swarm>, peer_did: Did) -> Self {
        let (remote_stream_tx, remote_stream_rx) = mpsc::channel(1024);
        Self {
            tid,
            local_stream,
            remote_stream_tx,
            remote_stream_rx,
            swarm,
            peer_did,
            cancel_token: CancellationToken::new(),
        }
    }

    fn cancel_token(&self) -> CancellationToken {
        self.cancel_token.clone()
    }

    async fn listen(&mut self) {
        let (mut local_read, mut local_write) = self.local_stream.split();

        let listen_local = async {
            loop {
                if self.cancel_token.is_cancelled() {
                    break TunnelDefeat::ConnectionClosed;
                }

                let mut buf = [0u8; 30000];
                match local_read.read(&mut buf).await {
                    Err(e) => {
                        break e.kind().into();
                    }
                    Ok(n) if n == 0 => {
                        break TunnelDefeat::ConnectionClosed;
                    }
                    Ok(n) => {
                        let body = Bytes::copy_from_slice(&buf[..n]);
                        let message = TunnelMessage::TcpPackage {
                            tid: self.tid,
                            body,
                        };
                        let custom_msg = wrap_custom_message(&message);
                        if let Err(e) = self.swarm.send_message(custom_msg, self.peer_did).await {
                            tracing::error!("Send TcpPackage message failed: {e:?}");
                            break TunnelDefeat::WebrtcDatachannelSendFailed;
                        }
                    }
                }
            }
        };

        let listen_remote = async {
            loop {
                if self.cancel_token.is_cancelled() {
                    break TunnelDefeat::ConnectionClosed;
                }

                if let Some(body) = self.remote_stream_rx.recv().await {
                    if let Err(e) = local_write.write_all(&body).await {
                        tracing::error!("Write to local stream failed: {e:?}");
                        break e.kind().into();
                    }
                }
            }
        };

        tokio::select! {
            defeat = listen_local => {
                tracing::info!("Local stream closed: {defeat:?}");
                let message = TunnelMessage::TcpClose {
                    tid: self.tid,
                    reason: defeat,
                };
                let custom_msg = wrap_custom_message(&message);
                if let Err(e) =  self.swarm.send_message(custom_msg, self.peer_did).await {
                    tracing::error!("Send TcpClose message failed: {e:?}");
                }
            },
            defeat = listen_remote => {
                tracing::info!("Remote stream closed: {defeat:?}");
                let message = TunnelMessage::TcpClose {
                    tid: self.tid,
                    reason: defeat,
                };
                let custom_msg = wrap_custom_message(&message);
                let _ = self.swarm.send_message(custom_msg, self.peer_did).await;
            }
        }
    }
}

pub async fn tcp_connect_with_timeout(
    addr: SocketAddr,
    request_timeout_s: u64,
) -> Result<TcpStream, TunnelDefeat> {
    let fut = tcp_connect(addr);
    match timeout(Duration::from_secs(request_timeout_s), fut).await {
        Ok(result) => result,
        Err(_) => Err(TunnelDefeat::ConnectionTimeout),
    }
}

async fn tcp_connect(addr: SocketAddr) -> Result<TcpStream, TunnelDefeat> {
    match TcpStream::connect(addr).await {
        Ok(o) => Ok(o),
        Err(e) => Err(e.kind().into()),
    }
}

pub fn wrap_custom_message(message: &TunnelMessage) -> Message {
    let message_bytes = bincode::serialize(message).unwrap();

    let backend_msg =
        BackendMessage::from((MessageType::TunnelMessage.into(), message_bytes.as_slice()));

    let backend_msg_bytes: Vec<u8> = backend_msg.into();

    let mut new_bytes: Vec<u8> = Vec::with_capacity(backend_msg_bytes.len() + 4);
    new_bytes.push(0);
    new_bytes.extend_from_slice(&[0u8; 3]);
    new_bytes.extend_from_slice(&backend_msg_bytes);

    Message::custom(&new_bytes).unwrap()
}
