use std::collections::HashMap;

use ipnet::Ipv6Net;
use serde::{Deserialize, Serialize};

pub mod storage;

pub type Ipv6Mapping = HashMap<Ipv6Net, Ipv6Net>;

#[derive(Serialize, Deserialize, Default)]
pub struct State {
    pub prefix: Ipv6Net,
    pub mapping: Ipv6Mapping,
}
