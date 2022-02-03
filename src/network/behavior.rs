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

extern crate dirs;

use crate::node_manager::handlers::ART_MGR;

use crate::node_api::MESSAGE_DELIVERY;
use libp2p::gossipsub;
use libp2p::{
    floodsub::{Floodsub, FloodsubEvent},
    kad::{
        record::{store::Error, Key},
        AddProviderOk, KademliaEvent, PeerRecord, PutRecordOk, QueryId, QueryResult, Quorum,
        Record,
    },
    mdns::{Mdns, MdnsEvent},
    swarm::NetworkBehaviourEventProcess,
    NetworkBehaviour, PeerId,
};
use libp2p_kad::store::MemoryStore;
use libp2p_kad::Kademlia;
use log::{debug, error, info, warn};
use std::collections::HashSet;

// We create a custom network behaviour that combines floodsub and mDNS.
// The derive generates a delegating `NetworkBehaviour` impl which in turn
// requires the implementations of `NetworkBehaviourEventProcess` for
// the events of each behaviour.
#[derive(NetworkBehaviour)]
#[behaviour(event_process = true)]
pub struct MyBehaviour {
    gossipsub: gossipsub::Gossipsub,
    floodsub: Floodsub,
    kademlia: Kademlia<MemoryStore>,
    mdns: Mdns,
    #[behaviour(ignore)]
    response_sender: tokio::sync::mpsc::Sender<String>,
    #[behaviour(ignore)]
    response_receiver: tokio::sync::mpsc::Receiver<String>,
}

impl MyBehaviour {
    pub fn new(
        gossipsub: gossipsub::Gossipsub,
        floodsub: Floodsub,
        kademlia: Kademlia<MemoryStore>,
        mdns: Mdns,
        response_sender: tokio::sync::mpsc::Sender<String>,
        response_receiver: tokio::sync::mpsc::Receiver<String>,
    ) -> Self {
        MyBehaviour {
            gossipsub,
            floodsub,
            kademlia,
            mdns,
            response_sender,
            response_receiver,
        }
    }

    pub fn kademlia(&mut self) -> &mut Kademlia<MemoryStore> {
        &mut self.kademlia
    }

    pub fn response_sender(&mut self) -> &mut tokio::sync::mpsc::Sender<String> {
        &mut self.response_sender
    }

    pub fn response_receiver(&mut self) -> &mut tokio::sync::mpsc::Receiver<String> {
        &mut self.response_receiver
    }

    pub fn gossipsub_mut(&mut self) -> &mut gossipsub::Gossipsub {
        &mut self.gossipsub
    }

    pub fn floodsub_mut(&mut self) -> &mut Floodsub {
        &mut self.floodsub
    }

    pub async fn lookup_blob(&mut self, hash: String) {
        let num = std::num::NonZeroUsize::new(2)
            .ok_or(Error::ValueTooLarge)
            .unwrap();
        self.kademlia.get_record(&Key::new(&hash), Quorum::N(num));
    }

    pub async fn list_peers(&mut self, peer_id: PeerId) {
        self.kademlia.get_closest_peers(peer_id);
    }

    pub async fn list_peers_cmd(&mut self) {
        match get_peers(&mut self.mdns) {
            Ok(val) => info!("Peers are : {}", val),
            Err(e) => error!("failed to get peers connected: {:?}", e),
        }
    }

    pub fn advertise_blob(&mut self, hash: String, value: Vec<u8>) -> Result<QueryId, Error> {
        let num = std::num::NonZeroUsize::new(2).ok_or(Error::ValueTooLarge)?;
        self.kademlia.put_record(Record::new(Key::new(&hash), value), Quorum::N(num))
    }

    fn log_kademlia_event(&mut self, message: &KademliaEvent) {
        if let KademliaEvent::OutboundQueryCompleted {
            result,
            id: query_id,
            ..
        } = message
        {
            debug!("Received query result for Kademlia query id {:?}", query_id);
            match result {
                QueryResult::GetProviders(Ok(ok)) => {
                    for peer in ok.providers.clone() {
                        debug!(target: "pyrsia_node_comms",
                        "Peer {:?} provides key {:?}",
                        peer,
                        std::str::from_utf8(ok.key.as_ref()).unwrap()
                        );
                    }
                }
                QueryResult::GetClosestPeers(Ok(ref ok)) => {
                    info!("GetClosestPeers result {:?}", ok.peers);
                    let connected_peers = itertools::join(ok.peers.clone(), ",");
                    // TODO This should use MessageDelivery. See issue https://github.com/pyrsia/pyrsia/issues/317
                    respond_send(self.response_sender.clone(), connected_peers);
                }
                QueryResult::GetProviders(Err(err)) => {
                    error!(target: "pyrsia_node_comms", "Failed to get providers: {:?}", err);
                }
                QueryResult::GetRecord(Ok(ok)) => {
                    for PeerRecord {
                        record: Record { key, value, .. },
                        ..
                    } in ok.records.clone()
                    {
                        debug!(target: "pyrsia_node_comms",
                            "Got record {:?} {:?}",
                            std::str::from_utf8(key.as_ref()).unwrap(),
                            std::str::from_utf8(&value).unwrap(),
                        );
                    }
                }
                QueryResult::GetRecord(Err(err)) => {
                    error!(target: "pyrsia_node_comms","Failed to get record: {:?}", err);
                }
                QueryResult::PutRecord(Ok(PutRecordOk { key })) => {
                    info!(target: "pyrsia_node_comms",
                        "Successfully put record {:?}",
                        std::str::from_utf8(key.as_ref()).unwrap()
                    );
                }
                QueryResult::PutRecord(Err(err)) => {
                    error!(target: "pyrsia_node_comms","Failed to put record: {:?}", err);
                }
                QueryResult::StartProviding(Ok(AddProviderOk { key })) => {
                    debug!(target: "pyrsia_node_comms",
                        "Successfully put provider record {:?}",
                        std::str::from_utf8(key.as_ref()).unwrap()
                    );
                }
                QueryResult::StartProviding(Err(err)) => {
                    error!(target: "pyrsia_node_comms","Failed to put provider record: {:?}", err);
                }
                _ => {
                    warn!("")
                }
            }
        }
    }
}

impl NetworkBehaviourEventProcess<gossipsub::GossipsubEvent> for MyBehaviour {
    // Called when `gossipsub` produces an event.
    fn inject_event(&mut self, message: gossipsub::GossipsubEvent) {
        if let gossipsub::GossipsubEvent::Message {
            propagation_source,
            message_id,
            message,
        } = message
        {
            let msg_data: String = String::from_utf8(message.data).unwrap();
            if msg_data.starts_with("magnet:") {
                // Synapse RPC Integration point
                info!("Start downloading {}", msg_data);
                let server = "ws://localhost:8412/";
                let pass = "donthackme";
                let download_dir = ART_MGR
                    .repository_path
                    .clone()
                    .into_os_string()
                    .into_string()
                    .unwrap();
                let directory: Option<&str> = Some(&download_dir);
                let files: Vec<&str> = vec![msg_data.as_str()];
                let r = super::torrent::add_torrent(server, pass, directory, files);
                match futures::executor::block_on(r) {
                    Err(e) => info!("Error: {}", e),
                    _ => info!("Added magnet {}", msg_data),
                };
                // This should kick-off the download
            } else {
                info!(
                    "Got message: {} with id: {} from peer: {:?}",
                    msg_data, message_id, propagation_source
                );
            }
        }
    }
}

impl NetworkBehaviourEventProcess<FloodsubEvent> for MyBehaviour {
    // Called when `floodsub` produces an event.
    fn inject_event(&mut self, message: FloodsubEvent) {
        if let FloodsubEvent::Message(message) = message {
            info!(target: "pyrsia_node_comms",
                "Received: '{:?}' from {:?}",
                String::from_utf8_lossy(&message.data),
                message.source
            );
        }
    }
}

impl NetworkBehaviourEventProcess<MdnsEvent> for MyBehaviour {
    // Called when `mdns` produces an event.
    fn inject_event(&mut self, event: MdnsEvent) {
        match event {
            MdnsEvent::Discovered(list) => {
                for (peer, multiaddr) in list {
                    self.floodsub.add_node_to_partial_view(peer);
                    self.kademlia.add_address(&peer, multiaddr);
                }
            }
            MdnsEvent::Expired(list) => {
                for (peer, multiaddr) in list {
                    if !self.mdns.has_node(&peer) {
                        self.floodsub.remove_node_from_partial_view(&peer);
                        self.kademlia.remove_address(&peer, &multiaddr);
                    }
                }
            }
        }
    }
}

impl NetworkBehaviourEventProcess<KademliaEvent> for MyBehaviour {
    // Called when `kademlia` produces an event.
    fn inject_event(&mut self, event: KademliaEvent) {
        debug!("Received event: {:?}", &event);
        self.log_kademlia_event(&event);
        if let KademliaEvent::OutboundQueryCompleted { id, result, .. } = event {
            MESSAGE_DELIVERY.deliver(id, result);
        };
    }
}
pub fn respond_send(response_sender: tokio::sync::mpsc::Sender<String>, response: String) {
    tokio::spawn(async move {
        match response_sender.send(response).await {
            Ok(_) => debug!("response for list_peers sent"),
            Err(_) => error!("failed to send response"),
        }
    });
}
pub fn get_peers(mdns: &mut Mdns) -> Result<String, Error> {
    let nodes = mdns.discovered_nodes();
    let mut unique_peers = HashSet::new();
    for peer in nodes {
        unique_peers.insert(peer);
    }
    let connected_peers = itertools::join(&unique_peers, ", ");
    Ok(connected_peers)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::node_api::{LOCAL_PEER_ID, SWARM_PROXY};
    use std::time::Duration;

    #[test]
    pub fn inject_kademlia_event() {
        let key = Key::new(&LOCAL_PEER_ID.to_bytes());

        // Kademlia won't let us create a QueryId, so we cannot create our own events. We have to ask Kademlia to create an event.
        let query_id = SWARM_PROXY.behaviour_mut(|b| b.kademlia().get_record(&key, Quorum::One));
        // Expect that Kademlia will call inject_event
        MESSAGE_DELIVERY
            .receive(query_id, Duration::from_secs(2))
            .expect("the message should be successfully received.");
    }
}
