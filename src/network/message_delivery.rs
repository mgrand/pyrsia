/*
   Copyright 2021 JFrog Ltd

   Licensed under the Apache License, Version 2.0 (the "License");
   you may not use this file except in compliance with the License.
   You may obtain a copy of the License at

       http://www.apache.org/licenses/LICENSE-2.0

   Unless required by applicable law or agreed to in writing, software
   distributed under the License is distributed on an "AS IS" BASIS,
   WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
   See the License for the specific language governing permissions and
   limitations under the License.
*/

extern crate anyhow;
extern crate core;
extern crate dashmap;
extern crate std;

use anyhow::{bail, Result};
use log::debug;
use std::collections::HashMap;
use std::fmt::Debug;
use std::hash::Hash;
use std::sync::Arc;
use tokio::sync::{Mutex, Notify};

const INITIAL_MAP_CAPACITY: usize = 23;

/// This is used to facility the delivery of events or messages from one thread to another thread matched a common ID such as a Kademlia query Id.
/// More complete documentation is at https://github.com/pyrsia/pyrsia/issues/306#issuecomment-1026958611
///
/// Type Parameters:
/// * I — The type of value used to match messages to recipients
/// * M — The type of the messages
#[derive(Debug)]
pub struct MessageDelivery<I: Eq + Hash, M: Debug> {
    id_message_map: Mutex<HashMap<I, MessageEnvelope<M>>>,
}

//TODO add a mechanism to evict old messages. Messages older than a few seconds will probably never be received, so we need to clean-up. see https://github.com/pyrsia/pyrsia/issues/318
impl<I: Eq + Hash + Clone + Debug, M: Debug> MessageDelivery<I, M> {
    /// Create a MessageDelivery struct
    pub fn default() -> MessageDelivery<I, M> {
        MessageDelivery {
            id_message_map: Mutex::new(HashMap::with_capacity(INITIAL_MAP_CAPACITY)),
        }
    }

    /// Make a message available for delivery to the thread that is or will be waiting for a message associated with the given id
    pub async fn deliver(&self, id: I, message: M) {
        debug!("deliver(id={:?}, message={:?}", id, message);
        let delivery_envelope = MessageEnvelope {
            notify: None,
            message: Some(message),
        };
        let mut lock = self.id_message_map.lock().await;
        if let Some(receive_envelope) = lock.insert(id, delivery_envelope) {
            receive_envelope.notify.expect("When we get an message envelope from receive, it is supposed to have Some(Notify).").notify_one();
        }
    }

    /// Receive a message associated with the given IT that is already available or wait for it.
    pub async fn receive(&self, id: I) -> Result<M> {
        debug!("receive(id={:?}", id);
        let notify = Arc::new(Notify::new());
        let receive_envelope = MessageEnvelope {
            notify: Some(notify.clone()),
            message: None,
        };
        let mut lock = self.id_message_map.lock().await;
        match lock.insert(id.clone(), receive_envelope) {
            Some(MessageEnvelope {
                message: Some(msg), ..
            }) => {
                lock.remove(&id);
                Ok(msg)
            }
            Some(MessageEnvelope { message: None, .. }) => {
                bail!("Received message envelope with no message for {:?}", id)
            }
            None => {
                notify.notified().await;
                match lock.remove(&id) {
                    Some(MessageEnvelope {
                        message: Some(msg), ..
                    }) => Ok(msg),
                    Some(MessageEnvelope { message: None, .. }) => {
                        bail!("Received message envelope with no message for {:?}", id)
                    }
                    None => bail!("Missing message envelope for {:?}", id),
                }
            }
        }
    }
}

#[derive(Debug)]
struct MessageEnvelope<M: Debug> {
    notify: Option<Arc<Notify>>,
    message: Option<M>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use log::info;
    use std::thread;
    use std::time::Duration;

    #[derive(Debug)]
    enum FakeTestEvent {
        Start,
        Stop,
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 1)]
    async fn deliver_then_receive() -> Result<()> {
        let message_delivery: MessageDelivery<u32, FakeTestEvent> = MessageDelivery::default();
        let id = 37;
        message_delivery.deliver(id, FakeTestEvent::Start).await;
        let actual_message = message_delivery.receive(id).await?;
        match actual_message {
            FakeTestEvent::Start => Ok(()),
            _ => bail!("Expected FakeTestEvent::Start but got {:?}", actual_message),
        }
    }

    const RECEIVE_BEFORE_DELIVER_ID: i32 = 41;

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn receive_before_deliver() -> Result<()> {
        let arc: Arc<MessageDelivery<i32, FakeTestEvent>> = Arc::new(MessageDelivery::default());
        let arc2 = arc.clone();
        thread::spawn(move || {
            info!("receive_before_deliver: Waiting for receipt attempt to begin. Delivery will be after");
            let message_delivery = &*arc2;
            tokio::time::sleep(Duration::from_millis(3));
            info!("receive_before_deliver: delivering");
            message_delivery.deliver(RECEIVE_BEFORE_DELIVER_ID, FakeTestEvent::Stop);
        });
        let message_delivery = &*arc;
        let actual_message = message_delivery.receive(RECEIVE_BEFORE_DELIVER_ID).await?;
        match actual_message {
            FakeTestEvent::Stop => Ok(()),
            _ => bail!("Expected FakeTestEvent::Stop but got {:?}", actual_message),
        }
    }
}
