use std::{
    collections::{BTreeMap, HashMap},
    net::Ipv6Addr,
};

use ipnet::Ipv6Net;
use serde::{Deserialize, Serialize};

pub mod storage;

pub type Ipv6NetMapping = HashMap<Ipv6Net, Ipv6Net>;
pub type Ipv6AddrMapping = BTreeMap<Ipv6Addr, Ipv6Addr>;

#[derive(Serialize, Deserialize, Default)]
pub struct State {
    pub prefix: Ipv6Net,
    pub mapping: Ipv6NetMapping,
}
