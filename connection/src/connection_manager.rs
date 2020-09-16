// Copyright (c) 2018-2020 MobileCoin Inc.

//! Manages a set of connections to peers.

use crate::{sync::SyncConnection, Connection};
use mc_common::{
    logger::{o, Logger},
    ResponderId,
};
use mc_util_uri::ConnectionUri;
use std::{
    collections::HashMap,
    iter::FromIterator,
    sync::{Arc, RwLock, RwLockReadGuard},
};

/// A connection manager manages a list of peers it is connected to.
pub struct ConnectionManager<C: Connection> {
    /// Connections to peers.
    peer_connections: Arc<RwLock<HashMap<ResponderId, SyncConnection<C>>>>,
}

impl<C: Connection> Clone for ConnectionManager<C> {
    fn clone(&self) -> Self {
        Self {
            peer_connections: self.peer_connections.clone(),
        }
    }
}

impl<C: Connection> ConnectionManager<C> {
    /// Constructor.
    ///
    /// # Arguments
    /// * `connections` - Connections to peers.
    pub fn new(connections: Vec<C>, logger: Logger) -> Self {
        let peer_connections = HashMap::from_iter(connections.into_iter().map(|conn| {
            let responder_id = conn.uri().responder_id().unwrap_or_else(|_| {
                panic!(
                    "Could not create responder_id from {:?}",
                    conn.uri().to_string()
                )
            });
            let name = conn.to_string();
            let sync_conn = SyncConnection::new(conn, logger.new(o!("mc.peers.peer_name" => name)));
            (responder_id, sync_conn)
        }));

        Self {
            peer_connections: Arc::new(RwLock::new(peer_connections)),
        }
    }

    fn read(&self) -> RwLockReadGuard<HashMap<ResponderId, SyncConnection<C>>> {
        self.peer_connections
            .read()
            .expect("ConnectionManager lock poisoned")
    }

    /// Retrieve a vector of all the connection URLs owned by this manager.
    pub fn responder_ids(&self) -> Vec<ResponderId> {
        self.read().keys().cloned().collect()
    }

    /// Retrieve an array of synchronous connection supports.
    pub fn connections(&self) -> Vec<SyncConnection<C>> {
        self.read().values().cloned().collect()
    }

    // /// Retrieve a map of URLs to the connection type.
    // pub fn id_to_conn(&self) -> HashMap<ResponderId, SyncConnection<C>> {
    //     self.read().clone()
    // }

    /// Retrieve a given connection by ResponderId.
    pub fn get_connection(&self, responder_id: &ResponderId) -> Option<SyncConnection<C>> {
        self.read().get(responder_id).cloned()
    }

    /// Retrieve a count of the number connections we're aware of.
    pub fn len(&self) -> usize {
        self.read().len()
    }

    /// Check whether there any connections or not.
    pub fn is_empty(&self) -> bool {
        self.read().is_empty()
    }
}
