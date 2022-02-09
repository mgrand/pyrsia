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

use super::behavior::MyBehaviour;
use super::transport::TcpTokioTransport;
use libp2p::gossipsub::MessageId;
use libp2p::gossipsub::{GossipsubMessage, MessageAuthenticity, ValidationMode};
use libp2p::{floodsub::Floodsub, mdns::Mdns, swarm::SwarmBuilder, Swarm};
use libp2p::{gossipsub, PeerId};
use std::collections::hash_map::DefaultHasher;

use crate::network::transport;
use crate::node_api::{GOSSIP_TOPIC, LOCAL_KEY};
use crate::node_manager::handlers::{FLOODSUB_TOPIC, LOCAL_PEER_ID};
use futures::executor::block_on;
use libp2p::core::muxing::StreamMuxerBox;
use libp2p::core::transport::Boxed;
use libp2p::swarm::NetworkBehaviour;
use libp2p_kad::store::MemoryStore;
use libp2p_kad::Kademlia;
use std::hash::{Hash, Hasher};
use std::time::Duration;

/// This is the normal way that a Pyrsia node will create a swarm
pub fn default() -> Swarm<MyBehaviour> {
    // To content-address message, we can take the hash of message and use it as an ID.
    let message_id_fn = |message: &GossipsubMessage| {
        let mut s = DefaultHasher::new();
        message.data.hash(&mut s);
        MessageId::from(s.finish().to_string())
    };

    // Set a custom gossipsub
    let gossipsub_config = gossipsub::GossipsubConfigBuilder::default()
        .heartbeat_interval(Duration::from_secs(10)) // This is set to aid debugging by not cluttering the log space
        .validation_mode(ValidationMode::Strict) // This sets the kind of message validation. The default is Strict (enforce message signing)
        .message_id_fn(message_id_fn) // content-address messages. No two messages of the
        // same content will be propagated.
        .build()
        .expect("Valid config");
    // build a gossipsub network behaviour
    let mut gossipsub: gossipsub::Gossipsub = gossipsub::Gossipsub::new(
        MessageAuthenticity::Signed(LOCAL_KEY.clone()),
        gossipsub_config,
    )
    .expect("Correct configuration");

    // subscribes to our gossip topic
    gossipsub.subscribe(&*GOSSIP_TOPIC).unwrap();

    let kademlia = Kademlia::new(*LOCAL_PEER_ID, MemoryStore::new(*LOCAL_PEER_ID));
    let mdns = match block_on(Mdns::new(Default::default())) {
        Ok(mdns) => mdns,
        Err(error) => {
            panic!("Error setting up mdns: {}", error)
        }
    };
    let transport: TcpTokioTransport = transport::new_tokio_tcp_transport(&*LOCAL_KEY); // Create a tokio-based TCP transport using noise for authenticated
    let mut behaviour = MyBehaviour::new(gossipsub, Floodsub::new(*LOCAL_PEER_ID), kademlia, mdns);
    behaviour.floodsub_mut().subscribe(FLOODSUB_TOPIC.clone());
    new(transport, behaviour)
}

/// This can be used by unit tests to use alternate transport and behavior
pub fn new<T: NetworkBehaviour>(
    transport: Boxed<(PeerId, StreamMuxerBox)>,
    behaviour: T,
) -> Swarm<T> {
    SwarmBuilder::new(transport, behaviour, *LOCAL_PEER_ID)
        // We want the connection background tasks to be spawned
        // onto the tokio runtime.
        .executor(Box::new(|fut| {
            tokio::spawn(fut);
        }))
        .build()
}

#[cfg(test)]
mod tests {
    use super::*;
    use libp2p::identity;
    use libp2p::swarm::DummyBehaviour;

    #[test]
    pub fn new_test() {
        let local_key = identity::Keypair::generate_ed25519();
        let transport = block_on(libp2p::development_transport(local_key)).unwrap();
        let behaviour = DummyBehaviour::default();

        let swarm = new(transport, behaviour);
        swarm.behaviour();
        // nothing more to test other than it doesn't blow up.
    }
}
