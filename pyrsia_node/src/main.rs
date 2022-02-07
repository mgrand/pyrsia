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
use pyrsia::node_manager::handlers::{DEFAULT_HOST, DEFAULT_PORT};

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

fn main() {
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

    (*PYRSIA_API_ADDRESS.write().unwrap()) = SocketAddr::new(
        IpAddr::V4(host.parse::<Ipv4Addr>().unwrap()),
        port.parse::<u16>().unwrap(),
    );
    SWARM_PROXY.start_polling_loop_using_my_thread();
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
