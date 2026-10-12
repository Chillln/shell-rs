//! 根据登录 shell 选择历史文件，统一解析为按执行顺序排列的命令。

pub use super::bash::Parsed;
use super::{
    bash,
    model::{History, HistorySource},
};

pub fn command() -> String {
    format!(
        "sh -c 'case ${{SHELL##*/}} in zsh) s=zsh; f=$HOME/.zsh_history;; fish) s=fish; f=${{XDG_DATA_HOME:-$HOME/.local/share}}/fish/fish_history;; *) s=bash; f=$HOME/.bash_history;; esac; echo @@shell; echo \"$s\"; if test -r \"$f\"; then echo @@size; wc -c < \"$f\"; echo @@history; tail -c {} \"$f\"; else echo @@missing; fi'",
        bash::LIMIT
    )
}

pub fn parse(output: &str) -> Parsed {
    let header = output
        .split_once("@@history\n")
        .map_or(output, |(head, _)| head);
    let source = header
        .lines()
        .collect::<Vec<_>>()
        .windows(2)
        .find_map(|pair| {
            (pair[0] == "@@shell").then(|| match pair[1] {
                "zsh" => HistorySource::Zsh,
                "fish" => HistorySource::Fish,
                _ => HistorySource::Bash,
            })
        })
        .unwrap_or_default();
    if source == HistorySource::Bash {
        return bash::parse(output);
    }
    let Some((head, mut body)) = output.split_once("@@history\n") else {
        return bash::parse(output);
    };
    let mut lines = head.lines();
    let size = lines
        .find(|line| *line == "@@size")
        .and_then(|_| lines.next())
        .and_then(|s| s.trim().parse::<u64>().ok());
    let Some(size) = size else {
        return Parsed::Unsupported;
    };
    let truncated = size > bash::LIMIT;
    if truncated {
        // 只从下一个完整记录开始，不能把截断命令的后半段当作新命令。
        body = match source {
            HistorySource::Fish => body
                .find("\n- cmd: ")
                .map(|at| &body[at + 1..])
                .unwrap_or_default(),
            HistorySource::Zsh => after_partial_zsh_record(body),
            HistorySource::Bash => unreachable!(),
        };
    }
    let commands = match source {
        HistorySource::Zsh => zsh_commands(body),
        HistorySource::Fish => fish_commands(body),
        _ => unreachable!(),
    };
    Parsed::History(History::new(commands, truncated).with_source(source))
}

fn after_partial_zsh_record(mut body: &str) -> &str {
    while let Some((line, rest)) = body.split_once('\n') {
        body = rest;
        if line
            .trim_end_matches('\r')
            .bytes()
            .rev()
            .take_while(|b| *b == b'\\')
            .count()
            % 2
            == 0
        {
            return body;
        }
    }
    ""
}

fn zsh_commands(body: &str) -> Vec<(String, Option<i64>)> {
    let mut commands = Vec::new();
    let mut continued: Option<(String, Option<i64>)> = None;
    for line in body.lines() {
        let (text, time) = if continued.is_none() {
            line.strip_prefix(": ")
                .and_then(|rest| {
                    let (metadata, text) = rest.split_once(';')?;
                    let (seconds, duration) = metadata.split_once(':')?;
                    let seconds = seconds.parse::<i64>().ok()?;
                    duration.parse::<u64>().ok()?;
                    Some((text, Some(seconds)))
                })
                .unwrap_or((line, None))
        } else {
            (line, None)
        };
        let more = text.bytes().rev().take_while(|b| *b == b'\\').count() % 2 == 1;
        let text = if more { &text[..text.len() - 1] } else { text };
        let (mut command, at) = continued.take().unwrap_or_else(|| (String::new(), time));
        command.push_str(text);
        if more {
            command.push('\n');
            continued = Some((command, at));
        } else if !command.trim().is_empty() {
            commands.push((command, at));
        }
    }
    commands
}

fn fish_commands(body: &str) -> Vec<(String, Option<i64>)> {
    let mut commands: Vec<(String, Option<i64>)> = Vec::new();
    for line in body.lines() {
        if let Some(encoded) = line.strip_prefix("- cmd: ") {
            let mut command = String::new();
            let mut chars = encoded.chars();
            while let Some(ch) = chars.next() {
                if ch == '\\' {
                    match chars.next() {
                        Some('n') => command.push('\n'),
                        Some('\\') => command.push('\\'),
                        Some(other) => {
                            command.push('\\');
                            command.push(other);
                        }
                        None => command.push('\\'),
                    }
                } else {
                    command.push(ch);
                }
            }
            commands.push((command, None));
        } else if let Some(time) = line.strip_prefix("  when: ")
            && let Some((_, at)) = commands.last_mut()
        {
            *at = time.trim().parse().ok();
        }
    }
    commands.retain(|(command, _)| !command.trim().is_empty());
    commands
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn script_selects_the_login_shell_and_respects_fish_data_directory() {
        let _guard = crate::testing::no_forks();
        let home = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        std::fs::write(home.path().join(".bash_history"), "bash command\n").unwrap();
        std::fs::write(home.path().join(".zsh_history"), ": 2:0;zsh command\n").unwrap();
        std::fs::create_dir_all(data.path().join("fish")).unwrap();
        std::fs::write(
            data.path().join("fish/fish_history"),
            "- cmd: fish command\n  when: 3\n",
        )
        .unwrap();
        for (shell, expected) in [
            ("bash", "bash command"),
            ("zsh", "zsh command"),
            ("fish", "fish command"),
        ] {
            let output = std::process::Command::new("sh")
                .args(["-c", &command()])
                .env("HOME", home.path())
                .env("SHELL", format!("/bin/{shell}"))
                .env("XDG_DATA_HOME", data.path())
                .output()
                .unwrap();
            assert!(output.status.success());
            let Parsed::History(history) = parse(std::str::from_utf8(&output.stdout).unwrap())
            else {
                panic!("缺少历史")
            };
            assert_eq!(history.entries()[0].command, expected);
        }
    }
    #[test]
    fn reads_zsh_extended_history_without_losing_times_or_multiline_commands() {
        let Parsed::History(history) = parse(
            "@@shell\nzsh\n@@size\n150\n@@history\n: 1700000000:2;echo 中文\\\nsecond line\n: 1700000001:0;ls -l\n: 1700000002:0;ls -l\n",
        ) else {
            panic!("缺少历史")
        };
        assert_eq!(history.source(), HistorySource::Zsh);
        assert_eq!(history.entries()[0].command, "ls -l");
        assert_eq!(history.entries()[0].runs, 2);
        assert_eq!(history.entries()[0].last_run, Some(1700000002));
        assert_eq!(history.entries()[1].command, "echo 中文\nsecond line");
        assert_eq!(
            zsh_commands("echo one\necho two\n"),
            vec![("echo one".into(), None), ("echo two".into(), None)]
        );
    }
    #[test]
    fn fish_decodes_only_history_escapes_and_ignores_paths() {
        let Parsed::History(history) = parse(
            "@@shell\nfish\n@@size\n200\n@@history\n- cmd: printf \\n中文\\\\n\n  when: 1700000000\n  paths:\n    - /tmp\n- cmd: grep \\t file\n  when: 1700000001\n",
        ) else {
            panic!("缺少历史")
        };
        assert_eq!(history.source(), HistorySource::Fish);
        assert_eq!(history.entries()[0].command, "grep \\t file");
        assert_eq!(history.entries()[1].command, "printf \n中文\\n");
        assert_eq!(history.entries()[1].last_run, Some(1700000000));
    }
    #[test]
    fn truncated_history_drops_partial_records() {
        for (shell, body, expected) in [
            (
                "fish",
                "partial\\nrm foo\n  when: 1\n- cmd: uptime\n  when: 2\n",
                "uptime",
            ),
            ("zsh", "partial\\\nrm foo\n: 2:0;uptime\n", "uptime"),
            ("zsh", "partial\\\nrm foo\nuptime\n", "uptime"),
        ] {
            let Parsed::History(history) = parse(&format!(
                "@@shell\n{shell}\n@@size\n{}\n@@history\n{body}",
                bash::LIMIT + 1
            )) else {
                panic!("缺少历史")
            };
            assert_eq!(history.entries().len(), 1);
            assert_eq!(history.entries()[0].command, expected);
            assert!(history.truncated());
        }
        assert!(crate::testing::sh_accepts(&command()));
    }
}
