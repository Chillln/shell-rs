use std::collections::HashSet;

use gpui_kit::Context;

use super::HostStore;
use crate::host::{HostInventory, SnippetScope};

impl HostStore {
    /// 仅快照保存的主机、分组和凭据，临时连接留在当前设备。
    pub fn inventory(&self) -> HostInventory {
        let mut inventory = HostInventory::new(
            self.groups.clone(),
            self.hosts.clone(),
            self.credentials.clone(),
        );
        inventory.next_ids = [
            self.next_host_id,
            self.next_group_id,
            self.next_credential_id,
        ];
        inventory.secret_revision = self.secret_revision.get();
        inventory
    }

    /// 在一次数据库事务成功后替换清单，保留仍存在的主机的本地关联数据。
    pub fn replace_inventory(
        &mut self,
        mut inventory: HostInventory,
        cx: &mut Context<Self>,
    ) -> rusqlite::Result<()> {
        // 等待后台导入时仍可关闭连接、展开分组；这些本机状态采用提交时的值。
        for host in &mut inventory.hosts {
            host.os = self
                .hosts
                .iter()
                .find(|old| old.id == host.id)
                .and_then(|old| old.os);
        }
        for group in &mut inventory.groups {
            if let Some(old) = self.groups.iter().find(|old| old.id == group.id) {
                group.expanded = old.expanded;
            }
        }
        if let Some(database) = &self.database {
            database.replace_inventory(&inventory)?;
        }
        let hosts: HashSet<_> = inventory.hosts.iter().map(|host| host.id).collect();
        let groups: HashSet<_> = inventory.groups.iter().map(|group| group.id).collect();
        self.recent.retain(|id| hosts.contains(id));
        self.bookmarks.retain(|(id, _), _| hosts.contains(id));
        self.forwards.retain(|rule| hosts.contains(&rule.host));
        self.snippets.retain(|snippet| match snippet.scope {
            SnippetScope::All => true,
            SnippetScope::Host(id) => hosts.contains(&id),
            SnippetScope::Group(id) => groups.contains(&id),
        });
        self.next_host_id = self.next_host_id.max(
            inventory
                .hosts
                .iter()
                .map(|host| host.id.0 + 1)
                .max()
                .unwrap_or(1),
        );
        self.next_group_id = self.next_group_id.max(
            inventory
                .groups
                .iter()
                .map(|group| group.id.0 + 1)
                .max()
                .unwrap_or(1),
        );
        self.next_credential_id = self.next_credential_id.max(
            inventory
                .credentials
                .iter()
                .map(|credential| credential.id.0 + 1)
                .max()
                .unwrap_or(1),
        );
        self.hosts = inventory.hosts;
        self.groups = inventory.groups;
        self.credentials = inventory.credentials;
        self.active = None;
        cx.notify();
        Ok(())
    }
}
