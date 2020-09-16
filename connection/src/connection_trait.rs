// Copyright (c) 2018-2020 MobileCoin Inc.

use mc_util_uri::ConnectionUri;
use std::{fmt::Display, hash::Hash};

/// A base connection trait, applicable to all connections.
pub trait Connection: Display + Eq + Hash + Ord + PartialEq + PartialOrd + Send + Sync {
    type Uri: ConnectionUri;

    fn uri(&self) -> Self::Uri;
}
