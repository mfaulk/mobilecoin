use crate::{Connection, SyncConnection};
use mc_common::ResponderId;
use mockall::*;

#[automock]
pub trait ConnectionManagerTrait<C: Connection + 'static> {
    /// All ResponderIds owned by this manager.
    fn responder_ids(&self) -> Vec<ResponderId>;

    /// All synchronous connections.
    fn connections(&self) -> Vec<SyncConnection<C>>;

    /// Retrieve a given connection by ResponderId.
    fn get_connection(&self, responder_id: &ResponderId) -> Option<SyncConnection<C>>;

    /// The number of connections.
    fn len(&self) -> usize;

    /// True if the number of connections is zero.
    fn is_empty(&self) -> bool;
}
