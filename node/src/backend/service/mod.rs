#![allow(clippy::ptr_offset_with_cast)]
//! An Backend HTTP service handle custom message from `MessageHandler` as CallbackFn.
pub mod http_server;
pub mod proxy;
pub mod tcp_server;
pub mod text;
pub mod utils;

use std::sync::Arc;

use arrayref::array_refs;
use async_trait::async_trait;
use bytes::Bytes;
use serde::Deserialize;
use serde::Serialize;
use tokio::sync::broadcast::Sender;
use tokio::sync::Mutex;

use crate::backend::extension::Extension;
use crate::backend::extension::ExtensionConfig;
use crate::backend::service::http_server::HttpServer;
use crate::backend::service::http_server::HttpServiceConfig;
use crate::backend::service::tcp_server::TcpServer;
use crate::backend::service::tcp_server::TcpServiceConfig;
use crate::backend::service::text::TextEndpoint;
use crate::backend::types::BackendMessage;
use crate::backend::types::MessageEndpoint;
use crate::backend::types::MessageType;
use crate::consts::BACKEND_MTU;
use crate::error::Error;
use crate::error::Result;
use crate::prelude::rings_core::chunk::Chunk;
use crate::prelude::rings_core::chunk::ChunkList;
use crate::prelude::rings_core::chunk::ChunkManager;
use crate::prelude::rings_core::swarm::callback::SwarmCallback;
use crate::prelude::*;

/// A Backend struct contains http_server.
pub struct Backend {
    pub swarm: Arc<Swarm>,
    http_server: Arc<HttpServer>,
    pub tcp_server: Arc<TcpServer>,
    text_endpoint: TextEndpoint,
    extension_endpoint: Extension,
    sender: Sender<BackendMessage>,
    chunk_list: Arc<Mutex<ChunkList<BACKEND_MTU>>>,
}

/// BackendConfig
#[derive(Deserialize, Serialize, Debug, Default)]
pub struct BackendConfig {
    /// hidden http services
    pub http_services: Vec<HttpServiceConfig>,
    /// hidden tcp services
    pub tcp_services: Vec<TcpServiceConfig>,
    /// extension
    pub extensions: ExtensionConfig,
}

/// HiddenServerMode
#[derive(Deserialize, Serialize, Debug, Clone)]
pub enum HiddenServerMode {
    /// HTTP
    Http {
        /// prefix of url
        /// Example 1: http://127.0.0.1:8080
        /// Example 2: https://mainnet.infura.io/v3
        prefix: String,
    },
    /// TCP
    Tcp {},
}

#[cfg(feature = "node")]
impl Backend {
    /// new backend
    /// - `ipfs_gateway`
    pub async fn new(
        config: BackendConfig,
        sender: Sender<BackendMessage>,
        swarm: Arc<Swarm>,
    ) -> Result<Self> {
        Ok(Self {
            swarm: swarm.clone(),
            http_server: Arc::new(HttpServer::from(config.http_services)),
            tcp_server: Arc::new(TcpServer::new(config.tcp_services, swarm.clone())),
            text_endpoint: TextEndpoint,
            sender,
            extension_endpoint: Extension::new(&config.extensions).await?,
            chunk_list: Default::default(),
        })
    }

    async fn handle_chunk_data(&self, data: &[u8]) -> Result<Option<Bytes>> {
        let chunk_item = Chunk::from_bincode(data).map_err(|_| Error::DecodeError)?;
        let mut chunk_list = self.chunk_list.lock().await;
        let data = chunk_list.handle(chunk_item);
        Ok(data)
    }

    /// Get service names from server config for storage register.
    pub fn service_names(&self) -> Vec<String> {
        let http_services = self
            .http_server
            .services
            .iter()
            .cloned()
            .filter_map(|b| b.register_service);

        let tcp_services = self
            .tcp_server
            .services
            .iter()
            .cloned()
            .filter_map(|b| b.register_service);

        http_services.chain(tcp_services).collect()
    }
}

#[cfg(feature = "node")]
#[async_trait]
impl SwarmCallback for Backend {
    async fn on_payload(
        &self,
        payload: &MessagePayload,
    ) -> std::result::Result<(), Box<dyn std::error::Error>> {
        if payload.transaction.destination != self.swarm.did() {
            return Ok(());
        }

        let data: Message = payload.transaction.data()?;

        let Message::CustomMessage(CustomMessage(msg)) = data else {
            return Ok(());
        };

        let (left, msg) = array_refs![&msg, 4; ..;];
        let (&[flag], _) = array_refs![left, 1, 3];

        let msg = if flag == 1 {
            let data = self.handle_chunk_data(msg).await?;
            if let Some(data) = data {
                BackendMessage::try_from(data.to_vec().as_ref())
            } else {
                return Ok(());
            }
        } else if flag == 0 {
            BackendMessage::try_from(msg)
        } else {
            tracing::warn!("invalid custom_message flag: {}", flag);
            return Ok(());
        };

        if let Err(e) = msg {
            tracing::error!("decode custom_message failed: {}", e);
            return Ok(());
        }
        let msg = msg.unwrap();
        tracing::debug!("receive custom_message: {:?}", msg);

        let result = match msg.message_type.into() {
            MessageType::SimpleText => self.text_endpoint.handle_message(payload, &msg).await,
            MessageType::HttpRequest => self.http_server.handle_message(payload, &msg).await,
            MessageType::TunnelMessage => self.tcp_server.handle_message(payload, &msg).await,
            MessageType::Extension => self.extension_endpoint.handle_message(payload, &msg).await,
            _ => {
                tracing::debug!(
                    "custom_message handle unsupported, tag: {:?}",
                    msg.message_type
                );
                Ok(vec![])
            }
        };
        if let Err(e) = self.sender.send(msg) {
            tracing::error!("broadcast backend_message failed, {}", e);
        }

        match result {
            Ok(v) => self
                .swarm
                .handle_message_handler_events(&v)
                .await
                .map_err(|e| e.into()),
            Err(e) => {
                tracing::error!("handle custom_message failed: {}", e);
                Ok(())
            }
        }
    }
}
