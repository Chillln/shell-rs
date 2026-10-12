# 目录报告与历史命令预测

ShellRS 的命令预测直接读取远端历史文件，去重后按最近使用顺序提示，只在当前连接的内存中缓存，不在本地保存命令。普通 Bash 的 `user@host:目录$` / `root@host:目录#` 和 `[user@host 目录]$` / `#`、Zsh 默认的 `hostname%` 提示符无需修改配置。输入内容与终端回显一致后才显示建议；密码提示、全屏程序和输入法组合输入期间不显示。右方向键采纳剩余文字，回车仍由你决定。

预测只匹配远端历史文件已有的命令。例如已有 `htop`，输入 `h` 才会出现相应建议。Bash 的新命令通常在 shell 退出时才写入文件；可先执行 `history -a`，再刷新右侧「历史命令」。ShellRS 不会自动修改远端的历史保存规则。

目录联动另外使用 OSC 7 获取工作目录。自定义提示符无法自动识别时，可以同时添加 OSC 133 的提示符边界。

如果 shell 已经提供 OSC 7 / OSC 133 集成，无需重复配置。需要目录联动或使用自定义提示符的 Bash，可以把下面这段放到远端 `~/.bashrc` 的末尾，然后执行 `source ~/.bashrc` 或重新连接。它保留已有的 `PROMPT_COMMAND`，不会修改历史保存规则。

```bash
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
}
if [[ ${PROMPT_COMMAND[*]} != *__shellrs_prompt* ]]; then
    PROMPT_COMMAND=("${PROMPT_COMMAND[@]}" __shellrs_prompt)
fi
if [[ $PS1 != *'\[\e]133;B\a\]'* ]]; then
    PS1+='\[\e]133;B\a\]'
fi
```

Zsh 可以保留原来的提示符，在 `~/.zshrc` 末尾追加提示符边界：

```zsh
autoload -Uz add-zsh-hook
__shellrs_prompt() { printf '\033]133;A\007' }
add-zsh-hook precmd __shellrs_prompt
PROMPT+=$'%{\e]133;B\a%}'
```

Fish 自带历史预测，ShellRS 在光标后已有文字时会保留 Fish 的原有操作，不叠加第二份预测。其他 shell 或复杂提示符主题可使用其提供的 OSC 133 集成；OSC 7 只报告工作目录。

目前预测只处理单行、光标在末尾且已回显的命令。命令换行、滚动查看历史、断开或重连后会隐藏建议；多行历史命令仍可在右侧「历史命令」中查看和输入。
