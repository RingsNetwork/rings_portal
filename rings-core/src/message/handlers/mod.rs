use crate::dht::{Did, PeerRing};
use crate::err::{Error, Result};
use crate::message::payload::{MessageRelay, MessageRelayMethod};
use crate::message::types::Message;
use crate::swarm::Swarm;

use async_recursion::async_recursion;
use futures::lock::Mutex;
use std::collections::VecDeque;
use std::sync::Arc;
use web3::types::Address;

pub mod connection;
pub mod storage;

use connection::TChordConnection;
use storage::TChordStorage;

#[cfg(not(feature = "wasm"))]
type CallbackFn = Box<dyn FnMut(&MessageRelay<Message>, Did) -> Result<()> + Send + Sync>;

#[cfg(feature = "wasm")]
type CallbackFn = Box<dyn FnMut(&MessageRelay<Message>, Did) -> Result<()>>;

#[derive(Clone)]
pub struct MessageHandler {
    dht: Arc<Mutex<PeerRing>>,
    swarm: Arc<Swarm>,
    callback: Option<Arc<Mutex<CallbackFn>>>,
}

impl MessageHandler {
    pub fn new_with_callback(
        dht: Arc<Mutex<PeerRing>>,
        swarm: Arc<Swarm>,
        callback: CallbackFn,
    ) -> Self {
        Self {
            dht,
            swarm,
            callback: Some(Arc::new(Mutex::new(callback))),
        }
    }

    pub fn new(dht: Arc<Mutex<PeerRing>>, swarm: Arc<Swarm>) -> Self {
        Self {
            dht,
            swarm,
            callback: None,
        }
    }

    pub async fn send_relay_message(
        &self,
        address: &Address,
        msg: MessageRelay<Message>,
    ) -> Result<()> {
        self.swarm.send_message(address, msg).await
    }

    pub async fn send_message_default(&self, address: &Address, message: Message) -> Result<()> {
        self.send_message(address, None, None, MessageRelayMethod::SEND, message)
            .await
    }

    pub async fn send_message(
        &self,
        address: &Address,
        to_path: Option<VecDeque<Did>>,
        from_path: Option<VecDeque<Did>>,
        method: MessageRelayMethod,
        message: Message,
    ) -> Result<()> {
        // TODO: diff ttl for each message?
        let payload = MessageRelay::new(
            message,
            &self.swarm.session(),
            None,
            to_path,
            from_path,
            method,
        )?;
        self.send_relay_message(address, payload).await
    }

    #[cfg_attr(feature = "wasm", async_recursion(?Send))]
    #[cfg_attr(not(feature = "wasm"), async_recursion)]
    pub async fn handle_message_relay(
        &self,
        relay: MessageRelay<Message>,
        prev: Did,
    ) -> Result<()> {
        let data = relay.data.clone();
        match data {
            Message::JoinDHT(msg) => self.join_chord(relay.clone(), prev, msg).await,
            Message::ConnectNodeSend(msg) => self.connect_node(relay.clone(), prev, msg).await,
            Message::ConnectNodeReport(msg) => self.connected_node(relay.clone(), prev, msg).await,
            Message::AlreadyConnected(msg) => {
                self.already_connected(relay.clone(), prev, msg).await
            }
            Message::FindSuccessorSend(msg) => self.find_successor(relay.clone(), prev, msg).await,
            Message::FindSuccessorReport(msg) => {
                self.found_successor(relay.clone(), prev, msg).await
            }
            Message::NotifyPredecessorSend(msg) => {
                self.notify_predecessor(relay.clone(), prev, msg).await
            }
            Message::NotifyPredecessorReport(msg) => {
                self.notified_predecessor(relay.clone(), prev, msg).await
            }
            Message::SearchVNode(msg) => self.search_vnode(relay.clone(), prev, msg).await,
            Message::FoundVNode(msg) => self.found_vnode(relay.clone(), prev, msg).await,
            Message::StoreVNode(msg) => self.store_vnode(relay.clone(), prev, msg).await,
            Message::MultiCall(msg) => {
                for message in msg.messages {
                    let payload = MessageRelay::new(
                        message.clone(),
                        &self.swarm.session(),
                        None,
                        Some(relay.to_path.clone()),
                        Some(relay.from_path.clone()),
                        relay.method.clone(),
                    )?;
                    self.handle_message_relay(payload, prev).await.unwrap_or(());
                }
                Ok(())
            }
            Message::CustomMessage(_) => Ok(()),
            x => Err(Error::MessageHandlerUnsupportMessageType(format!(
                "{:?}",
                x
            ))),
        }?;
        if let Some(cb) = &self.callback {
            let mut callback = cb.lock().await;
            callback(&relay, prev)?;
        }
        Ok(())
    }

    /// This method is required because web-sys components is not `Send`
    /// which means a listening loop cannot running concurrency.
    pub async fn listen_once(&self) -> Option<MessageRelay<Message>> {
        if let Some(relay_message) = self.swarm.poll_message().await {
            if !relay_message.verify() {
                log::error!("Cannot verify msg or it's expired: {:?}", relay_message);
            }
            let addr = relay_message.addr.into();
            if let Err(e) = self.handle_message_relay(relay_message.clone(), addr).await {
                log::error!("Error in handle_message: {}", e);
            }
            Some(relay_message)
        } else {
            None
        }
    }
}

#[cfg(not(feature = "wasm"))]
mod listener {
    use super::MessageHandler;
    use crate::types::message::MessageListener;
    use async_trait::async_trait;
    use std::sync::Arc;

    use futures_util::pin_mut;
    use futures_util::stream::StreamExt;

    #[async_trait]
    impl MessageListener for MessageHandler {
        async fn listen(self: Arc<Self>) {
            let relay_messages = self.swarm.iter_messages();
            pin_mut!(relay_messages);
            while let Some(relay_message) = relay_messages.next().await {
                if relay_message.is_expired() || !relay_message.verify() {
                    log::error!("Cannot verify msg or it's expired: {:?}", relay_message);
                    continue;
                }
                let addr = relay_message.addr.into();
                if let Err(e) = self.handle_message_relay(relay_message, addr).await {
                    log::error!("Error in handle_message: {}", e);
                    continue;
                }
            }
        }
    }
}

#[cfg(feature = "wasm")]
mod listener {
    use super::MessageHandler;
    use crate::poll;
    use crate::types::message::MessageListener;
    use async_trait::async_trait;
    use std::sync::Arc;
    use wasm_bindgen_futures::spawn_local;

    #[async_trait(?Send)]
    impl MessageListener for MessageHandler {
        async fn listen(self: Arc<Self>) {
            let handler = Arc::clone(&self);
            let func = move || {
                let handler = Arc::clone(&handler);
                spawn_local(Box::pin(async move {
                    handler.listen_once().await;
                }));
            };
            poll!(func, 200);
        }
    }
}

#[cfg(not(feature = "wasm"))]
#[cfg(test)]
mod test {
    use super::*;
    use crate::dht::PeerRing;
    use crate::ecc::SecretKey;
    use crate::message::MessageHandler;
    use crate::session::SessionManager;
    use crate::swarm::Swarm;
    use crate::swarm::TransportManager;
    use crate::types::ice_transport::IceTrickleScheme;
    use std::sync;
    use webrtc::peer_connection::sdp::sdp_type::RTCSdpType;

    use futures::lock::Mutex;
    use std::sync::Arc;
    use tokio::time::{sleep, Duration};

    #[tokio::test]
    async fn test_custom_handler() -> Result<()> {
        let stun = "stun://stun.l.google.com:19302";

        let key1 = SecretKey::random();
        let key2 = SecretKey::random();

        println!(
            "test with key1:{:?}, key2:{:?}",
            key1.address(),
            key2.address()
        );

        let dht1 = PeerRing::new(key1.address().into());
        let dht2 = PeerRing::new(key2.address().into());

        let session1 = SessionManager::new_with_seckey(&key1).unwrap();
        let session2 = SessionManager::new_with_seckey(&key2).unwrap();

        let swarm1 = Arc::new(Swarm::new(stun, key1.address(), session1.clone()));
        let swarm2 = Arc::new(Swarm::new(stun, key2.address(), session2.clone()));

        let transport1 = swarm1.new_transport().await.unwrap();
        let transport2 = swarm2.new_transport().await.unwrap();

        fn custom_handler(relay: &MessageRelay<Message>, id: Did) -> Result<()> {
            println!("{:?}, {:?}", relay, id);
            Ok(())
        }

        let scop_var: Arc<sync::Mutex<Vec<Did>>> = Arc::new(sync::Mutex::new(vec![]));

        let closure_handler = move |relay: &MessageRelay<Message>, id: Did| {
            let mut v = scop_var.lock().unwrap();
            v.push(id);
            println!("{:?}, {:?}", relay, id);
            Ok(())
        };

        let cb: CallbackFn = box custom_handler;
        let cb2: CallbackFn = box closure_handler;

        let handler1 =
            MessageHandler::new_with_callback(Arc::new(Mutex::new(dht1)), Arc::clone(&swarm1), cb);
        let handler2 =
            MessageHandler::new_with_callback(Arc::new(Mutex::new(dht2)), Arc::clone(&swarm2), cb2);

        let handshake_info1 = transport1
            .get_handshake_info(session1, RTCSdpType::Offer)
            .await?;

        let addr1 = transport2.register_remote_info(handshake_info1).await?;

        let handshake_info2 = transport2
            .get_handshake_info(session2, RTCSdpType::Answer)
            .await?;

        let addr2 = transport1.register_remote_info(handshake_info2).await?;

        assert_eq!(addr1, key1.address());
        assert_eq!(addr2, key2.address());
        let promise_1 = transport1.connect_success_promise().await?;
        let promise_2 = transport2.connect_success_promise().await?;
        promise_1.await?;
        promise_2.await?;

        swarm1
            .register(&swarm2.address(), transport1.clone())
            .await
            .unwrap();
        swarm2
            .register(&swarm1.address(), transport2.clone())
            .await
            .unwrap();

        sleep(Duration::from_millis(1000)).await;

        assert!(handler1.listen_once().await.is_some());
        assert!(handler2.listen_once().await.is_some());

        handler1
            .send_message_default(&addr2, Message::custom("Hello world"))
            .await
            .unwrap();

        assert!(handler2.listen_once().await.is_some());

        handler2
            .send_message_default(&addr1, Message::custom("Hello world"))
            .await
            .unwrap();

        assert!(handler1.listen_once().await.is_some());
        Ok(())
    }
}