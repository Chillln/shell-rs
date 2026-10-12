//! 历史命令: the right sidebar's tool that lists the commands bash kept in
//! the history file of the host of the SSH terminal in front, read over
//! that terminal's own connection. A click puts one on the terminal's input
//! line; 执行 runs it there.

mod bash;
mod history_panel;
mod model;
mod shell;

pub use history_panel::HistoryPanel;

/// 提供统一的数据边界，终端模块只接收命令快照。
pub use shell::command as read_command;
pub fn prediction_commands(output: &str) -> Option<Vec<String>> {
    let shell::Parsed::History(history) = shell::parse(output) else {
        return None;
    };
    Some(
        history
            .entries()
            .iter()
            .take(5000)
            .map(|entry| entry.command.clone())
            .collect(),
    )
}

#[cfg(test)]
mod prediction_tests {
    use super::prediction_commands;

    #[test]
    fn predictions_share_the_deduplicated_remote_history_order() {
        let commands =
            prediction_commands("@@size\n100\n@@history\nls -l\nhtop\nls -l\nhistory\nhtop\n")
                .unwrap();
        assert_eq!(commands, ["htop", "history", "ls -l"]);
        assert!(prediction_commands("@@missing\n").is_none());
        let fish = prediction_commands("@@shell\nfish\n@@size\n100\n@@history\n- cmd: ls -l\n  when: 100\n- cmd: ls -lh\n  when: 200\n- cmd: ls -l\n  when: 300\n").unwrap();
        assert_eq!(fish, ["ls -l", "ls -lh"]);
    }
}
