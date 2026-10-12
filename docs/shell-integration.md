# 目录报告与历史命令预测

ShellRS 使用 OSC 7 获取工作目录，使用 OSC 133 的提示符边界判断是否正在输入 shell 命令。未收到提示符边界时不会显示预测；密码、程序交互、全屏程序和输入法组合输入期间也不会显示。预测从当前连接用户的历史文件读取，保留在内存中；右方向键采纳剩余文字，回车仍由你决定。

如果 shell 已经提供 OSC 7 / OSC 133 集成，无需重复配置。普通 Bash 可以把下面这段放到 `~/.bashrc` 的末尾，重新连接后生效。它保留已有的 `PROMPT_COMMAND`，不会修改历史保存规则。

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

Fish 自带历史预测，ShellRS 在光标后已有文字时会保留 Fish 的原有操作，不叠加第二份预测。其他 shell 或复杂提示符主题应使用其提供的 OSC 133 集成；只报告目录的 OSC 7 不会启用命令预测。

目前预测只处理单行、光标在末尾且已回显的命令。命令换行、滚动查看历史、断开或重连后会隐藏建议；多行历史命令仍可在右侧「历史命令」中查看和输入。
