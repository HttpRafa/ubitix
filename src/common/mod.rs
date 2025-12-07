use std::collections::HashMap;

use ipnet::Ipv6Net;
use serde::{Deserialize, Serialize};

pub mod storage;

#[derive(Serialize, Deserialize, Default)]
pub struct State {
    pub prefix: Ipv6Net,
    pub mapping: HashMap<Ipv6Net, Ipv6Net>,
}
