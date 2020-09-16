// Copyright (c) 2018-2020 MobileCoin Inc.

use crate::{
    error::{Result, RetryResult},
    Connection,
};
use mc_transaction_core::{tx::Tx, BlockIndex};
use std::time::Duration;

/// A trait which supports supporting the submission of transactions to a node
pub trait UserTxConnection: Connection {
    /// Propose a transaction over the encrypted channel.
    /// Returns the number of blocks in the ledger at the time the call was received.
    fn propose_tx(&mut self, tx: &Tx) -> Result<BlockIndex>;
}

/// A trait which supports re-trying transaction submission
pub trait RetryableUserTxConnection {
    /// Propose a transaction over the encrypted channel.
    /// Returns the number of blocks in the ledger at the time the call was received.
    fn propose_tx(
        &self,
        tx: &Tx,
        retry_iterator: impl IntoIterator<Item = Duration>,
    ) -> RetryResult<BlockIndex>;
}
