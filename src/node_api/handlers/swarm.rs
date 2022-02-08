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

use super::{RegistryError, RegistryErrorCode};
use crate::block_chain::block_chain::Blockchain;
use crate::node_manager::{handlers::*, model::cli::Status};
use libp2p::PeerId;
use libp2p_kad::{GetClosestPeersError, GetClosestPeersOk, QueryId, QueryResult};
use log::{debug, error, info, warn};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc::{Receiver, Sender};
use tokio::sync::Mutex;
use tokio::time::timeout;
use warp::{http::StatusCode, Rejection, Reply};

const GET_CLOSEST_PEERS_TIMEOUT_SECONDS: u64 = 2;

pub async fn handle_get_peers() -> Result<impl Reply, Rejection> {
    debug!("requesting closest peers");
    let query_id: QueryId = SWARM_PROXY
        .with_behaviour_mut((), |arg| arg.1.kademlia().get_closest_peers(*LOCAL_PEER_ID));
    match timeout(
        Duration::from_secs(GET_CLOSEST_PEERS_TIMEOUT_SECONDS),
        MESSAGE_DELIVERY.receive(query_id),
    )
    .await
    {
        Ok(Ok(QueryResult::GetClosestPeers(core::result::Result::Ok(GetClosestPeersOk {
            peers,
            ..
        })))) => {
            info!("Got peers: {:?}", peers);
            Ok(warp::http::response::Builder::new()
                .header("Content-Type", "application/json")
                .status(StatusCode::OK)
                .body(peers_as_json_string(peers))
                .unwrap())
        }
        Ok(Ok(QueryResult::GetClosestPeers(core::result::Result::Err(
            GetClosestPeersError::Timeout { peers, .. },
        )))) => {
            warn!("Got some peers but timed out: {:?}", peers);
            Ok(warp::http::response::Builder::new()
                .header("Content-Type", "application/json")
                .status(StatusCode::PARTIAL_CONTENT)
                .body(peers_as_json_string(peers))
                .unwrap())
        }
        Ok(Ok(other)) => {
            error!(
                "Expected a QueryResult::GetClosetsPeers, but got {:?}",
                other
            );
            Ok(warp::http::response::Builder::new()
                .header("Content-Type", "text")
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .body(StatusCode::INTERNAL_SERVER_ERROR.as_str().to_string())
                .unwrap())
        }
        Ok(Err(error)) => {
            error!("Error getting closest peers response: {}", error);
            Ok(warp::http::response::Builder::new()
                .header("Content-Type", "text")
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .body(StatusCode::INTERNAL_SERVER_ERROR.as_str().to_string())
                .unwrap())
        },
        Err(timeout_error) => {
            error!("Timed out waiting for Kademlia to find peers: {}", timeout_error);
            Ok(warp::http::response::Builder::new()
                .header("Content-Type", "text")
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .body(StatusCode::INTERNAL_SERVER_ERROR.as_str().to_string())
                .unwrap())
        }
    }
}

fn peers_as_json_string(peers: Vec<PeerId>) -> String {
    let mut string = String::from("[");
    if !peers.is_empty() {
        for peer in peers {
            string.push('"');
            string.push_str(&peer.to_base58());
            string.push_str("\",")
        }
        string.pop();
    }
    string.push(']');
    string
}

pub async fn handle_get_status() -> Result<impl Reply, Rejection> {
    let mut incomplete_peer_count = false;
    let query_id = SWARM_PROXY
        .with_behaviour_mut((), |arg| arg.1.kademlia().get_closest_peers(*LOCAL_PEER_ID));
    let peers_count = match timeout(
        *KADEMLIA_RESPONSE_TIMOUT,
        MESSAGE_DELIVERY.receive(query_id),
    )
    .await
    {
        Ok(Ok(QueryResult::GetClosestPeers(core::result::Result::Ok(GetClosestPeersOk {
            peers,
            ..
        })))) => peers.len(),
        Ok(Ok(QueryResult::GetClosestPeers(core::result::Result::Err(
            GetClosestPeersError::Timeout { peers, .. },
        )))) => {
            incomplete_peer_count = true;
            peers.len()
        }
        Ok(Ok(other)) => {
            error!(
                "Expected a QueryResult::GetClosetsPeers, but got {:?}",
                other
            );
            return Ok(warp::http::response::Builder::new()
                .header("Content-Type", "text")
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .body(StatusCode::INTERNAL_SERVER_ERROR.as_str().to_string())
                .unwrap());
        }
        Ok(Err(error)) => {
            error!("Error getting closest peers response: {}", error);
            return Ok(warp::http::response::Builder::new()
                .header("Content-Type", "text")
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .body(StatusCode::INTERNAL_SERVER_ERROR.as_str().to_string())
                .unwrap());
        },
        Err(timeout) => {
            error!("Timed out waiting for Kademlia peers to be found: {}", timeout);
            return Ok(warp::http::response::Builder::new()
                .header("Content-Type", "text")
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .body(StatusCode::INTERNAL_SERVER_ERROR.as_str().to_string())
                .unwrap());
        }
    };

    let art_count_result = get_arts_count();
    if art_count_result.is_err() {
        return Err(warp::reject::custom(RegistryError {
            code: RegistryErrorCode::Unknown(art_count_result.err().unwrap().to_string()),
        }));
    }

    let disk_space_result = disk_usage(ARTIFACTS_DIR);
    if disk_space_result.is_err() {
        return Err(warp::reject::custom(RegistryError {
            code: RegistryErrorCode::Unknown(disk_space_result.err().unwrap().to_string()),
        }));
    }

    let status = Status {
        artifact_count: art_count_result.unwrap(),
        incomplete_peer_count,
        peers_count,
        disk_allocated: ALLOCATED_SPACE_FOR_ARTIFACTS,
        disk_usage: disk_space_result.unwrap(),
    };

    let ser_status = serde_json::to_string(&status).unwrap();

    Ok(warp::http::response::Builder::new()
        .header("Content-Type", "application/json")
        .status(StatusCode::OK)
        .body(ser_status)
        .unwrap())
}

// TODO Move to block chain module
pub async fn handle_get_blocks(
    tx: Sender<String>,
    rx: Arc<Mutex<Receiver<Blockchain>>>,
) -> Result<impl Reply, Rejection> {
    // Send "digested" request data to main
    match tx.send(String::from("blocks")).await {
        Ok(_) => debug!("request for peers sent"),
        Err(_) => error!("failed to send stdin input"),
    }

    // get result from main ( where the block chain lives )
    let block_chain = rx.lock().await.recv().await.unwrap();
    let blocks = format!("{}", block_chain);
    info!("Got receive_blocks: {}", blocks);

    // format the response
    Ok(warp::http::response::Builder::new()
        .header("Content-Type", "application/json")
        .status(StatusCode::OK)
        .body(blocks)
        .unwrap())
}
