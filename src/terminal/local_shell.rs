//! 解析本地终端运行的 Shell，检测系统默认值与可供选择的已安装程序。

use std::ffi::OsString;
#[cfg(any(unix, test))]
use std::path::Path;
use std::path::PathBuf;

use anyhow::{Result, anyhow};
use portable_pty::CommandBuilder;

use crate::i18n::t;

/// 用户指定的本地终端程序及参数；未指定时使用系统默认 Shell。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LocalShell {
    command: Option<String>,
}

impl LocalShell {
    /// 去除命令首尾空白；空命令表示系统默认 Shell。
    pub fn new(command: &str) -> Self {
        let command = command.trim();
        Self {
            command: (!command.is_empty()).then(|| command.to_string()),
        }
    }

    pub fn command(&self) -> Option<&str> {
        self.command.as_deref()
    }

    /// 构造启动命令；文件系统查询会阻塞，应在传输线程调用。
    /// 单独指定 Shell 时以登录 Shell 启动，以读取用户配置；带参数的命令按原样执行。
    pub(super) fn command_builder(&self) -> Result<CommandBuilder> {
        let Some(command) = &self.command else {
            return Ok(default_command());
        };
        let argv = split_command(command, QUOTING)
            .ok_or_else(|| anyhow!(t!("terminal.pty.unmatched_quote", command = command)))?;
        #[cfg(unix)]
        if let [program] = argv.as_slice()
            && is_shell_name(program)
            && let Some(path) = find_program(program)
        {
            let mut builder = CommandBuilder::new_default_prog();
            builder.env("SHELL", path);
            return Ok(builder);
        }
        Ok(CommandBuilder::from_argv(
            argv.into_iter().map(OsString::from).collect(),
        ))
    }
}

/// 获取系统默认 Shell：Unix 使用用户 Shell；Windows 优先 PowerShell 7，
/// 其次 Windows PowerShell，均不可用时退回 portable-pty 的默认命令提示符。
fn default_command() -> CommandBuilder {
    #[cfg(windows)]
    if let Some(program) = windows_default_shell() {
        return CommandBuilder::new(program);
    }
    CommandBuilder::new_default_prog()
}

/// 获取默认 Shell 的显示名称；Unix 返回路径，Windows 返回文件名。此操作可能阻塞。
pub fn default_shell_name() -> String {
    #[cfg(windows)]
    {
        let program = windows_default_shell()
            .unwrap_or_else(|| PathBuf::from(CommandBuilder::new_default_prog().get_shell()));
        program
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| program.to_string_lossy().into_owned())
    }
    #[cfg(not(windows))]
    CommandBuilder::new_default_prog().get_shell()
}

/// 扫描可供选择的 Shell；Unix 枚举常用程序，Windows 枚举 PowerShell 与命令提示符。
/// 返回值可直接写入设置输入框，此操作可能阻塞。
pub fn installed_shells() -> Vec<String> {
    #[cfg(unix)]
    {
        let listed = std::fs::read_to_string("/etc/shells")
            .map(|text| listed_shells(&text))
            .unwrap_or_default();
        let mut folders: Vec<PathBuf> = std::env::var_os("PATH")
            .map(|paths| {
                std::env::split_paths(&paths)
                    .filter(|folder| folder.is_absolute())
                    .collect()
            })
            .unwrap_or_default();
        folders.extend(SHELL_FOLDERS.iter().map(PathBuf::from));
        common_shells(&listed, &folders, is_executable, program_key)
    }
    #[cfg(windows)]
    {
        let mut shells = Vec::new();
        if find_on_path("pwsh.exe").is_some() {
            shells.push("pwsh.exe".to_string());
        } else if let Some(pwsh) = installed_pwsh() {
            shells.push(quote_word(&pwsh.to_string_lossy()));
        }
        if windows_powershell().is_some() {
            shells.push("powershell.exe".to_string());
        }
        shells.push("cmd.exe".to_string());
        shells
    }
}

/// 命令行的引号规则；正式运行只使用当前平台的规则，测试覆盖两种规则。
#[cfg_attr(not(test), allow(dead_code))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Quoting {
    /// POSIX 风格：支持单引号、双引号和反斜杠转义。
    Posix,
    /// Windows 风格：仅识别双引号，反斜杠保留在路径中。
    Windows,
}

#[cfg(not(windows))]
const QUOTING: Quoting = Quoting::Posix;
#[cfg(windows)]
const QUOTING: Quoting = Quoting::Windows;

/// 按引号外的空白拆分命令；引号未闭合时返回 `None`。
fn split_command(line: &str, quoting: Quoting) -> Option<Vec<String>> {
    let posix = quoting == Quoting::Posix;
    let mut words = Vec::new();
    let mut word = String::new();
    // 空引号也代表一个已开始的参数。
    let mut in_word = false;
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        match c {
            c if c.is_whitespace() => {
                if in_word {
                    words.push(std::mem::take(&mut word));
                    in_word = false;
                }
            }
            '"' => {
                in_word = true;
                loop {
                    match chars.next()? {
                        '"' => break,
                        // 双引号内仅对可能结束引号或触发展开的字符使用反斜杠转义。
                        '\\' if posix => match chars.next()? {
                            c @ ('"' | '\\' | '$' | '`') => word.push(c),
                            c => {
                                word.push('\\');
                                word.push(c);
                            }
                        },
                        c => word.push(c),
                    }
                }
            }
            '\'' if posix => {
                in_word = true;
                loop {
                    match chars.next()? {
                        '\'' => break,
                        c => word.push(c),
                    }
                }
            }
            '\\' if posix => {
                in_word = true;
                // 行尾反斜杠按字面值保留。
                word.push(chars.next().unwrap_or('\\'));
            }
            c => {
                in_word = true;
                word.push(c);
            }
        }
    }
    if in_word {
        words.push(word);
    }
    Some(words)
}

/// 将文件路径写成可被 `split_command` 还原为单个参数的文本。
/// Windows 路径含空白时加双引号；Windows 文件名本身不能包含双引号。
pub fn quote_word(word: &str) -> String {
    quote_word_for(word, QUOTING)
}

fn quote_word_for(word: &str, quoting: Quoting) -> String {
    match quoting {
        Quoting::Posix => {
            let plain = !word.is_empty()
                && word
                    .chars()
                    .all(|c| c.is_alphanumeric() || "/._-+@%:,=~".contains(c));
            if plain {
                word.to_string()
            } else {
                format!("'{}'", word.replace('\'', r"'\''"))
            }
        }
        Quoting::Windows => {
            if word.is_empty() || word.chars().any(char::is_whitespace) {
                format!("\"{word}\"")
            } else {
                word.to_string()
            }
        }
    }
}

/// 识别支持登录模式的 Shell；其他程序即使单独指定也不加登录参数。
#[cfg(any(unix, test))]
const SHELL_NAMES: &[&str] = &[
    "ash", "bash", "csh", "dash", "elvish", "fish", "ksh", "mksh", "nu", "oksh", "pwsh", "sh",
    "tcsh", "xonsh", "yash", "zsh",
];

#[cfg(any(unix, test))]
fn is_shell_name(program: &str) -> bool {
    Path::new(program)
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| SHELL_NAMES.contains(&name))
}

/// 菜单按此顺序展示常用 Shell；其他程序可通过文件选择器指定。
#[cfg(unix)]
const COMMON_SHELLS: &[&str] = &["zsh", "bash", "sh", "fish", "nu", "pwsh"];

/// 除 `PATH` 外还搜索系统及 Homebrew 目录，以覆盖图形应用环境变量不完整的情况。
#[cfg(unix)]
const SHELL_FOLDERS: &[&str] = &["/bin", "/usr/bin", "/usr/local/bin", "/opt/homebrew/bin"];

/// 从 `/etc/shells` 和候选目录中筛选可用程序，并按 `key` 去重。
#[cfg(unix)]
fn common_shells<K: Eq + std::hash::Hash>(
    listed: &[String],
    folders: &[PathBuf],
    usable: impl Fn(&Path) -> bool,
    key: impl Fn(&Path) -> K,
) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut shells = Vec::new();
    for name in COMMON_SHELLS {
        let named = listed
            .iter()
            .map(PathBuf::from)
            .filter(|path| path.file_name() == Some(std::ffi::OsStr::new(name)));
        for path in named.chain(folders.iter().map(|folder| folder.join(name))) {
            if usable(&path) && seen.insert(key(&path)) {
                shells.push(path.to_string_lossy().into_owned());
            }
        }
    }
    shells
}

/// 解析 `/etc/shells` 中的路径，忽略注释。
#[cfg(any(unix, test))]
fn listed_shells(text: &str) -> Vec<String> {
    text.lines()
        .map(|line| line.split('#').next().unwrap_or_default().trim())
        .filter(|line| line.starts_with('/'))
        .map(String::from)
        .collect()
}

/// 解析程序路径：绝对路径直接检查，普通名称从 `PATH` 查找。
#[cfg(unix)]
fn find_program(program: &str) -> Option<PathBuf> {
    if program.contains('/') {
        let path = PathBuf::from(program);
        return (path.is_absolute() && is_executable(&path)).then_some(path);
    }
    let paths = std::env::var_os("PATH")?;
    std::env::split_paths(&paths)
        .map(|dir| dir.join(program))
        .find(|candidate| is_executable(candidate))
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt as _;
    path.metadata()
        .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
}

/// 用目录的规范路径和原始文件名标识程序，避免符号链接造成重复，同时区分 `sh` 与 `dash`。
#[cfg(unix)]
fn program_key(path: &Path) -> (PathBuf, Option<OsString>) {
    let folder = path.parent().unwrap_or(Path::new("/"));
    (
        std::fs::canonicalize(folder).unwrap_or_else(|_| folder.to_path_buf()),
        path.file_name().map(OsString::from),
    )
}

#[cfg(windows)]
fn windows_default_shell() -> Option<PathBuf> {
    find_on_path("pwsh.exe")
        .or_else(installed_pwsh)
        .or_else(windows_powershell)
}

/// 在安装程序的默认目录查找不在 `PATH` 中的 PowerShell 7。
#[cfg(windows)]
fn installed_pwsh() -> Option<PathBuf> {
    let path = PathBuf::from(std::env::var_os("ProgramFiles")?).join(r"PowerShell\7\pwsh.exe");
    path.is_file().then_some(path)
}

#[cfg(windows)]
fn windows_powershell() -> Option<PathBuf> {
    let path = PathBuf::from(std::env::var_os("SystemRoot")?)
        .join(r"System32\WindowsPowerShell\v1.0\powershell.exe");
    path.is_file().then_some(path)
}

/// 从 `PATH` 查找程序；商店安装版可能是无法跟随的执行别名，因此不使用 `is_file`。
#[cfg(windows)]
fn find_on_path(name: &str) -> Option<PathBuf> {
    let paths = std::env::var_os("PATH")?;
    std::env::split_paths(&paths)
        .map(|dir| dir.join(name))
        .find(|candidate| {
            candidate
                .symlink_metadata()
                .is_ok_and(|metadata| !metadata.is_dir())
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(line: &str, quoting: Quoting) -> Vec<String> {
        split_command(line, quoting).expect("quotes are closed")
    }

    #[test]
    fn a_blank_command_is_the_default_shell() {
        assert_eq!(LocalShell::new("  ").command(), None);
        assert_eq!(LocalShell::new(""), LocalShell::default());
        assert_eq!(
            LocalShell::new(" /bin/zsh -l ").command(),
            Some("/bin/zsh -l")
        );
    }

    #[test]
    fn posix_commands_split_as_a_shell_reads_them() {
        let posix = Quoting::Posix;
        assert_eq!(words("/bin/zsh", posix), ["/bin/zsh"]);
        assert_eq!(
            words("  /bin/bash   --login -i ", posix),
            ["/bin/bash", "--login", "-i"]
        );
        assert_eq!(
            words(r#"'/opt/My Shells/fish' -C "echo \"hi\" \$HOME \n""#, posix),
            ["/opt/My Shells/fish", "-C", r#"echo "hi" $HOME \n"#]
        );
        assert_eq!(words(r"/opt/My\ Shells/nu", posix), ["/opt/My Shells/nu"]);
        assert_eq!(words(r#"sh -c '' """#, posix), ["sh", "-c", "", ""]);
        assert_eq!(words(r"a'b'c\", posix), [r"abc\"]);
        assert!(words("", posix).is_empty());
        assert_eq!(split_command("sh -c 'exit", posix), None);
        assert_eq!(split_command(r#"sh -c "exit\""#, posix), None);
    }

    #[test]
    fn windows_commands_keep_their_backslashes() {
        let windows = Quoting::Windows;
        assert_eq!(
            words(
                r#""C:\Program Files\PowerShell\7\pwsh.exe" -NoLogo"#,
                windows
            ),
            [r"C:\Program Files\PowerShell\7\pwsh.exe", "-NoLogo"]
        );
        assert_eq!(
            words(r"C:\Windows\System32\cmd.exe /k cd \d", windows),
            [r"C:\Windows\System32\cmd.exe", "/k", "cd", r"\d"]
        );
        // Windows 不把单引号视为引号。
        assert_eq!(
            words("cmd /c echo 'a b'", windows),
            ["cmd", "/c", "echo", "'a", "b'"]
        );
        assert_eq!(split_command(r#""C:\Program Files"#, windows), None);
    }

    #[test]
    fn a_chosen_file_reads_back_as_one_word() {
        for (quoting, path) in [
            (Quoting::Posix, "/bin/zsh"),
            (
                Quoting::Posix,
                "/Applications/My Shell.app/Contents/MacOS/sh",
            ),
            (Quoting::Posix, "/opt/it's/fish"),
            (Quoting::Posix, "/opt/$HOME/\"x\"\\y"),
            (Quoting::Windows, r"C:\Windows\System32\cmd.exe"),
            (Quoting::Windows, r"C:\Program Files\PowerShell\7\pwsh.exe"),
        ] {
            let quoted = quote_word_for(path, quoting);
            assert_eq!(words(&quoted, quoting), [path], "{quoted}");
        }
        assert_eq!(quote_word_for("/bin/zsh", Quoting::Posix), "/bin/zsh");
        assert_eq!(
            quote_word_for(r"C:\Program Files\Git\bin\bash.exe", Quoting::Windows),
            r#""C:\Program Files\Git\bin\bash.exe""#
        );
    }

    #[test]
    fn only_shells_named_on_their_own_start_as_login_shells() {
        assert!(is_shell_name("zsh"));
        assert!(is_shell_name("/opt/homebrew/bin/fish"));
        assert!(!is_shell_name("tmux"));
        assert!(!is_shell_name("/usr/bin/zsh5"));
    }

    #[test]
    fn etc_shells_lists_paths_without_its_comments() {
        let text = "# List of acceptable shells for chpass(1).\n\n/bin/bash\n  /bin/zsh # default\n/usr/bin/tmux\nnot-a-path\n";
        assert_eq!(
            listed_shells(text),
            ["/bin/bash", "/bin/zsh", "/usr/bin/tmux"]
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_shell_on_its_own_is_a_login_shell_and_anything_else_runs_as_written() {
        let builder = LocalShell::new("sh").command_builder().unwrap();
        assert!(builder.is_default_prog());
        let shell = builder.get_env("SHELL").unwrap().to_str().unwrap();
        assert!(shell.starts_with('/') && shell.ends_with("/sh"), "{shell}");

        let builder = LocalShell::new("/bin/sh -l").command_builder().unwrap();
        assert!(!builder.is_default_prog());
        assert_eq!(builder.get_argv(), &["/bin/sh", "-l"]);

        // 非 Shell 或不存在的程序按原样启动，以便返回具体错误。
        for command in ["/usr/bin/env", "/no/such/zsh"] {
            let builder = LocalShell::new(command).command_builder().unwrap();
            assert!(!builder.is_default_prog());
            assert_eq!(builder.get_argv(), &[command]);
        }

        assert!(
            LocalShell::default()
                .command_builder()
                .unwrap()
                .is_default_prog()
        );
        let error = LocalShell::new("sh -c 'exit")
            .command_builder()
            .unwrap_err()
            .to_string();
        assert!(error.contains("sh -c 'exit"), "{error}");
    }

    #[cfg(unix)]
    #[test]
    fn the_installed_shells_are_common_programs_here_and_include_sh() {
        let shells = installed_shells();
        assert!(shells.iter().all(|shell| is_executable(Path::new(shell))));
        assert!(
            shells.iter().all(|shell| COMMON_SHELLS
                .iter()
                .any(|name| shell.ends_with(&format!("/{name}")))),
            "{shells:?}"
        );
        assert!(
            shells.iter().any(|shell| shell.ends_with("/sh")),
            "{shells:?}"
        );
        assert!(default_shell_name().starts_with('/'));
    }

    #[cfg(unix)]
    #[test]
    fn only_common_shells_are_offered_each_program_once() {
        let listed = [
            "/bin/bash",
            "/bin/csh",
            "/bin/dash",
            "/bin/zsh",
            "/usr/bin/bash",
            "/bin/sh",
        ]
        .map(String::from);
        let folders = ["/usr/bin", "/opt/homebrew/bin"].map(PathBuf::from);
        let here = [
            "/bin/bash",
            "/bin/csh",
            "/bin/dash",
            "/bin/zsh",
            "/usr/bin/bash",
            "/usr/bin/zsh",
            "/bin/sh",
            "/opt/homebrew/bin/bash",
            "/opt/homebrew/bin/fish",
        ];
        let usable = |path: &Path| here.iter().any(|here| Path::new(here) == path);
        // 模拟合并 `/usr` 后 `/bin` 与 `/usr/bin` 指向同一目录。
        let key = |path: &Path| path.to_string_lossy().replace("/usr/bin/", "/bin/");
        assert_eq!(
            common_shells(&listed, &folders, usable, key),
            [
                "/bin/zsh",
                "/bin/bash",
                "/opt/homebrew/bin/bash",
                "/bin/sh",
                "/opt/homebrew/bin/fish",
            ]
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_starts_powershell_before_the_command_prompt() {
        let shells = installed_shells();
        assert_eq!(shells.last().map(String::as_str), Some("cmd.exe"));
        assert!(
            shells.iter().any(|shell| shell == "powershell.exe"),
            "{shells:?}"
        );
        let default = default_shell_name();
        assert!(
            matches!(default.as_str(), "pwsh.exe" | "powershell.exe"),
            "{default}"
        );
        let builder = LocalShell::default().command_builder().unwrap();
        assert!(!builder.is_default_prog());
        let program = PathBuf::from(&builder.get_argv()[0]);
        assert_eq!(
            program.file_name().unwrap().to_str(),
            Some(default.as_str())
        );
    }
}
