#![warn(missing_docs)]
//! handle simple text message
use std::str;

use async_trait::async_trait;

use crate::backend::types::BackendMessage;
use crate::backend::types::MessageEndpoint;
use crate::error::Error;
use crate::error::Result;
use crate::prelude::*;

/// SimpleTextEndpoint
#[derive(Clone, Debug, Default)]
pub struct TextEndpoint;

#[async_trait]
impl MessageEndpoint for TextEndpoint {
    async fn handle_message(
        &self,
        ctx: &MessagePayload,
        data: &BackendMessage,
    ) -> Result<Vec<MessageHandlerEvent>> {
        let text = str::from_utf8(&data.data).map_err(|_| Error::InvalidMessage)?;
        tracing::info!(
            "SimpleText, From: {}, Text: {}",
            ctx.relay.origin_sender(),
            text
        );
        Ok(vec![])
    }
}
