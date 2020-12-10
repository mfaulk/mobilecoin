//! A simulated network of nodes.

use crate::mock_network::{
    scp_node::{SCPNode, SCPNodeSharedData},
    NodeConfig, TestOptions,
};
use mc_common::{
    logger::{log, Logger},
    NodeID,
};
use mc_consensus_scp::Msg;
use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex},
    thread::JoinHandle,
};

#[derive(Clone)]
pub struct NetworkConfig {
    pub name: String,
    pub nodes: Vec<NodeConfig>,
}

impl NetworkConfig {
    pub fn new(name: String, nodes: Vec<NodeConfig>) -> Self {
        Self { name, nodes }
    }
}

pub struct SCPNetwork {
    handle_map: HashMap<NodeID, JoinHandle<()>>,
    pub names_map: HashMap<NodeID, String>,
    nodes_map: Arc<Mutex<HashMap<NodeID, SCPNode>>>,
    shared_data_map: HashMap<NodeID, Arc<Mutex<SCPNodeSharedData>>>,
    pub logger: Logger,
}

impl SCPNetwork {
    // Creates a simulated network.
    pub fn new(network_config: &NetworkConfig, test_options: &TestOptions, logger: Logger) -> Self {
        let mut scp_network = SCPNetwork {
            handle_map: HashMap::default(),
            names_map: HashMap::default(),
            nodes_map: Arc::new(Mutex::new(HashMap::default())),
            shared_data_map: HashMap::default(),
            logger: logger.clone(),
        };

        for node_config in network_config.nodes.iter() {
            assert!(!node_config.peers.contains(&node_config.id));

            let nodes_map_clone = Arc::clone(&scp_network.nodes_map);
            let peers_clone = node_config.peers.clone();

            let (node, join_handle) = SCPNode::new(
                node_config.clone(),
                test_options,
                Arc::new(move |logger, msg| {
                    SCPNetwork::broadcast_msg(logger, &nodes_map_clone, &peers_clone, msg)
                }),
                0, // first slot index
                logger.clone(),
            );
            scp_network
                .handle_map
                .insert(node_config.id.clone(), join_handle);
            scp_network
                .names_map
                .insert(node_config.id.clone(), node_config.name.clone());
            scp_network
                .shared_data_map
                .insert(node_config.id.clone(), node.shared_data.clone());
            scp_network
                .nodes_map
                .lock()
                .expect("lock failed on nodes_map inserting node")
                .insert(node_config.id.clone(), node);
        }

        scp_network
    }

    pub fn stop_all(&mut self) {
        let mut nodes_map = self
            .nodes_map
            .lock()
            .expect("lock failed on nodes_map in stop_all");
        let mut node_ids: Vec<NodeID> = Vec::new();
        for (node_id, node) in nodes_map.iter_mut() {
            log::trace!(
                self.logger,
                "sending stop to {}",
                self.names_map
                    .get(node_id)
                    .expect("could not find node_id in nodes_map"),
            );
            node.send_stop();
            node_ids.push(node_id.clone());
        }
        drop(nodes_map);

        for node_id in node_ids {
            self.handle_map
                .remove(&node_id)
                .expect("thread handle is missing")
                .join()
                .expect("SCPNode join failed");
        }
    }

    pub fn push_value(&self, node_id: &NodeID, value: &str) {
        self.nodes_map
            .lock()
            .expect("lock failed on nodes_map pushing value")
            .get(node_id)
            .expect("could not find node_id in nodes_map")
            .send_value(value);
    }

    pub fn get_ledger(&self, node_id: &NodeID) -> Vec<Vec<String>> {
        self.shared_data_map
            .get(node_id)
            .expect("could not find node_id in shared_data_map")
            .lock()
            .expect("lock failed on shared_data getting ledger")
            .ledger
            .clone()
    }

    pub fn get_ledger_size(&self, node_id: &NodeID) -> usize {
        self.shared_data_map
            .get(node_id)
            .expect("could not find node_id in shared_data_map")
            .lock()
            .expect("lock failed on shared_data getting ledger size")
            .ledger_size()
    }

    pub fn broadcast_msg(
        logger: Logger,
        nodes_map: &Arc<Mutex<HashMap<NodeID, SCPNode>>>,
        peers: &HashSet<NodeID>,
        msg: Msg<String>,
    ) {
        let mut nodes_map = nodes_map
            .lock()
            .expect("lock failed on nodes_map in broadcast");

        log::trace!(logger, "(broadcast) {}", msg);

        let amsg = Arc::new(msg);

        for peer_id in peers {
            nodes_map
                .get_mut(&peer_id)
                .expect("failed to get peer from nodes_map")
                .send_msg(amsg.clone());
        }
    }
}

impl Drop for SCPNetwork {
    fn drop(&mut self) {
        self.stop_all();
    }
}