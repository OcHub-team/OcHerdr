use super::*;

pub(super) fn host_nav_heading(label: &'static str) -> ochub_ui::gpui::AnyElement {
    div()
        .px_2()
        .pt_4()
        .pb_1()
        .text_xs()
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(theme::muted())
        .child(label)
        .into_any_element()
}

pub(super) fn selection_mark(selected: bool) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .justify_center()
        .size(px(20.))
        .rounded(px(CORNER_COMPACT))
        .border_1()
        .border_color(if selected {
            theme::accent()
        } else {
            theme::border_strong()
        })
        .bg(if selected {
            theme::accent_fill()
        } else {
            theme::surface()
        })
        .when(selected, |mark| {
            mark.child(icon(IconName::Check, theme::accent_text(), 12.))
        })
}

pub(super) fn host_pill(label: &'static str) -> impl IntoElement {
    div()
        .px_2()
        .py_1()
        .rounded(px(CORNER_COMPACT))
        .bg(theme::accent_soft())
        .text_xs()
        .text_color(theme::accent())
        .child(label)
}

pub(super) fn inspector_row(label: &'static str, value: String) -> ochub_ui::gpui::AnyElement {
    row()
        .child(row_label(label, None))
        .child(
            div()
                .flex_none()
                .max_w(px(240.))
                .min_w_0()
                .truncate()
                .text_sm()
                .text_color(theme::muted())
                .child(value),
        )
        .into_any_element()
}

pub(super) fn readonly_field_control(value: String) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .h(px(34.))
        .w_full()
        .px_3()
        .rounded(px(CORNER_CONTROL))
        .border_1()
        .border_color(theme::border())
        .bg(theme::inset())
        .text_sm()
        .text_color(theme::subtext())
        .child(value)
}

pub(super) fn host_health_summary(
    health: Option<&HostHealthView>,
    i18n: I18n,
) -> (ochub_ui::gpui::Rgba, &'static str) {
    match health {
        Some(HostHealthView::Checking { .. }) => {
            (theme::yellow(), i18n.text(k::HOSTS_HEALTH_CHECKING))
        }
        Some(HostHealthView::Checked { cached, .. }) => match cached.status {
            HostHealthStatus::Ready => (theme::green(), i18n.text(k::HOSTS_HEALTH_READY)),
            HostHealthStatus::SshOnly => {
                (theme::yellow(), i18n.text(k::HOSTS_HEALTH_HERDR_NOT_READY))
            }
            HostHealthStatus::UnsupportedHerdr => {
                (theme::yellow(), i18n.text(k::HOSTS_HEALTH_UPDATE_REQUIRED))
            }
            HostHealthStatus::AuthenticationRequired => {
                (theme::yellow(), i18n.text(k::HOSTS_HEALTH_AUTH_REQUIRED))
            }
            HostHealthStatus::HostKeyRequired => {
                (theme::yellow(), i18n.text(k::HOSTS_HEALTH_HOST_KEY))
            }
            HostHealthStatus::Unreachable => (theme::red(), i18n.text(k::HOSTS_HEALTH_UNREACHABLE)),
            HostHealthStatus::Failed => (theme::red(), i18n.text(k::HOSTS_HEALTH_FAILED)),
        },
        None => (theme::muted(), i18n.text(k::HOSTS_HEALTH_NOT_CHECKED)),
    }
}

pub(super) fn host_health_guidance(status: HostHealthStatus, i18n: I18n) -> &'static str {
    i18n.text(match status {
        HostHealthStatus::Ready => k::HOSTS_HEALTH_GUIDANCE_READY,
        HostHealthStatus::SshOnly => k::HOSTS_HEALTH_GUIDANCE_SSH_ONLY,
        HostHealthStatus::UnsupportedHerdr => k::HOSTS_HEALTH_GUIDANCE_UNSUPPORTED,
        HostHealthStatus::AuthenticationRequired => k::HOSTS_HEALTH_GUIDANCE_AUTH,
        HostHealthStatus::HostKeyRequired => k::HOSTS_HEALTH_GUIDANCE_HOST_KEY,
        HostHealthStatus::Unreachable => k::HOSTS_HEALTH_GUIDANCE_UNREACHABLE,
        HostHealthStatus::Failed => k::HOSTS_HEALTH_GUIDANCE_FAILED,
    })
}

pub(super) fn health_surface(health: Option<&HostHealthView>) -> ochub_ui::gpui::Rgba {
    match health {
        Some(HostHealthView::Checked { cached, .. }) => match cached.status {
            HostHealthStatus::Ready => theme::green_soft(),
            HostHealthStatus::Unreachable | HostHealthStatus::Failed => theme::red_soft(),
            _ => theme::yellow_soft(),
        },
        Some(HostHealthView::Checking { .. }) => theme::yellow_soft(),
        None => theme::inset(),
    }
}
