//! 设置中的本地终端 Shell 编辑器：输入程序及参数，或从已安装列表和系统文件选择器中选取。
//! 设置即时保存，新建或重启本地终端时生效。

use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName, Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    input::{Input, InputEvent, InputState},
    menu::{DropdownMenu as _, PopupMenuItem},
    setting::RenderOptions,
};
use gpui_kit::*;

use crate::app::CatalogIcon;
use crate::i18n::{UiLocale, t};
use crate::terminal::{default_shell_name, installed_shells, quote_word};

use super::SettingsStore;

/// 保留输入光标和候选列表，并与设置存储同步。
pub struct LocalShellEditor {
    store: Entity<SettingsStore>,
    input: Entity<InputState>,
    /// 后台扫描完成后保存默认 Shell 和已安装的 Shell。
    found: Option<FoundShells>,
    _subscriptions: [Subscription; 3],
    _scan: Task<()>,
}

struct FoundShells {
    default: String,
    installed: Vec<String>,
}

impl LocalShellEditor {
    pub fn new(store: Entity<SettingsStore>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let shell = stored(&store, cx).unwrap_or_default();
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(t!("settings.local_terminal.automatic_unknown"))
                .default_value(shell)
        });
        let subscriptions = [
            cx.subscribe(&input, |this, input, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    let shell = input.read(cx).value().to_string();
                    this.write(Some(shell), cx);
                }
            }),
            // 输入本身只经过首尾空白规范化；重置或重新读取设置时才同步到输入框。
            cx.observe_in(&store, window, |this, store, window, cx| {
                let shell = stored(&store, cx).unwrap_or_default();
                if this.input.read(cx).value().trim() != shell {
                    this.input
                        .update(cx, |input, cx| input.set_value(shell, window, cx));
                }
            }),
            cx.observe_global_in::<UiLocale>(window, |this, window, cx| {
                this.show_placeholder(window, cx)
            }),
        ];
        let scan = cx.spawn_in(window, async move |this, cx| {
            let found = cx
                .background_spawn(async {
                    FoundShells {
                        default: default_shell_name(),
                        installed: installed_shells(),
                    }
                })
                .await;
            this.update_in(cx, |this, window, cx| {
                this.found = Some(found);
                this.show_placeholder(window, cx);
                cx.notify();
            })
            .ok();
        });
        Self {
            store,
            input,
            found: None,
            _subscriptions: subscriptions,
            _scan: scan,
        }
    }

    /// 默认 Shell 已知时显示「自动检测（/bin/zsh）」。
    fn automatic_label(&self) -> SharedString {
        match &self.found {
            Some(found) => t!(
                "settings.local_terminal.automatic",
                shell = found.default.as_str()
            ),
            None => t!("settings.local_terminal.automatic_unknown"),
        }
    }

    /// 用占位文案说明留空时运行的程序。
    fn show_placeholder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let placeholder = self.automatic_label();
        self.input.update(cx, |input, cx| {
            input.set_placeholder(placeholder, window, cx)
        });
    }

    /// 选择 Shell；`None` 表示恢复系统默认值。
    pub(super) fn choose(
        &mut self,
        shell: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.input.update(cx, |input, cx| {
            input.set_value(shell.clone().unwrap_or_default(), window, cx)
        });
        self.write(shell, cx);
    }

    /// 通过系统文件选择器选取程序。
    fn browse(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let receiver = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(t!("settings.local_terminal.browse_prompt")),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = receiver.await else {
                return;
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            this.update_in(cx, |this, window, cx| {
                this.choose(Some(quote_word(&path.to_string_lossy())), window, cx)
            })
            .ok();
        })
        .detach();
    }

    fn write(&self, shell: Option<String>, cx: &mut Context<Self>) {
        self.store.update(cx, |store, cx| {
            store.update(|settings| settings.local_terminal.shell = shell, cx)
        });
    }
}

/// 读取保存的 Shell；`None` 表示系统默认值。
fn stored(store: &Entity<SettingsStore>, cx: &App) -> Option<String> {
    store.read(cx).settings().local_terminal.shell
}

/// 渲染输入框、已安装 Shell 菜单和文件选择按钮。
pub(super) fn local_shell_field(
    editor: &Entity<LocalShellEditor>,
    options: &RenderOptions,
    cx: &App,
) -> Div {
    let this = editor.read(cx);
    let current = stored(&this.store, cx);
    let automatic = this.automatic_label();
    let installed = this
        .found
        .as_ref()
        .map(|found| found.installed.clone())
        .unwrap_or_default();
    let (menu_editor, browse_editor) = (editor.downgrade(), editor.downgrade());
    let menu = Button::new("local-shell-menu")
        .ghost()
        .xsmall()
        .icon(IconName::ChevronDown)
        .tooltip(t!("settings.local_terminal.installed"))
        .dropdown_menu_with_anchor(Anchor::TopRight, move |menu, _, _| {
            let item = |label: SharedString, shell: Option<String>| {
                let editor = menu_editor.clone();
                PopupMenuItem::new(label)
                    .checked(current == shell)
                    .on_click(move |_, window, cx| {
                        editor
                            .update(cx, |editor, cx| editor.choose(shell.clone(), window, cx))
                            .ok();
                    })
            };
            let mut menu = menu.item(item(automatic.clone(), None));
            if !installed.is_empty() {
                menu = menu.separator();
            }
            installed.iter().fold(menu, |menu, shell| {
                menu.item(item(shell.clone().into(), Some(shell.clone())))
            })
        });
    h_flex()
        .w_full()
        .gap_2()
        .child(
            Input::new(&this.input)
                .id("local-shell")
                .aria_label(t!("settings.local_terminal.shell"))
                .with_size(options.size())
                .font_family(cx.theme().mono_font_family.clone())
                .suffix(menu)
                .flex_1()
                .min_w_0(),
        )
        .child(
            Button::new("browse-local-shell")
                .outline()
                .with_size(options.size())
                .icon(Icon::new(CatalogIcon::FileSearch))
                .label(t!("settings.local_terminal.browse"))
                .on_click(move |_, window, cx| {
                    browse_editor
                        .update(cx, |editor, cx| editor.browse(window, cx))
                        .ok();
                }),
        )
}
