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
use std::hash::Hash;
use std::sync::{Arc, Condvar, LockResult, Mutex, MutexGuard};

const INITIAL_MAP_CAPACITY: usize = 23;

/// This is used to facility the delivery of events or messages from one thread to another thread matched a common ID such as a Kademlia query Id.
/// More complete documentation is at https://github.com/pyrsia/pyrsia/issues/306#issuecomment-1026958611
///
/// Type Parameters:
/// * I — The type of value used to match messages to recipients
/// * M — The type of the messages
pub struct MessageDelivery<I: Eq + Hash, M: Clone> {
    id_message_map: DashMap<I, MessageEnvelope<M>>,
}

impl<I: Eq + Hash + Clone, M: Clone> MessageDelivery<I, M> {
    pub fn default() -> MessageDelivery<I, M> {
        MessageDelivery {
            id_message_map: DashMap::with_capacity(INITIAL_MAP_CAPACITY),
        }
    }

    pub fn deliver(&self, id: I, message: M) {
        let delivery_envelope = MessageEnvelope {
            arc: None,
            message: Some(message),
        };
        if let Some(receiving_envelope) = self.id_message_map.insert(id, delivery_envelope) {
            receiving_envelope.notify_all();
        }
    }

    pub fn receive(&self, id: I) -> Result<M> {
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
                bail!("Received message envelope with no message")
            }
            None => {
                if let Err(error) = Self::wait_for_delivery(arc) {
                    bail!("Error locking mutex for receive: {}", error)
                }
                match self.id_message_map.remove(&id) {
                    Some((
                        _,
                        MessageEnvelope {
                            message: Some(msg), ..
                        },
                    )) => Ok(msg),
                    Some((_, MessageEnvelope { message: None, .. })) => {
                        bail!("Received message envelope with no message")
                    }
                    None => bail!("Missing message envelope"),
                }
            }
        }
    }

    fn wait_for_delivery(
        arc: Arc<(Mutex<bool>, Condvar)>,
    ) -> Result<()> {
        fn to_anyhow(r: LockResult<MutexGuard<'_, bool>>) -> Result<MutexGuard<'_, bool>> {
            match r {
                Ok(guard) => Ok(guard),
                Err(error) => bail!("Error unlocking mutex for receive: {}", error)
            }
        }
        let (mutex, cvar) = &*arc;
        let mut guard = to_anyhow(mutex.lock())?;
        while !*guard {
            guard = to_anyhow(cvar.wait(guard))?;
        }
        Ok(())
    }
}

struct MessageEnvelope<M: Clone> {
    arc: Option<Arc<(Mutex<bool>, Condvar)>>,
    message: Option<M>,
}

impl<M: Clone> MessageEnvelope<M> {
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

    #[test]
    fn placeholder_test() {
        assert!(true)
    }
}
