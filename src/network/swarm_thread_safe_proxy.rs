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

use crate::block_chain::block_chain;
use crate::block_chain::block_chain::Blockchain;
use crate::node_api::{FLOODSUB_TOPIC, GOSSIP_TOPIC, LOCAL_PEER_ID, PYRSIA_API_ADDRESS, SWARM_PROXY};
use libp2p::core::connection::ListenerId;
use libp2p::core::network::NetworkInfo;
use libp2p::swarm::dial_opts::DialOpts;
use libp2p::swarm::{AddAddressResult, AddressScore, DialError, NetworkBehaviour, SwarmEvent};
use libp2p::{Multiaddr, PeerId, Swarm, TransportError};
use log::{debug, error, info};
use std::borrow::BorrowMut;
use std::cell::RefCell;
use std::io::Error;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::thread::ThreadId;
use tokio::io;
use tokio::io::{AsyncBufReadExt, BufReader, Lines, Stdin};
use tokio::sync::mpsc;
use tokio::sync::mpsc::{Receiver, Sender};
use warp::Filter;
use crate::docker::error_util::custom_recover;
use crate::docker::v2::handlers::blobs::GetBlobsHandle;
use crate::docker::v2::routes::make_docker_routes;
use crate::logging::http;
use crate::node_api::routes::make_node_routes;

struct PollingLoopControl {
    polling_loop_thread: Option<ThreadId>,
    shutdown_requested: bool,
}

impl PollingLoopControl {
    fn new(thread_id: ThreadId) -> PollingLoopControl {
        PollingLoopControl {
            polling_loop_thread: Some(thread_id),
            shutdown_requested: false,
        }
    }
}

/// A thread safe proxy to insure that only one thread at a time is making a call to the swarm. It uses internal mutability so that the caller of this struct's methods can use an immutable reference.
pub struct SwarmThreadSafeProxy<T: NetworkBehaviour> {
    mutex: Mutex<RefCell<Swarm<T>>>,
    polling_loop_control: Arc<Mutex<RefCell<Option<PollingLoopControl>>>>,
}

impl<T: NetworkBehaviour> SwarmThreadSafeProxy<T> {
    pub fn new(swarm: Swarm<T>) -> SwarmThreadSafeProxy<T> {
        SwarmThreadSafeProxy {
            mutex: Mutex::new(RefCell::new(swarm)),
            polling_loop_control: Arc::new(Mutex::new(RefCell::new(None))),
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
            let mut guard = self
                .polling_loop_control
                .lock()
                .expect("If the mutex is broken, panic");
            let cell = guard.borrow_mut();
            if (**cell).get_mut().is_some() {
                return error!("start_polling_loop_using_my_thread was called while the polling loop was already running");
            }
            cell.replace(Some(PollingLoopControl::new(thread::current().id())));
        }
        run_polling_loop(self.polling_loop_control.clone());
    }

    /// If the swarm polling loop is not running spawn a thread to run it. This always returns immediately.
    ///
    /// This is intended to be called from unit tests.
    pub fn start_polling_loop_using_other_thread(&self) {
        let mut guard = self
            .polling_loop_control
            .lock()
            .expect("If the mutex is broken, panic");
        let cell = guard.borrow_mut();
        if (**cell).borrow().is_some() {
            return debug!("start_polling_loop_using_other_thread was called while the polling loop was already running");
        }
        let control = self.polling_loop_control.clone();
        let polling_thread_id = thread::spawn(move || {
            run_polling_loop(control);
        })
        .thread()
        .id();
        cell.replace(Some(PollingLoopControl {
            polling_loop_thread: Some(polling_thread_id),
            shutdown_requested: false,
        }));
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

    pub fn listen_on(&self, address: Multiaddr) -> Result<ListenerId, TransportError<Error>> {
        (*self.ref_cell()).borrow_mut().listen_on(address)
    }

    pub fn remove_listener(&self, id: ListenerId) -> bool {
        (*self.ref_cell()).borrow_mut().remove_listener(id)
    }

    pub fn dial(&self, opts: impl Into<DialOpts>) -> Result<(), DialError> {
        (*self.ref_cell()).borrow_mut().dial(opts)
    }

    pub fn local_peer_id(&self) -> PeerId {
        *(*self.ref_cell()).borrow().local_peer_id()
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

    #[allow(clippy::result_unit_err)] // A result that returns a unit error is a requirement inherited from the underlying method.
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

/// Return true if there is a pending request to shut down the polling loop.
fn shutdown_requested(control: &Arc<Mutex<RefCell<Option<PollingLoopControl>>>>) -> bool {
    match *(control.lock().expect("mutex is OK").borrow()) {
        Some(PollingLoopControl {
            shutdown_requested: flag,
            ..
        }) => flag,
        None => true,
    }
}

struct PollingContext {
    bc: Arc<Mutex<Blockchain>>,
    stdin: Lines<BufReader<Stdin>>,
    tx: Sender<String>,
    rx: Receiver<String>,
    blobs_need_hash: GetBlobsHandle,
    blocks_get_rx_from_api: Receiver<String>,
    blocks_get_tx_answer_to_api: Sender<Blockchain>,
}

impl PollingContext {
    fn default() -> PollingContext {
        let raw_chain = block_chain::Blockchain::new();
        let bc = Arc::new(Mutex::new(raw_chain));

        // Read full lines from stdin
        let mut stdin = io::BufReader::new(io::stdin()).lines();

        // TODO Because we have otherwise unrelated things communicating through channels, there is much
        let (tx, mut rx) = mpsc::channel(32);
        let mut blobs_need_hash = GetBlobsHandle::new();
        let b1 = blobs_need_hash.clone();
        //docker node specific tx
        let tx1 = tx.clone();

        // We need to have two channels (to seperate the handling)
        // 1. API to main
        // 2. main to API
        let (blocks_get_tx_to_main, mut blocks_get_rx_from_api) = mpsc::channel(32); // Request Channel
        let (blocks_get_tx_answer_to_api, blocks_get_rx_answers_from_main) = mpsc::channel(32); // Response Channel

        let docker_routes = make_docker_routes(b1, tx1);
        let routes = docker_routes.or(make_node_routes(
            blocks_get_tx_to_main.clone(),
            blocks_get_rx_answers_from_main,
        ));

        let (addr, server) = warp::serve(
            routes
                .and(http::log_headers())
                .recover(custom_recover)
                .with(warp::log("pyrsia_registry")),
        )
            .bind_ephemeral(PYRSIA_API_ADDRESS.read().unwrap().clone());

        info!(
        "Pyrsia Docker Node is now running on port {}:{}!",
        addr.ip(),
        addr.port()
    );

        tokio::spawn(server);
        let tx4 = tx.clone();

        PollingContext { bc, stdin, tx, rx, blobs_need_hash, blocks_get_rx_from_api, blocks_get_tx_answer_to_api }
    }
}

fn run_polling_loop(control: Arc<Mutex<RefCell<Option<PollingLoopControl>>>>) {
    debug!(
        "Running polling loop in thread {:?}",
        control
            .lock()
            .expect("If the mutex is broken, panic")
            .borrow()
            .as_ref()
            .unwrap()
            .polling_loop_thread
            .unwrap()
    );
    let mut polling_context = PollingContext::default();
    while !shutdown_requested(&control) {
        polling_logic(&mut polling_context);
    }
    cleanup_for_polling_loop_exit(&control);
}

fn cleanup_for_polling_loop_exit(control: &Arc<Mutex<RefCell<Option<PollingLoopControl>>>>) {
    let mut guard = control.lock().expect("If the mutex is broken, panic");
    let cell = guard.borrow_mut();
    let old_control = cell.replace(None);
    debug!(
        "Thread exited polling loop: {:?}",
        old_control.as_ref().unwrap().polling_loop_thread.unwrap()
    );
}

fn polling_logic(polling_context: &mut PollingContext) {
    let evt = {
        tokio::select! {
            line = polling_context.stdin.next_line() => Some(EventType::Input(line.expect("can get line").expect("can read line from stdin"))),
            message = polling_context.rx.recv() => Some(EventType::Message(message.expect("message exists"))),

            new_hash = polling_context.blobs_need_hash.select_next_some() => {
                debug!("Looking for {}", new_hash);
                SWARM_PROXY.with_behaviour_mut(new_hash, |arg| {
                    let (new_hash, behavior) = arg;
                    behavior.lookup_blob(new_hash).await
                });
                None
            },

            event = SWARM_PROXY.select_next_some() =>  {
                !debug!("Received swarm event {:?}", event);
                if let SwarmEvent::NewListenAddr { address, .. } = event {
                    info!("Listening on {:?}", address);
                }

                //SwarmEvent::Behaviour(e) => panic!("Unexpected event: {:?}", e),
                None
            },
            get_blocks_request_input = polling_context.blocks_get_rx_from_api.recv() => //BlockChain::handle_api_requests(),

            // Channels are only one type (both tx and rx must match)
            // We need to have two channel for each API call to send and receive
            // The request and the response.

            {
                info!("Processing 'GET /blocks' {} request", get_blocks_request_input.unwrap());
                let bc1 = polling_context.bc.clone();
                let block_chaing_instance = bc1.lock().await.clone();
                polling_context.blocks_get_tx_answer_to_api.send(block_chaing_instance).await.expect("send to work");

                None
            }
        }
    };

    if let Some(event) = evt {
        match event {
            EventType::Response(resp) => {
                //here we have to manage which events to publish to floodsub
                SWARM_PROXY.with_behaviour_mut(resp, |arg| {
                    let (resp, behavior) = arg;
                    behavior
                        .floodsub_mut()
                        .publish(*FLOODSUB_TOPIC, resp.as_bytes())
                });
            }
            EventType::Input(line) => match line.as_str() {
                "peers" => SWARM_PROXY.with_behaviour_mut((), |arg| {
                    let (behavior) = arg;
                    behavior.list_peers_cmd().await
                }),
                cmd if cmd.starts_with("magnet:") => {
                    info!(
                        "{}",
                        SWARM_PROXY.with_behaviour_mut((), |arg| {
                            let (behavior) = arg;
                            behavior
                                .gossipsub_mut()
                                .publish(&*GOSSIP_TOPIC, cmd)
                                .unwrap()
                        })
                    )
                }
                _ => match polling_context.tx.send(line).await {
                    Ok(_) => debug!("line sent"),
                    Err(_) => error!("failed to send stdin input"),
                },
            },
            EventType::Message(message) => match message.as_str() {
                cmd if cmd.starts_with("peers") || cmd.starts_with("status") => SWARM_PROXY
                    .with_behaviour_mut((), |arg| {
                        let (behavior) = arg;
                        behavior.list_peers(*LOCAL_PEER_ID).await
                    }),
                cmd if cmd.starts_with("get_blobs") => {
                    SWARM_PROXY.with_behaviour_mut(message, |arg| {
                        let (message, behavior) = arg;
                        behavior.lookup_blob(message).await
                    })
                }
                "blocks" => { /*
                     let bc_state: Arc<_> = bc.clone();
                     let mut bc_instance: MutexGuard<_> = bc_state.lock().await;
                     let new_block =
                         bc_instance.mk_block("happy_new_block".to_string()).unwrap();

                     let new_chain = bc_instance
                         .clone()
                         .add_entry(new_block)
                         .expect("should have added");
                     *bc_instance = new_chain;
                     */
                }
                _ => info!("message received from peers: {}", message),
            },
        }
    }
}

enum EventType {
    Response(String),
    Message(String),
    Input(String),
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
