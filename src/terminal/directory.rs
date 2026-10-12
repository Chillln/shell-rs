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
}
