// Copyright (c) 2018-2020 MobileCoin Inc.

use crate::{
    error::{Result, RetryResult},
    Connection,
};
use mc_transaction_core::{Block, BlockID, BlockIndex};
use std::{ops::Range, time::Duration};

/// A connection trait providing APIs for use in retrieving blocks from a consensus node.
pub trait BlockchainConnection: Connection {
    /// Retrieve the block metadata from the blockchain service.
    fn fetch_blocks(&mut self, range: Range<BlockIndex>) -> Result<Vec<Block>>;

    /// Retrieve the BlockIDs (hashes) of the given blocks from the blockchain service.
    fn fetch_block_ids(&mut self, range: Range<BlockIndex>) -> Result<Vec<BlockID>>;

    /// Retrieve the consensus node's current block height
    fn fetch_block_height(&mut self) -> Result<BlockIndex>;
}

/// A connection trait providing retryable blockchain data APIs.
pub trait RetryableBlockchainConnection {
    /// Retrieve the block metadata from the blockchain service.
    fn fetch_blocks(
        &self,
        range: Range<BlockIndex>,
        retry_iterator: impl IntoIterator<Item = Duration>,
    ) -> RetryResult<Vec<Block>>;

    /// Retrieve the BlockIDs (hashes) of the given blocks from the blockchain service.
    fn fetch_block_ids(
        &self,
        range: Range<BlockIndex>,
        retry_iterator: impl IntoIterator<Item = Duration>,
    ) -> RetryResult<Vec<BlockID>>;

    /// Retrieve the highest block index published
    fn fetch_block_height(
        &self,
        retry_iterator: impl IntoIterator<Item = Duration>,
    ) -> RetryResult<BlockIndex>;
}
