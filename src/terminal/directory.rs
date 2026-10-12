//! 终端报告的目录与无交互输入的目录启动命令。

/// 目录报告只接受完整的文件 URI，不把截断序列或控制字符当作路径。
pub(super) fn reported_directory(params: &[&[u8]]) -> Option<String> {
    if params.first()? != b"7" || params.len() < 2 || params.len() >= 16 {
        return None;
    }
    let bytes = params[1..].join(&b';');
    if bytes.len() > 12 * 1024 {
        return None;
    }
    let text = std::str::from_utf8(&bytes).ok()?;
    if text.chars().any(char::is_control) {
        return None;
    }
    let uri = url::Url::parse(text).ok()?;
    if uri.scheme() != "file"
        || uri.query().is_some()
        || uri.fragment().is_some()
        || !uri.username().is_empty()
        || uri.password().is_some()
    {
        return None;
    }
    let path = percent_encoding::percent_decode_str(uri.path())
        .decode_utf8()
        .ok()?;
    (path.starts_with('/') && !path.chars().any(char::is_control)).then(|| path.into_owned())
}

/// 路径编码为 printf 的八进制参数，避免登录 shell 展开引号、感叹号和替换表达式。
pub fn directory_terminal_command(path: &str) -> Option<String> {
    if path.is_empty() || path.chars().any(char::is_control) {
        return None;
    }
    let encoded: String = path.bytes().map(|byte| format!("\\0{byte:03o}")).collect();
    Some(format!(
        "sh -c 'cd -- \"$(printf %b \"$1\")\" || exit; exec \"${{SHELL:-/bin/sh}}\" -l' sh '{encoded}'"
    ))
}

/// 通过独立 SSH exec 启动带报告的 Bash，不向交互输入写命令或改写远端配置。
pub fn directory_reporting_terminal_command() -> String {
    let encoded_rc: String = BASH_REPORTING_RC
        .bytes()
        .map(|byte| format!("\\0{byte:03o}"))
        .collect();
    let message: String = crate::i18n::t!("directory.reporting.bash_missing")
        .bytes()
        .map(|byte| format!("\\0{byte:03o}"))
        .collect();
    format!(
        "sh -c 'command -v bash >/dev/null 2>&1 || {{ printf \"%b\\n\" \"$3\"; exit 127; }}; exec bash -lc \"$1\" bash \"$2\"' sh 'exec bash --rcfile <(printf \"%b\" \"$1\") -i' '{encoded_rc}' '{message}'"
    )
}

// 登录环境由外层 Bash 加载，交互配置和报告钩子由匿名管道提供，不创建配置文件。
const BASH_REPORTING_RC: &str = r#"if [[ -r $HOME/.bashrc ]]; then
    source "$HOME/.bashrc"
fi
__shellrs_prompt() {
    local LC_ALL=C encoded= ch code i
    for ((i=0; i<${#PWD}; i++)); do
        ch=${PWD:i:1}
        case "$ch" in
            [a-zA-Z0-9/._~-]) encoded+=$ch ;;
            *) printf -v code '%%%02X' "'$ch"; encoded+=$code ;;
        esac
    done
    printf '\033]7;file://localhost%s\007\033]133;A\007' "$encoded"
    if [[ $PS1 != *'\[\e]133;B\a\]'* ]]; then
        PS1+='\[\e]133;B\a\]'
    fi
}
if [[ ${PROMPT_COMMAND[*]} != *__shellrs_prompt* ]]; then
    if ((BASH_VERSINFO[0] > 5 || (BASH_VERSINFO[0] == 5 && BASH_VERSINFO[1] >= 1))); then
        PROMPT_COMMAND=("${PROMPT_COMMAND[@]}" __shellrs_prompt)
    else
        PROMPT_COMMAND="${PROMPT_COMMAND:+$PROMPT_COMMAND$'\n'}__shellrs_prompt"
    fi
fi
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_file_paths_without_accepting_commands_or_partial_urls() {
        assert_eq!(
            reported_directory(&[b"7", b"file://server/srv/a%20b/%E4%B8%AD%E6%96%87"]),
            Some("/srv/a b/中文".into())
        );
        for uri in [
            "https://server/tmp",
            "file://server/tmp?x",
            "file://server/tmp#x",
            "file://user@server/tmp",
            "file://server/a%00b",
            "file://server/a%0Ab",
        ] {
            assert_eq!(reported_directory(&[b"7", uri.as_bytes()]), None);
        }
        assert!(reported_directory(&[b"7", &vec![b'a'; 16 * 1024]]).is_none());
    }

    #[test]
    fn entry_encodes_special_characters_and_refuses_control_characters() {
        let command = directory_terminal_command("/srv/a b'!$(touch hacked)/中文").unwrap();
        assert!(!command.contains("touch hacked"));
        assert!(!command.contains('!'));
        assert!(crate::testing::sh_accepts(&command));
        assert!(directory_terminal_command("/srv/a\nb").is_none());
        assert!(directory_terminal_command("").is_none());
    }

    #[test]
    fn reporting_entry_is_syntax_checked_and_never_writes_shell_configuration() {
        let command = directory_reporting_terminal_command();
        assert!(crate::testing::sh_accepts(&command));
        let _forks = crate::testing::no_forks();
        assert!(
            std::process::Command::new("bash")
                .args(["--noprofile", "--norc", "-n", "-c", BASH_REPORTING_RC])
                .status()
                .unwrap()
                .success()
        );
        assert!(!BASH_REPORTING_RC.contains("history -a"));
        assert!(!BASH_REPORTING_RC.contains(".bashrc >>"));
    }

    #[test]
    fn reporting_hook_preserves_profile_and_encodes_changed_directories() {
        use std::io::Write as _;
        use std::process::{Command, Stdio};
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("a b#%中文");
        std::fs::create_dir(&directory).unwrap();
        let profile = "PS1='custom> '\nPROMPT_COMMAND=(\"printf 'existing-hook\\n'\")\nHISTFILE=\"$HOME/history\"\n";
        std::fs::write(root.path().join(".bashrc"), profile).unwrap();
        let encoded_rc: String = BASH_REPORTING_RC
            .bytes()
            .map(|byte| format!("\\0{byte:03o}"))
            .collect();
        let _forks = crate::testing::no_forks();
        let mut child = Command::new("bash")
            .args([
                "--noprofile",
                "--norc",
                "-c",
                "exec bash --rcfile <(printf '%b' \"$1\") -i",
                "bash",
            ])
            .arg(encoded_rc)
            .env_remove("BASH_ENV")
            .env_remove("ENV")
            .env("HOME", root.path().to_string_lossy().replace('\\', "/"))
            .env(
                "SHELLRS_TEST_DIRECTORY",
                directory.to_string_lossy().replace('\\', "/"),
            )
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(b"cd -- \"$SHELLRS_TEST_DIRECTORY\"\nprintf 'user-command\\n'\nexit\n")
            .unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8(output.stdout).unwrap();
        assert!(stdout.contains("existing-hook\n"));
        assert!(stdout.contains("\x1b]7;file://localhost"));
        assert!(
            stdout.contains("/a%20b%23%25%E4%B8%AD%E6%96%87\x07"),
            "{stdout}"
        );
        assert!(String::from_utf8_lossy(&output.stderr).contains("custom> "));
        assert_eq!(
            std::fs::read_to_string(root.path().join(".bashrc")).unwrap(),
            profile
        );
        assert!(
            !std::fs::read_to_string(root.path().join("history"))
                .unwrap()
                .contains("__shellrs_prompt")
        );
    }
}
