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
extern crate libp2p;
extern crate libp2p_kad;
extern crate std;

use std::borrow::BorrowMut;
use std::io::Error;
use std::sync::atomic::{AtomicU16, AtomicU32};
use std::sync::Arc;

use crate::network::message_delivery::MessageDelivery;
use crate::node_api::{SWARM_PROXY, TOKIO_RUNTIME};
use futures::StreamExt;
use libp2p::core::connection::ListenerId;
use libp2p::core::network::NetworkInfo;
use libp2p::swarm::dial_opts::DialOpts;
use libp2p::swarm::{AddAddressResult, AddressScore, DialError, NetworkBehaviour, SwarmEvent};
use libp2p::{Multiaddr, PeerId, Swarm, TransportError};
use libp2p_kad::{QueryId, Quorum, Record};
use log::{debug, info, trace, warn};
use tokio::sync::mpsc::{channel, Receiver, Sender};
use tokio::sync::Mutex;

struct EventLoopControl {
    shutdown_requested: bool,
}

impl EventLoopControl {
    fn default() -> EventLoopControl {
        EventLoopControl {
            shutdown_requested: false,
        }
    }
}

/// A thread safe proxy to insure that only one thread at a time is making a call to the swarm. It uses internal mutability so that the caller of this struct's methods can use an immutable reference.
pub struct SwarmThreadSafeProxy<T: NetworkBehaviour> {
    mutex: Mutex<Swarm<T>>,
    event_loop_control: Arc<Mutex<Option<EventLoopControl>>>,
    kademlia_sequence_number: AtomicU32,
    kademlia_query_id_delivery: MessageDelivery<u32, QueryId>,
    kademlia_request_sender: Sender<KademliaRequest>,
    kademlia_request_receiver: Receiver<KademliaRequest>,
}

const KADEMLIA_REQUEST_CHANNEL_CAPACITY: usize = 100;

impl<T: NetworkBehaviour> SwarmThreadSafeProxy<T> {
    pub fn new(swarm: Swarm<T>) -> SwarmThreadSafeProxy<T> {
        let (kademlia_request_sender, kademlia_request_receiver) =
            channel(KADEMLIA_REQUEST_CHANNEL_CAPACITY);
        SwarmThreadSafeProxy {
            mutex: Mutex::new(swarm),
            event_loop_control: Arc::new(Mutex::new(None)),
            kademlia_sequence_number: AtomicU32::new(0),
            kademlia_query_id_delivery: MessageDelivery::default(),
            kademlia_request_sender,
            kademlia_request_receiver,
        }
    }

    /// If the swarm event loop is not running spawn a thread to run it. This always returns immediately.
    ///
    /// This is intended to be called from unit tests.
    pub async fn start_event_loop_using_other_thread(&self) {
        if self.create_event_context().await {
            let control = self.event_loop_control.clone();
            TOKIO_RUNTIME.spawn(async {
                run_event_loop(control).await;
            });
        }
    }

    async fn create_event_context(&self) -> bool {
        let mut lock = self.event_loop_control.lock().await;
        if (*lock).is_some() {
            debug!("start_event_loop_using_other_thread was called while the event loop was already running");
            false
        } else {
            *lock = Some(EventLoopControl::default());
            info!("Event loop started.");
            true
        }
    }

    #[cfg(test)] // Currently used only to support testing
    async fn is_event_loop_running(&self) -> bool {
        let lock = self.event_loop_control.lock().await;
        let result = (*lock).is_some();
        result
    }

    #[cfg(test)] // Currently used only to support testing
    /// Request the event loop to stop. This sets a flag preventing more event loop iterations. It does not immediately stop or interrupt anything.
    pub async fn request_event_loop_shutdown(&self) {
        let mut guard = self.event_loop_control.lock().await;
        let control_option = guard.borrow_mut();
        if control_option.is_some() {
            control_option.as_mut().unwrap().shutdown_requested = true;
        } else {
            warn!("request_event_loop_shutdown called when event loop is not running")
        }
    }

    pub async fn network_info(&self) -> NetworkInfo {
        trace!("network_info: entering");
        let result = self.mutex.lock().await.network_info();
        trace!("network_info: exiting");
        result
    }

    pub async fn listen_on(&self, address: Multiaddr) -> Result<ListenerId, TransportError<Error>> {
        trace!("listen_on: entering");
        let result = self.mutex.lock().await.listen_on(address);
        trace!("listen_on: exiting");
        result
    }

    pub async fn remove_listener(&self, id: ListenerId) -> bool {
        trace!("remove_listener: entering");
        let result = self.mutex.lock().await.remove_listener(id);
        trace!("remove_listener: exiting");
        result
    }

    pub async fn dial(&self, opts: impl Into<DialOpts>) -> Result<(), DialError> {
        trace!("dial: entering");
        let result = self.mutex.lock().await.dial(opts);
        trace!("dial: exiting");
        result
    }

    pub async fn local_peer_id(&self) -> PeerId {
        trace!("local_peer_id: entering");
        let result = *(*self.mutex.lock().await).local_peer_id();
        trace!("local_peer_id: exiting");
        result
    }

    pub async fn add_external_addresses(&self, a: Multiaddr, s: AddressScore) -> AddAddressResult {
        trace!("add_external_addresses: entering");
        let result = (*self.mutex.lock().await)
            .borrow_mut()
            .add_external_address(a, s);
        trace!("add_external_addresses: exiting");
        result
    }

    pub async fn remove_external_addresses(&self, a: &Multiaddr) -> bool {
        trace!("remove_external_addresses: entering");
        let result = (*self.mutex.lock().await)
            .borrow_mut()
            .remove_external_address(a);
        trace!("remove_external_addresses: exiting");
        result
    }

    pub async fn ban_peer_id(&self, peer_id: PeerId) {
        trace!("ban_peer_id: entering");
        let result = (*self.mutex.lock().await).borrow_mut().ban_peer_id(peer_id);
        trace!("ban_peer_id: exiting");
        result
    }

    pub async fn unban_peer_id(&self, peer_id: PeerId) {
        trace!("unban_peer_id: entering");
        let result = (*self.mutex.lock().await)
            .borrow_mut()
            .unban_peer_id(peer_id);
        trace!("unban_peer_id: exiting");
        result
    }

    #[allow(clippy::result_unit_err)] // A result that returns a unit error is a requirement inherited from the underlying method.
    pub async fn disconnect_peer_id(&self, peer_id: PeerId) -> Result<(), ()> {
        trace!("disconnect_peer_id: entering");
        let result = (*self.mutex.lock().await)
            .borrow_mut()
            .disconnect_peer_id(peer_id);
        trace!("disconnect_peer_id: exiting");
        result
    }

    pub async fn is_connected(&self, peer_id: &PeerId) -> bool {
        trace!("is_connected: entering");
        let result = self.mutex.lock().await.is_connected(peer_id);
        trace!("is_connected: exiting");
        result
    }

    pub async fn with_behaviour<U, V>(&self, value: V, f: fn((V, &T)) -> U) -> U {
        trace!("with_behaviour: entering");
        let result = f((value, self.mutex.lock().await.behaviour()));
        trace!("with_behaviour: exiting");
        result
    }

    pub async fn with_behaviour_mut<U, V>(&self, value: V, f: fn((V, &mut T)) -> U) -> U {
        trace!("with_behaviour_mut: entering");
        let result = f((
            value,
            (*self.mutex.lock().await).borrow_mut().behaviour_mut(),
        ));
        trace!("with_behaviour_mut: exiting");
        result
    }

    async fn process_next_event(&self) {
        debug!("waiting for next SwarmEvent");
        let swarm_event = (*self.mutex.lock().await)
            .borrow_mut()
            .select_next_some()
            .await;
        debug!("Processing swarm event ");
        match swarm_event {
            SwarmEvent::Behaviour(_behaviour) => {
                debug!("SwarmEvent::Behaviour");
            }
            SwarmEvent::ConnectionEstablished { .. } => {}
            SwarmEvent::ConnectionClosed { .. } => {
                warn!("connection closed")
            }
            SwarmEvent::IncomingConnection { .. } => {}
            SwarmEvent::IncomingConnectionError { .. } => {}
            SwarmEvent::OutgoingConnectionError { .. } => {}
            SwarmEvent::BannedPeer { .. } => {}
            SwarmEvent::NewListenAddr { .. } => {}
            SwarmEvent::ExpiredListenAddr { .. } => {}
            SwarmEvent::ListenerClosed { .. } => {}
            SwarmEvent::ListenerError { .. } => {}
            SwarmEvent::Dialing(_) => {}
        }
    }
}

async fn run_event_loop(control: Arc<Mutex<Option<EventLoopControl>>>) {
    debug!("Running event loop");
    while !control.lock().await.as_ref().unwrap().shutdown_requested {
        SWARM_PROXY.process_next_event().await;
        tokio::task::yield_now().await;
    }
    cleanup_for_event_loop_exit(&control).await;
}

async fn cleanup_for_event_loop_exit(control: &Arc<Mutex<Option<EventLoopControl>>>) {
    let mut lock = control.lock().await;
    *lock = None;
    info!("Exiting event loop");
}

enum KademliaRequest {
    GetClosestPeers(libp2p_kad::kbucket::Key<PeerId>),
    GetClosestLocalPeers(libp2p_kad::kbucket::Key<PeerId>),
    GetRecord {
        key: libp2p_kad::record::Key,
        quorum: Quorum,
    },
    PutRecord {
        record: Record,
        quorum: Quorum,
    },
}

#[cfg(test)]
mod tests {
    use futures::executor::block_on;
    use libp2p::identity;
    use libp2p::swarm::DummyBehaviour;
    use std::time::Duration;

    use super::*;

    fn swarm_proxy_for_test() -> SwarmThreadSafeProxy<DummyBehaviour> {
        let local_key = identity::Keypair::generate_ed25519();
        let local_peer_id = PeerId::from(local_key.public());
        let transport = block_on(libp2p::development_transport(local_key)).unwrap();
        let behaviour = DummyBehaviour::default();
        let swarm = Swarm::new(transport, behaviour, local_peer_id);
        SwarmThreadSafeProxy::new(swarm)
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 1)]
    async fn start_and_stop_event_loop() {
        let proxy = swarm_proxy_for_test();
        assert!(!proxy.is_event_loop_running().await);
        proxy.start_event_loop_using_other_thread().await;
        assert!(proxy.is_event_loop_running().await);

        // Verify that we can access behavior when the event loop is running
        let mut success = false;
        let success_ptr = &mut success;
        proxy
            .with_behaviour(success_ptr, |arg| {
                let (success_ptr, _behaviour) = arg;
                *success_ptr = true;
            })
            .await;
        assert!(
            success,
            "success should be true if with_behaviour called its function arg"
        );

        // shut down the event loop
        proxy.request_event_loop_shutdown().await;
        tokio::time::sleep(Duration::from_millis(10)).await;
        assert!(!proxy.is_event_loop_running().await);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 1)]
    async fn start_and_stop_shared_event_loop() {
        let proxy = &*SWARM_PROXY;
        assert!(!proxy.is_event_loop_running().await);
        proxy.start_event_loop_using_other_thread().await;
        assert!(proxy.is_event_loop_running().await);

        // Verify that we can access behavior when the event loop is running
        let mut success = false;
        let success_ptr = &mut success;
        proxy
            .with_behaviour(success_ptr, |arg| {
                let (success_ptr, _behaviour) = arg;
                *success_ptr = true;
            })
            .await;
        assert!(
            success,
            "success should be true if with_behaviour called its function arg"
        );

        // shut down the event loop
        proxy.request_event_loop_shutdown().await;
        tokio::time::sleep(Duration::from_millis(10)).await;
        assert!(!proxy.is_event_loop_running().await);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 1)]
    async fn with_behaviour_test() {
        let proxy = swarm_proxy_for_test();
        let mut success = false;
        let success_ptr = &mut success;
        proxy
            .with_behaviour(success_ptr, |arg| {
                let (success_ptr, _behaviour) = arg;
                *success_ptr = true;
            })
            .await;
        assert!(
            success,
            "success should be true if with_behaviour called its function arg"
        );

        let mut success = false;
        let success_ptr = &mut success;
        proxy
            .with_behaviour_mut(success_ptr, |arg| {
                let (success_ptr, _behaviour) = arg;
                *success_ptr = true;
            })
            .await;
        assert!(
            success,
            "success should be true if with_behaviour_mut called its function arg"
        );
    }

    const MH_IDENTITY: u8 = 0x00u8; // Code indicating an identity hash in a multihash
    const PEER_HASH_LENGTH: u8 = 0x24; // length of a peer ID

    #[tokio::test(flavor = "multi_thread", worker_threads = 1)]
    async fn local_peer_id_test() {
        let proxy = swarm_proxy_for_test();
        let peer_id = proxy.local_peer_id().await;
        let peer_bytes = peer_id.to_bytes();
        assert_eq!(
            MH_IDENTITY, peer_bytes[0],
            "Type of hash for peer from DummyBehavior"
        );
        assert_eq!(
            PEER_HASH_LENGTH, peer_bytes[1],
            "Expected length of peer id from DummyBehavior"
        )
    }
}
