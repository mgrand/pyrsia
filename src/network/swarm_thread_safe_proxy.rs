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

use std::borrow::{Borrow, BorrowMut};
use libp2p::core::connection::ListenerId;
use libp2p::core::network::NetworkInfo;
use libp2p::swarm::dial_opts::DialOpts;
use libp2p::swarm::{AddAddressResult, AddressScore, DialError, NetworkBehaviour};
use libp2p::{Multiaddr, PeerId, Swarm, TransportError};
use std::cell::RefCell;
use std::io::Error;
use std::sync::{Mutex, MutexGuard};
use std::thread;
use std::thread::Thread;
use log::error;

struct PollingLoopControl {
    polling_loop_thread: Option<Thread>,
    shutdown_requested: bool,
}

/// A thread safe proxy to insure that only one thread at a time is making a call to the swarm. It uses internal mutability so that the caller of this struct's methods can use an immutable reference.
pub struct SwarmThreadSafeProxy<T: NetworkBehaviour> {
    mutex: Mutex<RefCell<Swarm<T>>>,
    polling_loop_control: Mutex<RefCell<Option<PollingLoopControl>>>,
}

impl<T: NetworkBehaviour> SwarmThreadSafeProxy<T> {
    pub fn new(swarm: Swarm<T>) -> SwarmThreadSafeProxy<T> {
        SwarmThreadSafeProxy {
            mutex: Mutex::new(RefCell::new(swarm)),
            polling_loop_control: Mutex::new(RefCell::new(None))
        }
    }

    /// return true if the mutex is in a poisoned state due to a previous panic.
    pub fn is_poisoned(&self) -> bool {
        self.mutex.is_poisoned()
    }

    /// Run the swarm polling loop using the caller's thread, returning when the polling loop is stopped.
    /// If another thread is already running the loop, logs an error and returns immediately.
    ///
    /// This is intended to be called from the main thread.
    pub fn start_polling_loop_using_my_thread(&self) {
        {
            let mut cell = self.polling_loop_control.lock().expect("If the mutex is broken, panic").borrow_mut();
            if (**cell).get_mut().is_some() {
                return error!("start_polling_loop_using_my_thread was called while the polling loop was already running");
            }
            cell.replace(Some(PollingLoopControl { polling_loop_thread: Some(thread::current()), shutdown_requested: false }));
        }
        while !self.shutdown_requested() {
            polling_logic();
        }
    }

    fn shutdown_requested(&self) -> bool {
        match *(**self.polling_loop_control.lock().expect("mutex is OK").borrow()).borrow() {
            Some(PollingLoopControl{shutdown_requested: flag, ..}) => flag,
            None => true
        }
    }

    /// If the swarm polling loop is not running spawn a thread to run it. This always returns immediately.
    ///
    /// This is intented to be called from unit tests.
    pub fn start_polling_loop_using_other_thread(&self) {
        todo!()
    }

    /// Request the polling loop to stop. This sets a flag preventing more polling loop iterations. It does not immediately stop or interrupt anything.
    pub fn stop_polling_loop(&self) {
        todo!()
    }

    fn ref_cell(&self) -> MutexGuard<RefCell<Swarm<T>>> {
        self.mutex
            .lock()
            .expect("SwarmThreadSafeProxy called after a panic during a previous call!")
    }

    pub fn network_info(&self) -> NetworkInfo {
        (*self.ref_cell()).borrow().network_info()
    }

    pub fn listen_on(&self, addr: Multiaddr) -> Result<ListenerId, TransportError<Error>> {
        (*self.ref_cell()).borrow_mut().listen_on(addr)
    }

    pub fn remove_listener(&self, id: ListenerId) -> bool {
        (*self.ref_cell()).borrow_mut().remove_listener(id)
    }

    pub fn dial(&self, opts: impl Into<DialOpts>) -> Result<(), DialError> {
        (*self.ref_cell()).borrow_mut().dial(opts)
    }

    pub fn local_peer_id(&self) -> PeerId {
        (*self.ref_cell()).borrow().local_peer_id().clone()
    }

    pub fn add_external_addresses(&self, a: Multiaddr, s: AddressScore) -> AddAddressResult {
        (*self.ref_cell()).borrow_mut().add_external_address(a, s)
    }

    pub fn remove_external_addresses(&self, a: &Multiaddr) -> bool {
        (*self.ref_cell()).borrow_mut().remove_external_address(a)
    }

    pub fn ban_peer_id(&self, peer_id: PeerId) {
        (*self.ref_cell()).borrow_mut().ban_peer_id(peer_id)
    }

    pub fn unban_peer_id(&self, peer_id: PeerId) {
        (*self.ref_cell()).borrow_mut().unban_peer_id(peer_id)
    }

    pub fn disconnect_peer_id(&self, peer_id: PeerId) -> Result<(), ()> {
        (*self.ref_cell()).borrow_mut().disconnect_peer_id(peer_id)
    }

    pub fn is_connected(&self, peer_id: &PeerId) -> bool {
        (*self.ref_cell()).borrow().is_connected(peer_id)
    }

    pub fn with_behaviour<U, V>(&self, value: V, f: fn((V, &T)) -> U) -> U {
        f((value, (*self.ref_cell()).borrow().behaviour()))
    }

    pub fn with_behaviour_mut<U, V>(&self, value: V, f: fn((V, &mut T)) -> U) -> U {
        f((value, (*self.ref_cell()).borrow_mut().behaviour_mut()))
    }
}

fn polling_logic() {

}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::executor::block_on;
    use libp2p::identity;
    use libp2p::swarm::DummyBehaviour;

    fn swarm_proxy_for_test() -> SwarmThreadSafeProxy<DummyBehaviour> {
        let local_key = identity::Keypair::generate_ed25519();
        let local_peer_id = PeerId::from(local_key.public());
        let transport = block_on(libp2p::development_transport(local_key)).unwrap();
        let behaviour = DummyBehaviour::default();
        let swarm = Swarm::new(transport, behaviour, local_peer_id);
        SwarmThreadSafeProxy::new(swarm)
    }

    #[test]
    pub fn new_proxy_test() {
        let proxy = swarm_proxy_for_test();
        assert!(!proxy.is_poisoned())
    }
}
