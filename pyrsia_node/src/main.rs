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

extern crate clap;
extern crate futures;
extern crate libp2p;
extern crate log;
extern crate pretty_env_logger;
extern crate pyrsia;
extern crate tokio;
extern crate warp;

use pyrsia::block_chain::*;
use pyrsia::docker::error_util::*;
use pyrsia::docker::v2::handlers::blobs::GetBlobsHandle;
use pyrsia::docker::v2::routes::*;
use pyrsia::document_store::document_store::DocumentStore;
use pyrsia::document_store::document_store::IndexSpec;
use pyrsia::logging::*;
use pyrsia::network::swarm::{self};
use pyrsia::network::transport::{new_tokio_tcp_transport, TcpTokioTransport};
use pyrsia::node_api::routes::make_node_routes;
use pyrsia::node_api::{LOCAL_KEY, LOCAL_PEER_ID, PYRSIA_API_ADDRESS, SWARM_PROXY};

use clap::{App, Arg, ArgMatches};
use futures::StreamExt;
use libp2p::{
    core::identity,
    floodsub::{self, Topic},
    swarm::SwarmEvent,
    Multiaddr, PeerId,
};
use log::{debug, error, info};
use std::borrow::Borrow;
use std::{
    env,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    sync::Arc,
};
use tokio::{
    io::{self, AsyncBufReadExt},
    sync::{mpsc, Mutex, MutexGuard},
};
use warp::Filter;

const DEFAULT_HOST: &str = "127.0.0.1";
const DEFAULT_PORT: &str = "7888";

#[tokio::main]
async fn main() {
    pretty_env_logger::init();
    info!("Pyrsia node starting");

    let matches: ArgMatches = process_command_line_arguments();

    // Reach out to another node if specified
    if let Some(to_dial) = matches.value_of("peer") {
        let addr: Multiaddr = to_dial.parse().unwrap();
        SWARM_PROXY.dial(addr).unwrap();
        info!("Dialed {:?}", to_dial)
    }

    // Listen on all interfaces and whatever port the OS assigns
    SWARM_PROXY
        .listen_on("/ip4/0.0.0.0/tcp/0".parse().unwrap())
        .unwrap();

    // Get host and port from the settings. Defaults to DEFAULT_HOST and DEFAULT_PORT
    let host = matches.value_of("host").unwrap();
    let port = matches.value_of("port").unwrap();
    debug!(
        "Pyrsia Docker Node will bind to host = {}, port = {}",
        host, port
    );

    SWARM_PROXY.start_polling_loop_using_other_thread();

    // Get host and port from the settings. Defaults to DEFAULT_HOST and DEFAULT_PORT
    let host = matches.value_of("host").unwrap();
    let port = matches.value_of("port").unwrap();
    debug!(
        "Pyrsia Docker Node will bind to host = {}, port = {}",
        host, port
    );

    let address = SocketAddr::new(
        IpAddr::V4(host.parse::<Ipv4Addr>().unwrap()),
        port.parse::<u16>().unwrap(),
    );

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
        .bind_ephemeral(address);

    info!(
        "Pyrsia Docker Node is now running on port {}:{}!",
        addr.ip(),
        addr.port()
    );

    tokio::spawn(server);
    let tx4 = tx.clone();

    let raw_chain = block_chain::Blockchain::new();
    let bc = Arc::new(Mutex::new(raw_chain));
    // Kick it off
    loop {
        let evt = {
            tokio::select! {
                line = stdin.next_line() => Some(EventType::Input(line.expect("can get line").expect("can read line from stdin"))),
                message = rx.recv() => Some(EventType::Message(message.expect("message exists"))),

                new_hash = blobs_need_hash.select_next_some() => {
                    debug!("Looking for {}", new_hash);
                    swarm.behaviour_mut().lookup_blob(new_hash).await;
                    None
                },

                event = swarm.select_next_some() =>  {
                    if let SwarmEvent::NewListenAddr { address, .. } = event {
                        info!("Listening on {:?}", address);
                    }

                    //SwarmEvent::Behaviour(e) => panic!("Unexpected event: {:?}", e),
                    None
                },
                get_blocks_request_input = blocks_get_rx_from_api.recv() => //BlockChain::handle_api_requests(),

                // Channels are only one type (both tx and rx must match)
                // We need to have two channel for each API call to send and receive
                // The request and the response.

                {
                    info!("Processessing 'GET /blocks' {} request", get_blocks_request_input.unwrap());
                    let bc1 = bc.clone();
                    let block_chaing_instance = bc1.lock().await.clone();
                    blocks_get_tx_answer_to_api.send(block_chaing_instance).await.expect("send to work");

                    None
                }
            }
        };

        if let Some(event) = evt {
            match event {
                EventType::Response(resp) => {
                    //here we have to manage which events to publish to floodsub
                    swarm
                        .behaviour_mut()
                        .floodsub_mut()
                        .publish(floodsub_topic.clone(), resp.as_bytes());
                }
                EventType::Input(line) => match line.as_str() {
                    "peers" => swarm.behaviour_mut().list_peers_cmd().await,
                    cmd if cmd.starts_with("magnet:") => {
                        info!(
                            "{}",
                            swarm
                                .behaviour_mut()
                                .gossipsub_mut()
                                .publish(gossip_topic.clone(), cmd)
                                .unwrap()
                        )
                    }
                    _ => match tx4.send(line).await {
                        Ok(_) => debug!("line sent"),
                        Err(_) => error!("failed to send stdin input"),
                    },
                },
                EventType::Message(message) => match message.as_str() {
                    cmd if cmd.starts_with("peers") || cmd.starts_with("status") => {
                        swarm.behaviour_mut().list_peers(local_peer_id).await
                    }
                    cmd if cmd.starts_with("get_blobs") => {
                        swarm.behaviour_mut().lookup_blob(message).await
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

    info!("Pyrsia node exiting");
}

fn process_command_line_arguments() -> ArgMatches {
    App::new("Pyrsia Node")
        .version("0.1.0")
        .author(clap::crate_authors!(", "))
        .about("Application to connect to and participate in the Pyrsia network")
        .arg(
            Arg::new("host")
                .short('H')
                .long("host")
                .value_name("HOST")
                .default_value(DEFAULT_HOST)
                .takes_value(true)
                .required(false)
                .multiple(false)
                .help("Sets the host address to bind to for the Docker API"),
        )
        .arg(
            Arg::new("port")
                .short('p')
                .long("port")
                .value_name("PORT")
                .default_value(DEFAULT_PORT)
                .takes_value(true)
                .required(false)
                .multiple(false)
                .help("Sets the port to listen to for the Docker API"),
        )
        .arg(
            Arg::new("peer")
                //.short("p")
                .long("peer")
                .takes_value(true)
                .required(false)
                .multiple(false)
                .help("Provide an explicit peerId to connect with"),
        )
        .get_matches()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    pub fn placeholder_test() {
        assert!(true);
    }
}
