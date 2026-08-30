use std::sync::mpsc::{self, TryRecvError};
use std::time::Duration;

use ochub_ui::notifications::{NotificationLevel, NotificationRequest};

use super::*;

#[cfg(not(test))]
const AUTO_CHECK_INITIAL_DELAY: Duration = Duration::from_secs(30);
#[cfg(not(test))]
const AUTO_CHECK_POLL_INTERVAL: Duration = Duration::from_secs(60 * 60);
const PROGRESS_POLL_INTERVAL: Duration = Duration::from_millis(100);

impl OcHerdrView {
    pub(crate) fn open_update_dialog(&mut self, cx: &mut Context<Self>) {
        if self.update_installing {
            return;
        }
        if self.update_checking {
            self.set_overlay(Overlay::Update(UpdateDialog::Checking), cx);
            return;
        }
        if let Some(info) = self.update_info.clone().filter(|info| info.has_update) {
            self.set_overlay(Overlay::Update(UpdateDialog::Available(info)), cx);
            return;
        }
        self.start_update_check(true, cx);
    }

    fn start_update_check(&mut self, manual: bool, cx: &mut Context<Self>) {
        if self.update_checking || self.update_installing {
            return;
        }
        self.update_checking = true;
        if manual {
            self.set_overlay(Overlay::Update(UpdateDialog::Checking), cx);
        }

        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async { crate::update::check_for_updates() })
                .await;
            this.update(cx, |this, cx| {
                this.update_checking = false;
                this.update_state.last_check_at = Some(crate::update::now_timestamp());

                match result {
                    Ok(info) => {
                        let notify =
                            !manual && crate::update::should_notify(&info, &this.update_state);
                        if notify {
                            this.update_state.notified_version = info.latest_version.clone();
                        }
                        let _ = crate::update::save_state(&this.update_state);
                        this.update_info = Some(info.clone());

                        if manual && matches!(this.overlay, Overlay::Update(_)) {
                            let dialog = if info.has_update {
                                UpdateDialog::Available(info.clone())
                            } else {
                                UpdateDialog::Current {
                                    version: info.current_version.clone(),
                                }
                            };
                            this.set_overlay(Overlay::Update(dialog), cx);
                        }
                        if notify {
                            let latest = info.latest_version.as_deref().unwrap_or_default();
                            let request = NotificationRequest::new(
                                NotificationLevel::Info,
                                crate::tf!(this.i18n, k::UPDATE_AVAILABLE, latest = latest),
                            )
                            .message(this.i18n.text(k::UPDATE_AUTO_DETAIL));
                            this.notifications
                                .update(cx, |host, cx| host.notify(request, cx));
                        }
                    }
                    Err(error) => {
                        let _ = crate::update::save_state(&this.update_state);
                        if manual && matches!(this.overlay, Overlay::Update(_)) {
                            this.set_overlay(
                                Overlay::Update(UpdateDialog::Failed {
                                    message: error.to_string(),
                                    release_url: crate::update::latest_release_url(),
                                }),
                                cx,
                            );
                        }
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    #[cfg(not(test))]
    pub(crate) fn spawn_auto_update_check(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(AUTO_CHECK_INITIAL_DELAY)
                .await;
            loop {
                let alive = this
                    .update(cx, |this, cx| {
                        if crate::update::auto_check_due(
                            &this.update_state,
                            crate::update::now_timestamp(),
                        ) {
                            this.start_update_check(false, cx);
                        }
                    })
                    .is_ok();
                if !alive {
                    break;
                }
                cx.background_executor()
                    .timer(AUTO_CHECK_POLL_INTERVAL)
                    .await;
            }
        })
        .detach();
    }

    pub(crate) fn confirm_update_dialog(&mut self, cx: &mut Context<Self>) {
        match self.overlay.clone() {
            Overlay::Update(UpdateDialog::Available(info)) if info.can_self_install => {
                self.install_update(info, cx);
            }
            Overlay::Update(UpdateDialog::Available(info)) => {
                self.open_update_release_page(info.release_url, cx);
            }
            Overlay::Update(UpdateDialog::Failed { .. }) => self.start_update_check(true, cx),
            Overlay::Update(UpdateDialog::Current { .. }) => {
                self.set_overlay(Overlay::None, cx);
            }
            _ => {}
        }
    }

    pub(crate) fn close_update_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.update_installing {
            return;
        }
        if matches!(self.overlay, Overlay::Update(_)) {
            self.set_overlay(Overlay::None, cx);
            self.focus.focus(window, cx);
        }
    }

    pub(crate) fn install_update(
        &mut self,
        info: crate::update::UpdateInfo,
        cx: &mut Context<Self>,
    ) {
        if self.update_installing || !info.can_self_install {
            return;
        }
        let version = info
            .latest_version
            .clone()
            .unwrap_or_else(|| info.current_version.clone());
        self.update_installing = true;
        self.set_overlay(
            Overlay::Update(UpdateDialog::Downloading {
                version,
                downloaded: 0,
                total: None,
            }),
            cx,
        );

        let (progress_tx, progress_rx) = mpsc::channel::<(u64, Option<u64>)>();
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(PROGRESS_POLL_INTERVAL).await;
                let mut latest = None;
                loop {
                    match progress_rx.try_recv() {
                        Ok(progress) => latest = Some(progress),
                        Err(TryRecvError::Empty) => break,
                        Err(TryRecvError::Disconnected) => return,
                    }
                }
                if let Some((downloaded, total)) = latest {
                    let _ = this.update(cx, |this, cx| {
                        if let Overlay::Update(UpdateDialog::Downloading {
                            downloaded: visible_downloaded,
                            total: visible_total,
                            ..
                        }) = &mut this.overlay
                        {
                            *visible_downloaded = downloaded;
                            *visible_total = total;
                            cx.notify();
                        }
                    });
                }
            }
        })
        .detach();

        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    let prepared = crate::update::prepare(|downloaded, total| {
                        let _ = progress_tx.send((downloaded, total));
                    })?;
                    let Some(prepared) = prepared else {
                        return Ok::<_, anyhow::Error>(None);
                    };
                    let version = prepared.version.clone();
                    prepared.install_and_arm_restart()?;
                    Ok(Some(version))
                })
                .await;

            let installed = this
                .update(cx, |this, cx| {
                    this.update_installing = false;
                    match result {
                        Ok(Some(version)) => {
                            this.set_overlay(
                                Overlay::Update(UpdateDialog::Installed { version }),
                                cx,
                            );
                            true
                        }
                        Ok(None) => {
                            this.set_overlay(
                                Overlay::Update(UpdateDialog::Current {
                                    version: crate::update::current_version().to_owned(),
                                }),
                                cx,
                            );
                            false
                        }
                        Err(error) => {
                            let release_url = this
                                .update_info
                                .as_ref()
                                .map(|info| info.release_url.clone())
                                .unwrap_or_else(crate::update::latest_release_url);
                            this.set_overlay(
                                Overlay::Update(UpdateDialog::Failed {
                                    message: error.to_string(),
                                    release_url,
                                }),
                                cx,
                            );
                            false
                        }
                    }
                })
                .unwrap_or(false);
            if installed {
                cx.background_executor()
                    .timer(Duration::from_millis(600))
                    .await;
                cx.update(|cx| cx.quit());
            }
        })
        .detach();
    }

    pub(crate) fn open_update_release_page(&mut self, url: String, cx: &mut Context<Self>) {
        let fallback = url.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move { crate::update::open_release_page(&url) })
                .await;
            if let Err(error) = result {
                this.update(cx, |this, cx| {
                    this.set_overlay(
                        Overlay::Update(UpdateDialog::Failed {
                            message: error.to_string(),
                            release_url: fallback,
                        }),
                        cx,
                    );
                })
                .ok();
            }
        })
        .detach();
    }
}
