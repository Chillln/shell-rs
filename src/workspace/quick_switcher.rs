//! 主机、已打开标签和常用命令共用的键盘快速切换入口。

use std::rc::Rc;

use gpui_kit::component::{
    ActiveTheme as _, Icon, WindowExt as _,
    command::{Command, CommandGroup, CommandItem, CommandState},
    h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::app::{
    CatalogIcon, CenterTab, ChooseQuickTarget, CloseQuickSwitcher, NewHost, NewLocalTerminal,
    NewTemporaryConnection, OpenSettings, QuickTarget, ShowCredentials, ShowForwards,
    ShowQuickSwitcher,
};
use crate::i18n::t;
use crate::shared::RenamableTab as _;
use crate::terminal::TerminalLifecycle;

use super::Workspace;

#[derive(Clone)]
struct SwitchItem {
    id: SharedString,
    title: SharedString,
    detail: SharedString,
    icon: CatalogIcon,
    keywords: Vec<SharedString>,
    target: QuickTarget,
}

pub(super) struct QuickSwitcher {
    state: Entity<CommandState>,
    focus: FocusHandle,
    return_focus: Option<FocusHandle>,
    dispatch: FocusHandle,
    groups: Rc<[Vec<SwitchItem>; 3]>,
}

impl Workspace {
    pub(super) fn on_show_quick_switcher(
        &mut self,
        _: &ShowQuickSwitcher,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if window.has_active_dialog(cx) {
            return;
        }
        if let Some(switcher) = &self.quick_switcher {
            let state = switcher.read(cx).state.clone();
            state.update(cx, |state, cx| state.focus(window, cx));
            return;
        }
        let store = self.store.read(cx);
        let host_item = |id| {
            let host = store.host(id)?;
            let detail: SharedString =
                format!("{}@{}:{}", host.user, host.address, host.port).into();
            let group = host
                .group
                .map(|id| store.group_path(id))
                .unwrap_or_default();
            Some(SwitchItem {
                id: format!("switch-host:{}", id.0).into(),
                title: host.name.clone(),
                detail: detail.clone(),
                icon: CatalogIcon::Server,
                keywords: vec![host.name.clone(), detail, group.into()],
                target: QuickTarget::Host(id),
            })
        };
        let mut tabs = Vec::new();
        for (&id, panel) in &self.terminals {
            let panel = panel.read(cx);
            if let Some(mut item) = host_item(panel.host_id()) {
                item.title = panel.tab_title(cx);
                item.id = format!("switch-terminal:{}", id.0).into();
                item.icon = CatalogIcon::Terminal;
                item.target = QuickTarget::Tab(CenterTab::Terminal(id));
                tabs.push(item);
            }
        }
        for (&id, panel) in &self.explorers {
            let panel = panel.read(cx);
            if let Some(mut item) = host_item(panel.host_id()) {
                item.title = panel.tab_title(cx);
                item.id = format!("switch-explorer:{}", id.0).into();
                item.icon = CatalogIcon::FolderTree;
                item.target = QuickTarget::Tab(CenterTab::Explorer(id));
                tabs.push(item);
            }
        }
        for (&id, panel) in &self.local_terminals {
            let panel = panel.read(cx);
            let title = panel
                .terminal()
                .read(cx)
                .title(cx)
                .map(SharedString::from)
                .unwrap_or_else(|| panel.default_title());
            tabs.push(SwitchItem {
                id: format!("switch-local:{}", id.0).into(),
                title,
                detail: t!("workspace.title_bar.local_terminal"),
                icon: CatalogIcon::Terminal,
                keywords: vec![],
                target: QuickTarget::Tab(CenterTab::LocalTerminal(id)),
            });
        }
        for (&id, panel) in &self.editors {
            let panel = panel.read(cx);
            let detail: SharedString = panel.location().path().into();
            let mut keywords = panel
                .host_id()
                .and_then(host_item)
                .map(|item| item.keywords)
                .unwrap_or_default();
            keywords.push(detail.clone());
            tabs.push(SwitchItem {
                id: format!("switch-editor:{}", id.0).into(),
                title: panel.name().into(),
                detail: detail.clone(),
                icon: CatalogIcon::FileText,
                keywords,
                target: QuickTarget::Tab(CenterTab::Editor(id)),
            });
        }
        if self.settings_tab.is_some() {
            tabs.push(SwitchItem {
                id: "switch-settings".into(),
                title: t!("workspace.sidebar.settings"),
                detail: SharedString::default(),
                icon: CatalogIcon::Settings,
                keywords: vec![],
                target: QuickTarget::Tab(CenterTab::Settings),
            });
        }
        tabs.sort_by(|a, b| {
            let active = |item: &SwitchItem| matches!(item.target, QuickTarget::Tab(tab) if Some(tab) == self.active_tab);
            active(b).cmp(&active(a)).then_with(|| a.title.cmp(&b.title)).then_with(|| a.id.cmp(&b.id))
        });
        if let Some(item) = tabs.first_mut()
            && matches!(item.target, QuickTarget::Tab(tab) if Some(tab) == self.active_tab)
        {
            item.detail = format!("{} · {}", t!("switcher.current"), item.detail).into();
        }
        let mut hosts: Vec<_> = store
            .hosts()
            .iter()
            .filter_map(|host| host_item(host.id))
            .collect();
        hosts.sort_by(|a, b| a.title.cmp(&b.title).then_with(|| a.id.cmp(&b.id)));
        let commands = [
            (
                QuickTarget::NewHost,
                "switch-new-host",
                t!("app.shortcut.new_host"),
                CatalogIcon::Server,
            ),
            (
                QuickTarget::LocalTerminal,
                "switch-new-local",
                t!("workspace.title_bar.new_local_terminal"),
                CatalogIcon::Terminal,
            ),
            (
                QuickTarget::TemporaryConnection,
                "switch-temporary",
                t!("workspace.title_bar.quick_connect"),
                CatalogIcon::Plug,
            ),
            (
                QuickTarget::Settings,
                "switch-open-settings",
                t!("workspace.sidebar.open_settings"),
                CatalogIcon::Settings,
            ),
            (
                QuickTarget::Credentials,
                "switch-credentials",
                t!("workspace.title_bar.credentials"),
                CatalogIcon::KeyRound,
            ),
            (
                QuickTarget::Forwards,
                "switch-forwards",
                t!("workspace.title_bar.forwards"),
                CatalogIcon::ArrowLeftRight,
            ),
        ]
        .into_iter()
        .map(|(target, id, title, icon)| SwitchItem {
            id: id.into(),
            title,
            detail: SharedString::default(),
            icon,
            keywords: vec![],
            target,
        })
        .collect();
        let groups = Rc::new([tabs, hosts, commands]);
        let return_focus = window.focused(cx);
        let dispatch = self.focus_handle.clone();
        let switcher = cx.new(|cx| {
            let state = cx.new(|cx| CommandState::new(window, cx));
            state.update(cx, |state, cx| state.focus(window, cx));
            QuickSwitcher {
                state,
                focus: cx.focus_handle(),
                return_focus,
                dispatch,
                groups,
            }
        });
        self.quick_switcher = Some(switcher);
        cx.notify();
    }

    pub(super) fn on_close_quick_switcher(
        &mut self,
        _: &CloseQuickSwitcher,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(switcher) = self.quick_switcher.take() {
            if let Some(focus) = switcher.read(cx).return_focus.clone() {
                window.focus(&focus, cx);
            }
            cx.notify();
        }
    }

    pub(super) fn on_choose_quick_target(
        &mut self,
        action: &ChooseQuickTarget,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.on_close_quick_switcher(&CloseQuickSwitcher, window, cx);
        match action.target() {
            QuickTarget::Tab(tab) => self.show_center_tab(tab, window, cx),
            QuickTarget::Host(id) => {
                let tab = self
                    .terminals
                    .iter()
                    .filter(|(_, panel)| {
                        let panel = panel.read(cx);
                        panel.host_id() == id
                            && matches!(
                                panel.lifecycle(cx),
                                TerminalLifecycle::Running | TerminalLifecycle::Starting
                            )
                    })
                    .min_by_key(|(id, _)| id.0)
                    .map(|(id, _)| CenterTab::Terminal(*id));
                if let Some(tab) = tab {
                    self.show_center_tab(tab, window, cx);
                } else {
                    self.connect_host(id, window, cx);
                }
            }
            target => {
                let action: Box<dyn Action> = match target {
                    QuickTarget::NewHost => Box::new(NewHost),
                    QuickTarget::LocalTerminal => Box::new(NewLocalTerminal),
                    QuickTarget::TemporaryConnection => Box::new(NewTemporaryConnection),
                    QuickTarget::Settings => Box::new(OpenSettings),
                    QuickTarget::Credentials => Box::new(ShowCredentials),
                    QuickTarget::Forwards => Box::new(ShowForwards),
                    _ => unreachable!(),
                };
                let dispatch = self.focus_handle.clone();
                // 退出工作区动作处理器后再派发，避免常用命令重入同一实体。
                window.defer(cx, move |window, cx| {
                    dispatch.dispatch_action(action.as_ref(), window, cx)
                });
            }
        }
    }

    fn show_center_tab(&mut self, tab: CenterTab, window: &mut Window, cx: &mut Context<Self>) {
        if let Some((group, panel)) = self.center_tab_location(tab, cx) {
            group.update(cx, |group, cx| {
                if let Some(ix) = group
                    .panels()
                    .iter()
                    .position(|candidate| candidate.panel_id(cx) == panel)
                {
                    group.select_tab(ix, window, cx);
                }
            });
            // 已经显示的标签不会触发 Dock 激活，也需要把焦点交回其内容。
            if let Some(focus) = self.tab_focus_handle(tab, cx) {
                window.focus(&focus, cx);
            }
        }
    }
}

impl Render for QuickSwitcher {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let items = self.groups.clone();
        let dispatch = self.dispatch.clone();
        let cancel = self.dispatch.clone();
        let backdrop_close = self.dispatch.clone();
        let mut command = Command::new(&self.state)
            .bordered(false)
            .placeholder(t!("switcher.search"))
            .max_h(window.viewport_size().height * 0.6)
            .empty(|_, _, cx| {
                div()
                    .p_4()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(t!("switcher.empty"))
            })
            .footer(|_, _, cx| {
                div()
                    .px_3()
                    .py_2()
                    .border_t_1()
                    .border_color(cx.theme().border)
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(t!("switcher.help"))
            })
            .on_confirm(move |index, window, cx| {
                if let Some(item) = items
                    .get(index.section)
                    .and_then(|group| group.get(index.row))
                {
                    dispatch.dispatch_action(&ChooseQuickTarget::new(item.target), window, cx);
                }
            })
            .on_cancel(move |window, cx| cancel.dispatch_action(&CloseQuickSwitcher, window, cx));
        for (group, title) in self.groups.iter().zip([
            t!("switcher.tabs"),
            t!("switcher.hosts"),
            t!("switcher.commands"),
        ]) {
            command = command.group(CommandGroup::new().label(title).items(
                group.iter().cloned().map(|item| {
                    CommandItem::new()
                        .label(item.title.clone())
                        .keywords(
                            item.keywords
                                .iter()
                                .cloned()
                                .chain(std::iter::once(item.detail.clone())),
                        )
                        .child(move |_, cx| {
                            // 行渲染只消费普通快照，不在 CommandState 渲染期间读取其他实体。
                            h_flex()
                                .id(item.id.clone())
                                .test_support()
                                .aria_label(format!("{} {}", item.title, item.detail))
                                .w_full()
                                .gap_2()
                                .child(
                                    div()
                                        .id(SharedString::from(format!("{}:icon", item.id)))
                                        .test_support()
                                        .size_4()
                                        .flex_none()
                                        .child(
                                            Icon::new(item.icon)
                                                .size_4()
                                                .text_color(cx.theme().muted_foreground),
                                        ),
                                )
                                .child(
                                    v_flex()
                                        .min_w_0()
                                        .flex_1()
                                        .child(
                                            div()
                                                .id(SharedString::from(format!(
                                                    "{}:title",
                                                    item.id
                                                )))
                                                .test_support()
                                                .truncate()
                                                .child(item.title.clone()),
                                        )
                                        .when(!item.detail.is_empty(), |column| {
                                            column.child(
                                                div()
                                                    .id(SharedString::from(format!(
                                                        "{}:detail",
                                                        item.id
                                                    )))
                                                    .test_support()
                                                    .truncate()
                                                    .text_xs()
                                                    .text_color(cx.theme().muted_foreground)
                                                    .child(item.detail.clone()),
                                            )
                                        }),
                                )
                        })
                }),
            ));
        }
        let paddings = gpui_kit::component::window_paddings(window);
        let width = rems(38.)
            .to_pixels(window.rem_size())
            .min(window.viewport_size().width * 0.9);
        // Base 的焦点陷阱和关闭行为包住标准 Command，使搜索字段可立即输入。
        gpui_kit::base::Dialog::new(cx)
            .focus_handle(self.focus.clone())
            .request_close(move |_, window, cx| {
                backdrop_close.dispatch_action(&CloseQuickSwitcher, window, cx)
            })
            .backdrop(div().size_full().bg(cx.theme().overlay))
            .popup(
                div()
                    .id("quick-switcher")
                    .test_support()
                    .aria_label(t!("switcher.title"))
                    .absolute()
                    .left((window.viewport_size().width - width) / 2.)
                    .top(
                        paddings.top
                            + (window.viewport_size().height - paddings.top - paddings.bottom)
                                * 0.1,
                    )
                    .w(width)
                    .rounded(cx.theme().radius_lg)
                    .border_1()
                    .border_color(cx.theme().border)
                    .bg(cx.theme().popover)
                    .text_color(cx.theme().popover_foreground)
                    .shadow_lg()
                    .overflow_hidden()
                    .occlude()
                    .child(command),
            )
    }
}
