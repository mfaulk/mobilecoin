// Copyright (c) 2018-2020 MobileCoin Inc.

//! Connection support

mod attested_connection;
mod blockchain_connection;
mod connection_manager;
mod connection_trait;
mod error;
mod sync;
mod thick;
mod user_tx_connection;

pub use self::{
    attested_connection::{AttestationError, AttestedConnection},
    blockchain_connection::{BlockchainConnection, RetryableBlockchainConnection},
    connection_manager::ConnectionManager,
    connection_trait::Connection,
    error::{Error, Result, RetryError, RetryResult},
    sync::SyncConnection,
    thick::{ThickClient, ThickClientAttestationError},
    user_tx_connection::{RetryableUserTxConnection, UserTxConnection},
};

pub use mc_common::trace_time as _trace_time;
pub use retry as _retry;
