//! 客户端加密的主机保险库与 S3 同步。

mod crypto;
mod model;
mod panel;
mod service;
mod storage;
mod vault;

#[cfg(test)]
pub(crate) use tests::test_open_cloud;

pub use model::CloudSyncSettings;
pub use panel::CloudSyncPanel;
pub(crate) use panel::{SyncInput, SyncPhase};
pub(crate) use service::{AutomaticSync, Download, SyncServices, Upload, inventory_stamp};

#[cfg(test)]
pub(crate) mod tests;
