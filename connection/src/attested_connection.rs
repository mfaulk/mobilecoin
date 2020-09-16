// Copyright (c) 2018-2020 MobileCoin Inc.

use crate::Connection;
use grpcio::{Error as GrpcError, RpcStatusCode};
use std::{
    fmt::{Debug, Display},
    result::Result as StdResult,
};

/// A marker trait used to encapsulate connection-impl-specific attestation errors.
pub trait AttestationError: Debug + Display + Send + Sync {}

pub trait AttestedConnection: Connection {
    type Error: AttestationError + From<GrpcError>;

    fn is_attested(&self) -> bool;

    fn attest(&mut self) -> StdResult<(), Self::Error>;

    fn deattest(&mut self);

    fn attested_call<T>(
        &mut self,
        func: impl FnOnce(&mut Self) -> StdResult<T, GrpcError>,
    ) -> StdResult<T, Self::Error> {
        if !self.is_attested() {
            self.attest()?;
        }

        let result = func(self);

        if let Err(GrpcError::RpcFailure(rpc_status)) = &result {
            if rpc_status.status == RpcStatusCode::UNAUTHENTICATED {
                self.deattest();
            }
        }

        Ok(result?)
    }
}
