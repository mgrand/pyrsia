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
use dashmap::DashMap;
use log::debug;
use std::fmt::Debug;
use std::hash::Hash;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

const INITIAL_MAP_CAPACITY: usize = 23;

/// This is used to facility the delivery of events or messages from one thread to another thread matched a common ID such as a Kademlia query Id.
/// More complete documentation is at https://github.com/pyrsia/pyrsia/issues/306#issuecomment-1026958611
///
/// Type Parameters:
/// * I — The type of value used to match messages to recipients
/// * M — The type of the messages
#[derive(Debug)]
pub struct MessageDelivery<I: Eq + Hash, M: Debug> {
    id_message_map: DashMap<I, MessageEnvelope<M>>,
}

//TODO add a mechanism to evict old messages. Messages older than a few seconds will probably never be received, so we need to clean-up. see https://github.com/pyrsia/pyrsia/issues/318
impl<I: Eq + Hash + Clone + Debug, M: Debug> MessageDelivery<I, M> {
    /// Create a MessageDelivery struct
    pub fn default() -> MessageDelivery<I, M> {
        MessageDelivery {
            id_message_map: DashMap::with_capacity(INITIAL_MAP_CAPACITY),
        }
    }

    /// Make a message available for delivery to the thread that is or will be waiting for a message associated with the given id
    pub fn deliver(&self, id: I, message: M) {
        debug!("deliver(id={:?}, message={:?}", id, message);
        let delivery_envelope = MessageEnvelope {
            arc: None,
            message: Some(message),
        };
        if let Some(receiving_envelope) = self.id_message_map.insert(id, delivery_envelope) {
            receiving_envelope.notify_all();
        }
    }

    /// Receive a message associated with the given IT that is already available or wait for it.
    pub fn receive(&self, id: I, timeout_duration: Duration) -> Result<M> {
        debug!("receive(id={:?}", id);
        let arc = Arc::new((Mutex::new(false), Condvar::new()));
        let receiving_envelope = MessageEnvelope {
            arc: Some(arc.clone()),
            message: None,
        };
        match self.id_message_map.insert(id.clone(), receiving_envelope) {
            Some(MessageEnvelope {
                message: Some(msg), ..
            }) => {
                self.id_message_map.remove(&id);
                Ok(msg)
            }
            Some(MessageEnvelope { message: None, .. }) => {
                bail!("Received message envelope with no message for {:?}", id)
            }
            None => {
                Self::wait_for_delivery(arc, timeout_duration, &id)?;
                match self.id_message_map.remove(&id) {
                    Some((
                        _,
                        MessageEnvelope {
                            message: Some(msg), ..
                        },
                    )) => Ok(msg),
                    Some((_, MessageEnvelope { message: None, .. })) => {
                        bail!("Received message envelope with no message for {:?}", id)
                    }
                    None => bail!("Missing message envelope for {:?}", id),
                }
            }
        }
    }

    fn wait_for_delivery(
        arc: Arc<(Mutex<bool>, Condvar)>,
        timeout_duration: Duration,
        id: &I,
    ) -> Result<()> {
        let (mutex, cvar) = &*arc;
        let mut guard = match mutex.lock() {
            Ok(guard) => guard,
            Err(error) => bail!("Error for {:?} unlocking mutex for receive: {}", id,  error),
        };
        while !*guard {
            guard = match cvar.wait_timeout(guard, timeout_duration) {
                Ok((guard, wait_timeout_result)) => {
                    if wait_timeout_result.timed_out() {
                        bail!("MessageDelivery::receive timed out for {:?}", id);
                    } else {
                        guard
                    }
                }
                Err(error) => bail!("Error unlocking mutex for receive: {}", error),
            }
        }
        Ok(())
    }
}

#[derive(Debug)]
struct MessageEnvelope<M: Debug> {
    arc: Option<Arc<(Mutex<bool>, Condvar)>>,
    message: Option<M>,
}

impl<M: Debug> MessageEnvelope<M> {
    pub fn notify_all(&self) {
        match self.arc {
            Some(ref arc_value) => {
                let (mutex, cvar) = &**arc_value;
                let mut guard = mutex.lock().expect("Error accessing mutex");
                *guard = true;
                cvar.notify_all()
            }
            None => panic!("notify_all called on a MessageEnvelope that has no Condvar"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use log::info;
    use std::{thread, time};

    #[derive(Debug)]
    enum FakeTestEvent {
        Start,
        Stop,
    }

    #[test]
    fn deliver_then_receive() -> Result<()> {
        let message_delivery: MessageDelivery<u32, FakeTestEvent> = MessageDelivery::default();
        let id = 37;
        message_delivery.deliver(id, FakeTestEvent::Start);
        let actual_message = message_delivery.receive(id, Duration::from_millis(2))?;
        match actual_message {
            FakeTestEvent::Start => Ok(()),
            _ => bail!("Expected FakeTestEvent::Start but got {:?}", actual_message),
        }
    }

    const RECEIVE_BEFORE_DELIVER_ID: i32 = 41;

    #[test]
    fn receive_before_deliver() -> Result<()> {
        let arc: Arc<MessageDelivery<i32, FakeTestEvent>> = Arc::new(MessageDelivery::default());
        let arc2 = arc.clone();
        thread::spawn(move || {
            info!("receive_before_deliver: Waiting for receipt attempt to begin. Delvery will be after");
            let message_delivery = &*arc2;
            while message_delivery
                .id_message_map
                .get(&RECEIVE_BEFORE_DELIVER_ID)
                .is_none()
            {
                thread::sleep(time::Duration::from_millis(1));
            }
            info!("receive_before_deliver: delivering");
            message_delivery.deliver(RECEIVE_BEFORE_DELIVER_ID, FakeTestEvent::Stop);
        });
        let message_delivery = &*arc;
        let actual_message =
            message_delivery.receive(RECEIVE_BEFORE_DELIVER_ID, Duration::from_millis(2))?;
        match actual_message {
            FakeTestEvent::Stop => Ok(()),
            _ => bail!("Expected FakeTestEvent::Stop but got {:?}", actual_message),
        }
    }
}
