//! A container's details: the dialog a click on it in the Docker tool
//! opens.

use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _, WindowExt as _,
    button::{Button, ButtonVariants as _},
    dialog::DialogFooter,
    h_flex,
    tab::{Tab, TabBar},
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::details::{Read, message, reading, render_sections, run};
use super::docker_panel::state_color;
use super::linux::{inspect_command, logs_command_in_range, parse_details};
use super::model::{Container, ContainerCommand, ContainerDetails, ContainerSubject, DockerObject};
use crate::app::{CatalogIcon, ControlContainers, OpenContainerTerminal, RemoveDockerObject};
use crate::i18n::t;
use crate::shared::soft_tag;
use crate::terminal::{LogView, RemoteTerminalId, TerminalView};

/// How wide the dialog is: long IDs, image names and environment lines.
const DIALOG_WIDTH: f32 = 46.;

/// Open the details of a container of the host of `terminal`, whose
/// connection reads them. `dispatch` is the workspace's focus handle: the
/// dialog's buttons close it and dispatch there.
pub fn open_container_dialog(
    container: Container,
    terminal: WeakEntity<TerminalView>,
    origin: RemoteTerminalId,
    dispatch: FocusHandle,
    window: &mut Window,
    cx: &mut App,
) {
    let view = cx.new(|cx| {
        ContainerDetailsView::new(container.clone(), terminal, dispatch.clone(), window, cx)
    });
    window.open_dialog(cx, move |dialog, window, cx| {
        let color = state_color(container.state, cx);
        let title = v_flex()
            .gap_1()
            .child(
                h_flex()
                    .gap_2()
                    .child(div().truncate().child(container.name.clone()))
                    .child(soft_tag(container.state.label(), color)),
            )
            .child(
                div()
                    .truncate()
                    .text_sm()
                    .font_weight(FontWeight::NORMAL)
                    .text_color(cx.theme().muted_foreground)
                    .child(container.image.clone()),
            );
        let subject = ContainerSubject::Container(container.name.clone());
        let control = |command: ContainerCommand| {
            let (dispatch, subject, id) = (dispatch.clone(), subject.clone(), container.id.clone());
            let button = Button::new(SharedString::from(format!(
                "container-dialog-{}",
                command.verb()
            )))
            .icon(Icon::new(match command {
                ContainerCommand::Start => CatalogIcon::Play,
                ContainerCommand::Stop => CatalogIcon::Square,
                ContainerCommand::Restart => CatalogIcon::RotateCw,
            }))
            .label(command.label())
            .on_click(move |_, window, cx| {
                // One dialog at a time: a confirmation takes its place.
                window.close_dialog(cx);
                dispatch.dispatch_action(
                    &ControlContainers {
                        subject: subject.clone(),
                        ids: vec![id.clone()],
                        command,
                    },
                    window,
                    cx,
                );
            });
            match command {
                ContainerCommand::Stop => button.danger(),
                ContainerCommand::Start => button.primary(),
                ContainerCommand::Restart => button.outline(),
            }
        };
        let remove = {
            let dispatch = dispatch.clone();
            let action = RemoveDockerObject {
                object: DockerObject::Container,
                id: container.id.clone(),
                name: container.name.clone(),
            };
            Button::new("container-dialog-remove")
                .outline()
                .icon(Icon::new(CatalogIcon::Trash))
                .label(t!("common.delete"))
                .on_click(move |_, window, cx| {
                    window.close_dialog(cx);
                    dispatch.dispatch_action(&action, window, cx);
                })
        };
        let up = container.state.is_up();
        let enter = {
            let dispatch = dispatch.clone();
            let action = OpenContainerTerminal::new(origin, container.id.clone());
            Button::new("container-terminal")
                .outline()
                .disabled(!up)
                .icon(Icon::new(CatalogIcon::Terminal))
                .label(t!("docker.open_terminal"))
                .tooltip(if up {
                    t!("docker.open_terminal")
                } else {
                    t!("docker.terminal_stopped")
                })
                .on_click(move |_, window, cx| {
                    window.close_dialog(cx);
                    dispatch.dispatch_action(&action, window, cx);
                })
        };
        let footer = DialogFooter::new()
            .justify_start()
            .child(enter)
            .child(control(if up {
                ContainerCommand::Stop
            } else {
                ContainerCommand::Start
            }))
            .child(control(ContainerCommand::Restart))
            // Docker removes a running container only by force.
            .when(!up, |footer| footer.child(remove));
        dialog
            .title(title)
            // Dialog geometry is an API boundary that takes `Pixels`; the
            // width follows the interface zoom through the rem.
            .w(rems(DIALOG_WIDTH).to_pixels(window.rem_size()))
            .max_h(window.viewport_size().height * 0.9)
            // Closed by its buttons or Escape, like every dialog.
            .overlay_closable(false)
            .child(view.clone())
            .footer(footer)
    });
}

/// The dialog's body: what `docker inspect` says about the container, and
/// its output.
pub struct ContainerDetailsView {
    details: Read<ContainerDetails>,
    output: Entity<LogView>,
    /// 0 the details, 1 the output.
    tab: usize,
    _details_task: Task<()>,
}

impl ContainerDetailsView {
    fn new(
        container: Container,
        terminal: WeakEntity<TerminalView>,
        dispatch: FocusHandle,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let details = run(
            &terminal,
            inspect_command(DockerObject::Container, &container.id),
            cx,
            |this, answer| {
                this.details = match answer {
                    Ok(output) => match parse_details(&output) {
                        Some(details) => Read::Known(details),
                        None => Read::Unknown(t!("docker.details.container_gone").into()),
                    },
                    Err(why) => Read::Unknown(why),
                };
            },
        );
        let id = container.id.clone();
        let output = cx.new(|cx| {
            LogView::new(
                "container-output",
                terminal,
                dispatch,
                move |seconds| logs_command_in_range(&id, seconds),
                window,
                cx,
            )
        });
        Self {
            details: Read::Reading,
            output,
            tab: 0,
            _details_task: details,
        }
    }

    fn select(&mut self, tab: usize, cx: &mut Context<Self>) {
        self.tab = tab;
        self.output
            .update(cx, |output, cx| output.set_visible(tab == 1, cx));
        cx.notify();
    }

    fn render_details(&self, cx: &App) -> AnyElement {
        let details = match &self.details {
            Read::Known(details) => details,
            Read::Unknown(why) => return message("container-details-unknown", why.clone(), cx),
            Read::Reading => return reading("container-details-reading", cx),
        };
        v_flex()
            .id("container-details")
            .test_support()
            .child(render_sections("container", &details.sections(), cx))
            .into_any_element()
    }
}

impl Render for ContainerDetailsView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .gap_3()
            .child(
                div().id("container-tabs").test_support().child(
                    TabBar::new("container-tab-bar")
                        .underline()
                        .small()
                        .selected_index(self.tab)
                        .on_click(cx.listener(|this, index: &usize, _, cx| this.select(*index, cx)))
                        .child(
                            Tab::new()
                                .icon(Icon::new(IconName::Info))
                                .label(t!("docker.details.tab_details")),
                        )
                        .child(
                            Tab::new()
                                .icon(Icon::new(CatalogIcon::FileText))
                                .label(t!("docker.details.tab_logs")),
                        ),
                ),
            )
            .child(if self.tab == 0 {
                self.render_details(cx)
            } else {
                self.output.clone().into_any_element()
            })
    }
}
