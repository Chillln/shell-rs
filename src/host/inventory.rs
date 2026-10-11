//! 云同步使用的主机清单边界，不携带秘密或运行状态。

use super::{Credential, Host, HostGroup};

#[derive(Clone)]
#[non_exhaustive]
pub struct HostInventory {
    pub groups: Vec<HostGroup>,
    pub hosts: Vec<Host>,
    pub credentials: Vec<Credential>,
    pub(crate) next_ids: [u64; 3],
    pub(crate) secret_revision: u64,
}

impl HostInventory {
    pub fn new(groups: Vec<HostGroup>, hosts: Vec<Host>, credentials: Vec<Credential>) -> Self {
        let next_ids = [
            hosts.iter().map(|item| item.id.0 + 1).max().unwrap_or(1),
            groups.iter().map(|item| item.id.0 + 1).max().unwrap_or(1),
            credentials
                .iter()
                .map(|item| item.id.0 + 1)
                .max()
                .unwrap_or(1),
        ];
        Self {
            groups,
            hosts,
            credentials,
            next_ids,
            secret_revision: 0,
        }
    }
}
