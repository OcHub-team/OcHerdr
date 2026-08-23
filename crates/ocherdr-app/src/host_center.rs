use std::collections::{HashMap, HashSet, VecDeque};
use std::fs;
use std::path::PathBuf;

use ocherdr_core::ConnectionProfile;
use ocherdr_herdr::{
    HostHealthCheck, check_host, open_system_terminal, ssh_host_aliases, ssh_login_command,
};
use ochub_ui::gpui::{
    App, AppContext, Context, Entity, EventEmitter, FocusHandle, ListAlignment, ListState, Render,
    ScrollHandle, SharedString, Task, Window, prelude::*, px,
};
use ochub_ui::text_input::{TextInput, TextInputEvent};

use super::*;
use crate::notify::FailureKind;

/// Persistable host-center slice of `connections.json`.
///
/// Appearance and language stay on the parent view so that file still has a
/// single writer.
#[derive(Clone, Debug)]
pub(crate) struct HostPersistState {
    pub(crate) profiles: Vec<ConnectionProfile>,
    pub(crate) recent_connection_ids: Vec<String>,
    pub(crate) host_metadata: HashMap<String, HostMetadata>,
    pub(crate) host_groups: Vec<String>,
    pub(crate) host_health: HashMap<String, HostHealthView>,
}

pub(crate) fn assemble_settings(
    host: &HostPersistState,
    appearance: AppearanceSettings,
    language: Language,
) -> Settings {
    Settings {
        connections: host
            .profiles
            .iter()
            .filter(|profile| is_saved_profile(profile))
            .cloned()
            .collect(),
        recent_connection_ids: host.recent_connection_ids.clone(),
        host_metadata: host.host_metadata.clone(),
        host_groups: host.host_groups.clone(),
        host_health: host
            .host_health
            .iter()
            .filter_map(|(id, health)| match health {
                HostHealthView::Checking { .. } => None,
                HostHealthView::Checked { cached, .. } => Some((id.clone(), cached.clone())),
            })
            .collect(),
        appearance,
        language,
    }
}

fn write_settings(settings: &Settings) -> std::result::Result<(), String> {
    let path =
        settings_path().ok_or_else(|| "Application Support directory is unavailable".to_owned())?;
    let parent = path
        .parent()
        .ok_or_else(|| "Settings path has no parent directory".to_owned())?;
    fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    let bytes = serde_json::to_vec_pretty(settings).map_err(|error| error.to_string())?;
    fs::write(path, bytes).map_err(|error| error.to_string())
}

pub(crate) fn persist_assembled_settings(
    host: &HostPersistState,
    appearance: &AppearanceSettings,
    language: Language,
) -> std::result::Result<(), String> {
    write_settings(&assemble_settings(host, appearance.clone(), language))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum HostSaveThen {
    ShowHostCenter,
    Connect,
}

#[derive(Clone, Debug)]
pub(crate) enum HostCenterEvent {
    PersistBestEffort(HostPersistState),
    PersistRevertible {
        state: HostPersistState,
        error: FailureKind,
    },
    HostSaved {
        state: HostPersistState,
        index: usize,
        then: HostSaveThen,
    },
    CatalogChanged(Vec<ConnectionProfile>),
    ProfileSelected(usize),
    OpenCreateForm,
    OpenEditForm(usize),
    DismissForm,
    ConfirmRemoveProfile(usize),
    ConfirmBulkRemove,
    Failed {
        kind: FailureKind,
        detail: SharedString,
    },
    CloseRequested,
}

struct HostRollback {
    profiles: Vec<ConnectionProfile>,
    recent_connection_ids: Vec<String>,
    host_metadata: HashMap<String, HostMetadata>,
    host_groups: Vec<String>,
    host_health: HashMap<String, HostHealthView>,
    orphaned_ssh_hosts: HashSet<String>,
    profile_index: usize,
    managed_profile_index: usize,
    host_bulk_selection: HashSet<String>,
}

pub(crate) struct HostCenter {
    pub(crate) profiles: Vec<ConnectionProfile>,
    pub(crate) profile_index: usize,
    /// Mirror of `Overlay::RemoteForm`. Written only by [`Self::set_form`]
    /// from the parent view.
    form: Option<RemoteForm>,
    pub(crate) managed_profile_index: usize,
    pub(crate) remote_advanced_open: bool,
    pub(crate) recent_connection_ids: Vec<String>,
    pub(crate) host_metadata: HashMap<String, HostMetadata>,
    pub(crate) host_groups: Vec<String>,
    pub(crate) host_health: HashMap<String, HostHealthView>,
    pub(crate) host_filter: HostFilter,
    /// Dropping an entry cancels that host's in-flight probe.
    host_check_inflight: HashMap<String, Task<()>>,
    host_check_queue: VecDeque<(String, ConnectionProfile)>,
    pub(crate) host_bulk_mode: bool,
    pub(crate) host_bulk_selection: HashSet<String>,
    pub(crate) orphaned_ssh_hosts: HashSet<String>,
    pub(crate) host_nav_scroll: ScrollHandle,
    pub(crate) host_inspector_scroll: ScrollHandle,
    pub(crate) host_form_scroll: ScrollHandle,
    pub(crate) host_list_state: ListState,
    pub(crate) host_list_revision: HostListRevision,
    pub(crate) remote_label: Entity<TextInput>,
    pub(crate) remote_destination: Entity<TextInput>,
    pub(crate) remote_port: Entity<TextInput>,
    pub(crate) remote_identity_file: Entity<TextInput>,
    pub(crate) remote_herdr_path: Entity<TextInput>,
    pub(crate) remote_group: Entity<TextInput>,
    pub(crate) remote_tags: Entity<TextInput>,
    pub(crate) remote_search: Entity<TextInput>,
    pub(crate) i18n: I18n,
    focus: FocusHandle,
    rollback: Option<HostRollback>,
}

impl EventEmitter<HostCenterEvent> for HostCenter {}

impl HostCenter {
    pub(crate) fn new(
        settings: Settings,
        i18n: I18n,
        focus: FocusHandle,
        cx: &mut Context<Self>,
    ) -> Self {
        let host_metadata = settings.host_metadata;
        let mut profiles = vec![ConnectionProfile::default()];
        profiles.extend(settings.connections);
        let saved_destinations = profiles
            .iter()
            .filter_map(|profile| match profile {
                ConnectionProfile::Ssh { destination, .. } => Some(destination.clone()),
                ConnectionProfile::Local { .. } => None,
            })
            .collect::<Vec<_>>();
        profiles.extend(
            ssh_host_aliases()
                .into_iter()
                .filter(|host| !saved_destinations.contains(host))
                .map(|host| {
                    let id = format!("ssh-config:{host}");
                    let metadata = host_metadata.get(&id).cloned().unwrap_or_default();
                    ConnectionProfile::Ssh {
                        id,
                        label: metadata.display_name.unwrap_or_else(|| host.clone()),
                        destination: host,
                        port: metadata.port_override,
                        identity_file: metadata.identity_file_override,
                        herdr_path: metadata
                            .herdr_path_override
                            .unwrap_or_else(|| "herdr".into()),
                    }
                }),
        );
        let mut orphaned_ssh_hosts = HashSet::new();
        for (id, metadata) in &host_metadata {
            let Some(alias) = id.strip_prefix("ssh-config:") else {
                continue;
            };
            if profiles.iter().any(|profile| profile.id() == id)
                || saved_destinations
                    .iter()
                    .any(|destination| destination == alias)
            {
                continue;
            }
            profiles.push(ConnectionProfile::Ssh {
                id: id.clone(),
                label: metadata
                    .display_name
                    .clone()
                    .unwrap_or_else(|| alias.to_owned()),
                destination: alias.to_owned(),
                port: metadata.port_override,
                identity_file: metadata.identity_file_override.clone(),
                herdr_path: metadata
                    .herdr_path_override
                    .clone()
                    .unwrap_or_else(|| "herdr".into()),
            });
            orphaned_ssh_hosts.insert(id.clone());
        }
        let remote_search = cx.new(|cx| {
            TextInput::new(cx, i18n.text(k::HOSTS_SEARCH_PLACEHOLDER))
                .search_field()
                .compact()
        });
        cx.subscribe(&remote_search, |this, _input, _: &TextInputEvent, cx| {
            this.ensure_managed_profile_visible(cx);
        })
        .detach();
        let known_ids = profiles
            .iter()
            .map(|profile| profile.id().to_owned())
            .collect::<HashSet<_>>();
        let recent_connection_ids = settings
            .recent_connection_ids
            .into_iter()
            .filter_map(|id| normalize_recent_host_id(&id, &profiles))
            .filter(|id| known_ids.contains(id))
            .collect::<Vec<_>>();
        let host_health = settings
            .host_health
            .into_iter()
            .filter(|(id, _)| known_ids.contains(id))
            .map(|(id, cached)| {
                (
                    id,
                    HostHealthView::Checked {
                        cached,
                        detail: String::new(),
                    },
                )
            })
            .collect();
        let center = Self {
            profiles,
            profile_index: 0,
            form: None,
            managed_profile_index: 0,
            remote_advanced_open: false,
            recent_connection_ids,
            host_metadata,
            host_groups: settings.host_groups,
            host_health,
            host_filter: HostFilter::All,
            host_check_inflight: HashMap::new(),
            host_check_queue: VecDeque::new(),
            host_bulk_mode: false,
            host_bulk_selection: HashSet::new(),
            orphaned_ssh_hosts,
            host_nav_scroll: ScrollHandle::new(),
            host_inspector_scroll: ScrollHandle::new(),
            host_form_scroll: ScrollHandle::new(),
            host_list_state: ListState::new(0, ListAlignment::Top, px(192.)),
            host_list_revision: HostListRevision::default(),
            remote_label: cx
                .new(|cx| TextInput::new(cx, i18n.text(k::HOSTS_FORM_PLACEHOLDER_LABEL))),
            remote_destination: cx
                .new(|cx| TextInput::new(cx, i18n.text(k::HOSTS_FORM_PLACEHOLDER_DESTINATION))),
            remote_port: cx.new(|cx| TextInput::new(cx, i18n.text(k::HOSTS_FORM_PLACEHOLDER_PORT))),
            remote_identity_file: cx
                .new(|cx| TextInput::new(cx, i18n.text(k::HOSTS_FORM_PLACEHOLDER_IDENTITY))),
            remote_herdr_path: cx.new(|cx| TextInput::new(cx, "herdr").with_content("herdr")),
            remote_group: cx
                .new(|cx| TextInput::new(cx, i18n.text(k::HOSTS_FORM_PLACEHOLDER_GROUP))),
            remote_tags: cx.new(|cx| TextInput::new(cx, i18n.text(k::HOSTS_FORM_PLACEHOLDER_TAGS))),
            remote_search,
            i18n,
            focus,
            rollback: None,
        };
        let host = cx.weak_entity();
        for field in [
            &center.remote_label,
            &center.remote_destination,
            &center.remote_port,
            &center.remote_identity_file,
            &center.remote_herdr_path,
            &center.remote_group,
            &center.remote_tags,
        ] {
            bind_enter_submit(field, host.clone(), cx, |this, _window, cx| {
                this.save_remote(false, cx);
            });
        }
        center
    }

    pub(crate) fn persist_state(&self) -> HostPersistState {
        HostPersistState {
            profiles: self.profiles.clone(),
            recent_connection_ids: self.recent_connection_ids.clone(),
            host_metadata: self.host_metadata.clone(),
            host_groups: self.host_groups.clone(),
            host_health: self.host_health.clone(),
        }
    }

    pub(crate) fn profiles(&self) -> &[ConnectionProfile] {
        &self.profiles
    }

    pub(crate) fn set_profile_index(&mut self, index: usize) {
        if self.profile_index == index {
            return;
        }
        self.profile_index = index;
        self.cancel_host_checks();
    }

    pub(crate) fn form(&self) -> Option<RemoteForm> {
        self.form
    }

    pub(crate) fn set_form(&mut self, form: Option<RemoteForm>) {
        self.form = form;
    }

    pub(crate) fn bulk_selection_len(&self) -> usize {
        self.host_bulk_selection.len()
    }

    pub(crate) fn display_label(&self, index: usize) -> String {
        let Some(profile) = self.profiles.get(index) else {
            return String::new();
        };
        host_display_label_for(profile, self.host_metadata.get(profile.id()), self.i18n)
    }

    pub(crate) fn switcher_entries(&self) -> Vec<usize> {
        let mut entries = Vec::new();
        let mut seen = HashSet::new();
        let push = |index: usize, entries: &mut Vec<usize>, seen: &mut HashSet<usize>| {
            if seen.insert(index) {
                entries.push(index);
            }
        };
        push(0, &mut entries, &mut seen);
        for (index, profile) in self.profiles.iter().enumerate() {
            if self
                .host_metadata
                .get(profile.id())
                .is_some_and(|metadata| metadata.favorite)
            {
                push(index, &mut entries, &mut seen);
            }
        }
        for id in &self.recent_connection_ids {
            if let Some(index) = self.profiles.iter().position(|profile| profile.id() == id) {
                push(index, &mut entries, &mut seen);
            }
        }
        entries.truncate(8);
        entries
    }

    pub(crate) fn open(&mut self, profile_index: usize, cx: &mut Context<Self>) {
        self.profile_index = profile_index;
        self.managed_profile_index = profile_index;
        self.host_filter = HostFilter::All;
        self.host_bulk_mode = false;
        self.host_bulk_selection.clear();
        self.refresh_common_host_health(cx);
        cx.notify();
    }

    pub(crate) fn dismiss(&mut self) {
        self.host_bulk_mode = false;
        self.host_bulk_selection.clear();
        self.cancel_host_checks();
    }

    pub(crate) fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.dismiss();
        self.focus.focus(window, cx);
        cx.emit(HostCenterEvent::CloseRequested);
        cx.notify();
    }

    pub(crate) fn remember_host(&mut self, id: &str, cx: &mut Context<Self>) {
        remember_recent(&mut self.recent_connection_ids, id);
        self.persist_best_effort(cx);
    }

    pub(crate) fn apply_language(&mut self, i18n: I18n, cx: &mut Context<Self>) {
        self.i18n = i18n;
        self.remote_search.update(cx, |input, cx| {
            input.set_placeholder(i18n.text(k::HOSTS_SEARCH_PLACEHOLDER), cx)
        });
        self.remote_label.update(cx, |input, cx| {
            input.set_placeholder(i18n.text(k::HOSTS_FORM_PLACEHOLDER_LABEL), cx)
        });
        self.remote_destination.update(cx, |input, cx| {
            input.set_placeholder(i18n.text(k::HOSTS_FORM_PLACEHOLDER_DESTINATION), cx)
        });
        self.remote_port.update(cx, |input, cx| {
            input.set_placeholder(i18n.text(k::HOSTS_FORM_PLACEHOLDER_PORT), cx)
        });
        self.remote_identity_file.update(cx, |input, cx| {
            input.set_placeholder(i18n.text(k::HOSTS_FORM_PLACEHOLDER_IDENTITY), cx)
        });
        self.remote_group.update(cx, |input, cx| {
            input.set_placeholder(i18n.text(k::HOSTS_FORM_PLACEHOLDER_GROUP), cx)
        });
        self.remote_tags.update(cx, |input, cx| {
            input.set_placeholder(i18n.text(k::HOSTS_FORM_PLACEHOLDER_TAGS), cx)
        });
    }

    pub(crate) fn commit_persist(&mut self) {
        self.rollback = None;
    }

    pub(crate) fn rollback_persist(&mut self) {
        if let Some(snapshot) = self.rollback.take() {
            self.apply_rollback(snapshot);
        }
    }

    fn snapshot(&self) -> HostRollback {
        HostRollback {
            profiles: self.profiles.clone(),
            recent_connection_ids: self.recent_connection_ids.clone(),
            host_metadata: self.host_metadata.clone(),
            host_groups: self.host_groups.clone(),
            host_health: self.host_health.clone(),
            orphaned_ssh_hosts: self.orphaned_ssh_hosts.clone(),
            profile_index: self.profile_index,
            managed_profile_index: self.managed_profile_index,
            host_bulk_selection: self.host_bulk_selection.clone(),
        }
    }

    fn apply_rollback(&mut self, snapshot: HostRollback) {
        self.profiles = snapshot.profiles;
        self.recent_connection_ids = snapshot.recent_connection_ids;
        self.host_metadata = snapshot.host_metadata;
        self.host_groups = snapshot.host_groups;
        self.host_health = snapshot.host_health;
        self.orphaned_ssh_hosts = snapshot.orphaned_ssh_hosts;
        self.profile_index = snapshot.profile_index;
        self.managed_profile_index = snapshot.managed_profile_index;
        self.host_bulk_selection = snapshot.host_bulk_selection;
    }

    fn begin_mutation(&mut self) {
        self.rollback = Some(self.snapshot());
    }

    fn persist_best_effort(&mut self, cx: &mut Context<Self>) {
        cx.emit(HostCenterEvent::PersistBestEffort(self.persist_state()));
    }

    fn persist_revertible(&mut self, error: FailureKind, cx: &mut Context<Self>) {
        cx.emit(HostCenterEvent::PersistRevertible {
            state: self.persist_state(),
            error,
        });
    }

    fn fail(&self, kind: FailureKind, detail: impl std::fmt::Display, cx: &mut Context<Self>) {
        cx.emit(HostCenterEvent::Failed {
            kind,
            detail: detail.to_string().into(),
        });
    }

    fn mark_checking(&mut self, id: String) {
        let previous = match self.host_health.remove(&id) {
            Some(HostHealthView::Checking { previous }) => previous,
            Some(checked @ HostHealthView::Checked { .. }) => Some(Box::new(checked)),
            None => None,
        };
        self.host_health
            .insert(id, HostHealthView::Checking { previous });
    }

    fn cancel_host_check(&mut self, id: &str, cx: &mut Context<Self>) {
        self.host_check_inflight.remove(id);
        self.host_check_queue
            .retain(|(queued_id, _)| queued_id != id);
        restore_cancelled_check(&mut self.host_health, id);
        self.pump_host_checks(cx);
    }

    fn cancel_host_checks(&mut self) {
        self.host_check_inflight.clear();
        self.host_check_queue.clear();
        self.host_health = restore_cancelled_checks(std::mem::take(&mut self.host_health));
    }

    pub(crate) fn invalidate_probe_for_saved_host(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(id) = self
            .profiles
            .get(index)
            .map(|profile| profile.id().to_owned())
        else {
            return;
        };
        discard_probe_for_host(
            &mut self.host_check_inflight,
            &mut self.host_check_queue,
            &mut self.host_health,
            &id,
        );
        self.pump_host_checks(cx);
    }

    pub(crate) fn open_add_remote(&mut self, cx: &mut Context<Self>) {
        self.remote_advanced_open = false;
        self.clear_remote_form(cx);
        cx.emit(HostCenterEvent::OpenCreateForm);
        cx.notify();
    }

    pub(crate) fn close_add_remote(&mut self, cx: &mut Context<Self>) {
        cx.emit(HostCenterEvent::DismissForm);
        cx.notify();
    }

    pub(crate) fn select_managed_profile(&mut self, index: usize, cx: &mut Context<Self>) {
        if index >= self.profiles.len() {
            return;
        }
        if self.host_bulk_mode && index != 0 {
            let id = self.profiles[index].id().to_owned();
            if !self.host_bulk_selection.insert(id.clone()) {
                self.host_bulk_selection.remove(&id);
            }
            cx.notify();
            return;
        }
        self.managed_profile_index = index;
        if self.form.is_some() {
            cx.emit(HostCenterEvent::DismissForm);
        }
        cx.notify();
    }

    pub(crate) fn open_edit_remote(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(profile) = self.profiles.get(index).cloned() else {
            return;
        };
        if matches!(profile, ConnectionProfile::Local { .. }) {
            self.fail(
                FailureKind::CannotEditThisMac,
                self.i18n.text(k::NOTIFY_DETAIL_CANNOT_EDIT_THIS_MAC),
                cx,
            );
            cx.notify();
            return;
        }
        self.managed_profile_index = index;
        self.fill_remote_form(&profile, cx);
        cx.emit(HostCenterEvent::OpenEditForm(index));
        cx.notify();
    }

    pub(crate) fn set_host_filter(&mut self, filter: HostFilter, cx: &mut Context<Self>) {
        self.host_filter = filter;
        if self.form.is_some() {
            cx.emit(HostCenterEvent::DismissForm);
        }
        if let Some(index) = self.filtered_profile_indexes(cx).first().copied() {
            self.managed_profile_index = index;
        }
        cx.notify();
    }

    pub(crate) fn toggle_host_favorite(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(profile) = self.profiles.get(index) else {
            return;
        };
        let id = profile.id().to_owned();
        self.begin_mutation();
        let metadata = self.host_metadata.entry(id).or_default();
        metadata.favorite = !metadata.favorite;
        self.persist_revertible(FailureKind::UpdateFavorites, cx);
        cx.notify();
    }

    pub(crate) fn toggle_host_bulk_mode(&mut self, cx: &mut Context<Self>) {
        self.host_bulk_mode = !self.host_bulk_mode;
        self.host_bulk_selection.clear();
        if self.form.is_some() {
            cx.emit(HostCenterEvent::DismissForm);
        }
        if self.host_bulk_mode {
            self.remote_group
                .update(cx, |input, cx| input.set_content("", cx));
            self.remote_tags
                .update(cx, |input, cx| input.set_content("", cx));
        }
        cx.notify();
    }

    pub(crate) fn bulk_set_favorite(&mut self, favorite: bool, cx: &mut Context<Self>) {
        if self.host_bulk_selection.is_empty() {
            return;
        }
        self.begin_mutation();
        for id in self.host_bulk_selection.clone() {
            self.host_metadata.entry(id).or_default().favorite = favorite;
        }
        self.persist_revertible(FailureKind::UpdateFavorites, cx);
        cx.notify();
    }

    pub(crate) fn bulk_apply_organization(&mut self, cx: &mut Context<Self>) {
        if self.host_bulk_selection.is_empty() {
            return;
        }
        let group = self.remote_group.read(cx).content().trim().to_owned();
        let group = (!group.is_empty()).then_some(group);
        let tags = parse_host_tags(&self.remote_tags.read(cx).content());
        if group.is_none() && tags.is_empty() {
            self.fail(
                FailureKind::NeedGroupOrTag,
                self.i18n.text(k::NOTIFY_DETAIL_NEED_GROUP_OR_TAG),
                cx,
            );
            cx.notify();
            return;
        }
        self.begin_mutation();
        for id in self.host_bulk_selection.clone() {
            let metadata = self.host_metadata.entry(id).or_default();
            if let Some(group) = &group {
                metadata.group = Some(group.clone());
            }
            for tag in &tags {
                if !metadata
                    .tags
                    .iter()
                    .any(|existing| existing.eq_ignore_ascii_case(tag))
                {
                    metadata.tags.push(tag.clone());
                }
            }
        }
        if let Some(group) = group
            && !self
                .host_groups
                .iter()
                .any(|existing| existing.eq_ignore_ascii_case(&group))
        {
            self.host_groups.push(group);
            self.host_groups.sort_by_key(|group| group.to_lowercase());
        }
        self.persist_revertible(FailureKind::ApplyOrganization, cx);
        cx.notify();
    }

    pub(crate) fn request_bulk_remove(&mut self, cx: &mut Context<Self>) {
        if self.host_bulk_selection.is_empty() {
            return;
        }
        let active_id = self.profiles[self.profile_index].id().to_owned();
        if self.host_bulk_selection.contains(&active_id) {
            self.fail(
                FailureKind::CannotRemoveActiveHost,
                self.i18n.text(k::NOTIFY_DETAIL_CANNOT_REMOVE_ACTIVE),
                cx,
            );
            cx.notify();
            return;
        }
        cx.emit(HostCenterEvent::ConfirmBulkRemove);
        cx.notify();
    }

    pub(crate) fn confirm_bulk_remove(&mut self, cx: &mut Context<Self>) {
        let selected = self.host_bulk_selection.clone();
        let current_id = self.profiles[self.profile_index].id().to_owned();
        let managed_id = self
            .profiles
            .get(self.managed_profile_index)
            .map(|profile| profile.id().to_owned())
            .unwrap_or_else(|| current_id.clone());
        for id in &selected {
            self.cancel_host_check(id, cx);
        }
        self.begin_mutation();
        let removed_ids = self
            .profiles
            .iter()
            .filter(|profile| {
                selected.contains(profile.id())
                    && (is_saved_profile(profile) || self.orphaned_ssh_hosts.contains(profile.id()))
            })
            .map(|profile| profile.id().to_owned())
            .collect::<HashSet<_>>();
        self.profiles
            .retain(|profile| !removed_ids.contains(profile.id()));
        for profile in &mut self.profiles {
            if !selected.contains(profile.id())
                || connection_source(profile) != ConnectionSource::SshConfig
            {
                continue;
            }
            let ConnectionProfile::Ssh {
                id,
                label,
                destination,
                port,
                identity_file,
                herdr_path,
            } = profile
            else {
                continue;
            };
            let alias = id
                .strip_prefix("ssh-config:")
                .unwrap_or(destination)
                .to_owned();
            *label = alias.clone();
            *destination = alias;
            *port = None;
            *identity_file = None;
            *herdr_path = "herdr".into();
        }
        for id in &selected {
            self.host_metadata.remove(id);
            self.host_health.remove(id);
        }
        self.orphaned_ssh_hosts.retain(|id| !selected.contains(id));
        self.recent_connection_ids
            .retain(|id| !removed_ids.contains(id));
        self.profile_index = self
            .profiles
            .iter()
            .position(|profile| profile.id() == current_id)
            .unwrap_or(0);
        self.managed_profile_index = self
            .profiles
            .iter()
            .position(|profile| profile.id() == managed_id)
            .unwrap_or(self.profile_index);
        self.host_bulk_selection.clear();
        self.persist_revertible(FailureKind::RemoveHosts, cx);
        cx.notify();
    }

    pub(crate) fn filtered_profile_indexes(&self, cx: &App) -> Vec<usize> {
        visible_host_indices(
            &HostCatalog {
                profiles: &self.profiles,
                metadata: &self.host_metadata,
                recent_ids: &self.recent_connection_ids,
                orphaned: &self.orphaned_ssh_hosts,
                health: &self.host_health,
            },
            &self.host_filter,
            self.remote_search.read(cx).content().as_ref(),
            self.profile_index,
            self.i18n,
        )
    }

    pub(crate) fn test_managed_host(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(profile) = self.profiles.get(index).cloned() else {
            return;
        };
        let id = profile.id().to_owned();
        self.cancel_host_check(&id, cx);
        self.mark_checking(id.clone());
        self.host_check_queue.push_back((id, profile));
        self.pump_host_checks(cx);
        cx.notify();
    }

    pub(crate) fn open_managed_host_in_terminal(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(command) = self.profiles.get(index).and_then(ssh_login_command) else {
            return;
        };
        if let Err(error) = open_system_terminal(&command) {
            self.fail(FailureKind::OpenTerminal, error, cx);
        }
        cx.notify();
    }

    pub(crate) fn refresh_common_host_health(&mut self, cx: &mut Context<Self>) {
        self.reload_ssh_config_hosts(cx);
        self.cancel_host_checks();
        let mut ids = Vec::new();
        if let Some(profile) = self.profiles.get(self.profile_index) {
            ids.push(profile.id().to_owned());
        }
        ids.extend(
            self.profiles
                .iter()
                .filter(|profile| {
                    self.host_metadata
                        .get(profile.id())
                        .is_some_and(|metadata| metadata.favorite)
                })
                .map(|profile| profile.id().to_owned()),
        );
        ids.extend(self.recent_connection_ids.iter().take(8).cloned());
        let mut seen = HashSet::new();
        for id in ids {
            if !seen.insert(id.clone()) {
                continue;
            }
            let Some(profile) = self
                .profiles
                .iter()
                .find(|profile| profile.id() == id)
                .cloned()
            else {
                continue;
            };
            self.mark_checking(id.clone());
            self.host_check_queue.push_back((id, profile));
        }
        self.pump_host_checks(cx);
        cx.notify();
    }

    fn reload_ssh_config_hosts(&mut self, cx: &mut Context<Self>) {
        let current_id = self
            .profiles
            .get(self.profile_index)
            .map(|profile| profile.id().to_owned())
            .unwrap_or_else(|| "local".into());
        let managed_id = self
            .profiles
            .get(self.managed_profile_index)
            .map(|profile| profile.id().to_owned())
            .unwrap_or_else(|| current_id.clone());
        let old_config = self
            .profiles
            .iter()
            .filter(|profile| connection_source(profile) == ConnectionSource::SshConfig)
            .map(|profile| (profile.id().to_owned(), profile.clone()))
            .collect::<HashMap<_, _>>();
        let mut profiles = self
            .profiles
            .iter()
            .filter(|profile| connection_source(profile) != ConnectionSource::SshConfig)
            .cloned()
            .collect::<Vec<_>>();
        let saved_destinations = profiles
            .iter()
            .filter_map(ssh_destination)
            .map(str::to_owned)
            .collect::<HashSet<_>>();
        let aliases = ssh_host_aliases();
        let mut discovered_ids = HashSet::new();
        for alias in aliases {
            if saved_destinations.contains(&alias) {
                continue;
            }
            let id = format!("ssh-config:{alias}");
            discovered_ids.insert(id.clone());
            let metadata = self.host_metadata.get(&id).cloned().unwrap_or_default();
            profiles.push(ConnectionProfile::Ssh {
                id,
                label: metadata.display_name.unwrap_or_else(|| alias.clone()),
                destination: alias,
                port: metadata.port_override,
                identity_file: metadata.identity_file_override,
                herdr_path: metadata
                    .herdr_path_override
                    .unwrap_or_else(|| "herdr".into()),
            });
        }
        self.orphaned_ssh_hosts.clear();
        for (id, profile) in old_config {
            if discovered_ids.contains(&id) {
                continue;
            }
            if self.host_metadata.contains_key(&id) || id == current_id {
                self.orphaned_ssh_hosts.insert(id);
                profiles.push(profile);
            }
        }
        self.profiles = profiles;
        self.profile_index = self
            .profiles
            .iter()
            .position(|profile| profile.id() == current_id)
            .unwrap_or(0);
        self.managed_profile_index = self
            .profiles
            .iter()
            .position(|profile| profile.id() == managed_id)
            .unwrap_or(self.profile_index);
        cx.emit(HostCenterEvent::CatalogChanged(self.profiles.clone()));
    }

    fn pump_host_checks(&mut self, cx: &mut Context<Self>) {
        while self.host_check_inflight.len() < 3 {
            let Some((id, profile)) = self.host_check_queue.pop_front() else {
                break;
            };
            if self.host_check_inflight.contains_key(&id) {
                continue;
            }
            let probe_id = id.clone();
            let task = cx.spawn(async move |this, cx| {
                let result = cx
                    .background_spawn(async move { check_host(&profile) })
                    .await;
                this.update(cx, |this, cx| {
                    if this.host_check_inflight.remove(&probe_id).is_none() {
                        return;
                    }
                    this.store_host_health(probe_id, result);
                    this.persist_best_effort(cx);
                    this.pump_host_checks(cx);
                    cx.notify();
                })
                .ok();
            });
            self.host_check_inflight.insert(id, task);
        }
    }

    fn store_host_health(&mut self, id: String, result: HostHealthCheck) {
        let cached = CachedHostHealth {
            status: result.status,
            checked_at: unix_timestamp(),
            herdr_version: result.herdr_version,
            session_count: result.session_count,
            latency_ms: result.latency_ms,
        };
        self.host_health.insert(
            id,
            HostHealthView::Checked {
                cached,
                detail: result.detail,
            },
        );
    }

    pub(crate) fn ensure_managed_profile_visible(&mut self, cx: &mut Context<Self>) {
        let indexes = self.filtered_profile_indexes(cx);
        if indexes.contains(&self.managed_profile_index) {
            cx.notify();
            return;
        }
        if let Some(index) = indexes.first().copied() {
            self.managed_profile_index = index;
        }
        cx.notify();
    }

    pub(crate) fn toggle_remote_advanced(&mut self, cx: &mut Context<Self>) {
        self.remote_advanced_open = !self.remote_advanced_open;
        cx.notify();
    }

    pub(crate) fn request_remove_node(&mut self, index: usize, cx: &mut Context<Self>) {
        if index == self.profile_index {
            self.fail(
                FailureKind::CannotRemoveActiveHost,
                self.i18n.text(k::NOTIFY_DETAIL_CANNOT_REMOVE_ACTIVE),
                cx,
            );
            cx.notify();
            return;
        }
        if self.profiles.get(index).is_some_and(is_saved_profile) {
            cx.emit(HostCenterEvent::ConfirmRemoveProfile(index));
            cx.notify();
        }
    }

    pub(crate) fn confirm_remove_node(&mut self, index: usize, cx: &mut Context<Self>) {
        if index == 0 || index >= self.profiles.len() {
            return;
        }
        let removed_id = self.profiles[index].id().to_owned();
        self.cancel_host_check(&removed_id, cx);
        self.begin_mutation();
        let removed = self.profiles.remove(index);
        self.recent_connection_ids.retain(|id| id != removed.id());
        self.host_metadata.remove(&removed_id);
        self.host_health.remove(&removed_id);
        if index == self.profile_index {
            self.profile_index = 0;
        } else if index < self.profile_index {
            self.profile_index -= 1;
        }
        self.managed_profile_index = self.managed_profile_index.min(self.profiles.len() - 1);
        self.persist_revertible(FailureKind::RemoveHost, cx);
        cx.notify();
    }

    pub(crate) fn save_remote(&mut self, connect: bool, cx: &mut Context<Self>) {
        let Some(draft) = self.parse_remote_draft(cx) else {
            return;
        };
        let group = self.remote_group.read(cx).content().trim().to_owned();
        let group = (!group.is_empty()).then_some(group);
        let tags = parse_host_tags(&self.remote_tags.read(cx).content());
        let Some(form) = self.form else {
            return;
        };
        self.begin_mutation();
        let index = match form {
            RemoteForm::Create => {
                self.profiles.push(draft);
                let index = self.profiles.len() - 1;
                let id = self.profiles[index].id().to_owned();
                self.host_metadata.insert(
                    id,
                    HostMetadata {
                        group: group.clone(),
                        tags: tags.clone(),
                        ..HostMetadata::default()
                    },
                );
                index
            }
            RemoteForm::Edit(index) if index < self.profiles.len() => {
                let source = connection_source(&self.profiles[index]);
                let ConnectionProfile::Ssh {
                    label: new_label,
                    destination: new_destination,
                    port: new_port,
                    identity_file: new_identity,
                    herdr_path: new_herdr,
                    ..
                } = draft
                else {
                    self.rollback_persist();
                    return;
                };
                let id = self.profiles[index].id().to_owned();
                let metadata = self.host_metadata.entry(id).or_default();
                metadata.group = group.clone();
                metadata.tags = tags.clone();
                if source == ConnectionSource::SshConfig {
                    metadata.display_name = Some(new_label.clone()).filter(|label| {
                        label != ssh_destination(&self.profiles[index]).unwrap_or_default()
                    });
                    metadata.port_override = new_port;
                    metadata.identity_file_override = new_identity.clone();
                    metadata.herdr_path_override =
                        (new_herdr != "herdr").then_some(new_herdr.clone());
                }
                match &mut self.profiles[index] {
                    ConnectionProfile::Ssh {
                        label,
                        destination,
                        port,
                        identity_file,
                        herdr_path,
                        ..
                    } => {
                        *label = new_label;
                        if source == ConnectionSource::Saved {
                            *destination = new_destination;
                        }
                        *port = new_port;
                        *identity_file = new_identity;
                        *herdr_path = new_herdr;
                    }
                    ConnectionProfile::Local { .. } => unreachable!(),
                }
                index
            }
            RemoteForm::Edit(_) => {
                self.rollback_persist();
                return;
            }
        };
        if let Some(group) = &group
            && !self
                .host_groups
                .iter()
                .any(|existing| existing.eq_ignore_ascii_case(group))
        {
            self.host_groups.push(group.clone());
            self.host_groups.sort_by_key(|group| group.to_lowercase());
        }
        self.managed_profile_index = index;
        cx.emit(HostCenterEvent::HostSaved {
            state: self.persist_state(),
            index,
            then: if connect {
                HostSaveThen::Connect
            } else {
                HostSaveThen::ShowHostCenter
            },
        });
        cx.notify();
    }

    fn parse_remote_draft(&mut self, cx: &mut Context<Self>) -> Option<ConnectionProfile> {
        let destination = self.remote_destination.read(cx).content().trim().to_owned();
        if destination.is_empty() {
            self.fail(
                FailureKind::SshDestinationRequired,
                self.i18n.text(k::NOTIFY_DETAIL_SSH_DESTINATION),
                cx,
            );
            cx.notify();
            return None;
        }
        let label = self.remote_label.read(cx).content().trim().to_owned();
        let port_text = self.remote_port.read(cx).content().trim().to_owned();
        let identity_file = self
            .remote_identity_file
            .read(cx)
            .content()
            .trim()
            .to_owned();
        let herdr_path = self.remote_herdr_path.read(cx).content().trim().to_owned();
        let port = if port_text.is_empty() {
            None
        } else {
            match port_text.parse::<u16>() {
                Ok(port) if port > 0 => Some(port),
                _ => {
                    self.fail(
                        FailureKind::SshPortInvalid,
                        self.i18n.text(k::NOTIFY_DETAIL_SSH_PORT),
                        cx,
                    );
                    cx.notify();
                    return None;
                }
            }
        };
        let id = match self.form {
            Some(RemoteForm::Edit(index)) => self
                .profiles
                .get(index)
                .map(|profile| profile.id().to_owned())
                .unwrap_or_else(|| format!("manual-{}", next_manual_profile_id(&self.profiles))),
            _ => format!("manual-{}", next_manual_profile_id(&self.profiles)),
        };
        Some(ConnectionProfile::Ssh {
            id,
            label: if label.is_empty() {
                destination.clone()
            } else {
                label
            },
            destination,
            port,
            identity_file: (!identity_file.is_empty()).then(|| PathBuf::from(identity_file)),
            herdr_path: if herdr_path.is_empty() {
                "herdr".into()
            } else {
                herdr_path
            },
        })
    }

    fn clear_remote_form(&mut self, cx: &mut Context<Self>) {
        self.remote_label
            .update(cx, |input, cx| input.set_content("", cx));
        self.remote_destination
            .update(cx, |input, cx| input.set_content("", cx));
        self.remote_port
            .update(cx, |input, cx| input.set_content("", cx));
        self.remote_identity_file
            .update(cx, |input, cx| input.set_content("", cx));
        self.remote_herdr_path
            .update(cx, |input, cx| input.set_content("herdr", cx));
        self.remote_group
            .update(cx, |input, cx| input.set_content("", cx));
        self.remote_tags
            .update(cx, |input, cx| input.set_content("", cx));
    }

    fn fill_remote_form(&mut self, profile: &ConnectionProfile, cx: &mut Context<Self>) {
        let metadata = self
            .host_metadata
            .get(profile.id())
            .cloned()
            .unwrap_or_default();
        self.remote_group.update(cx, |input, cx| {
            input.set_content(metadata.group.clone().unwrap_or_default(), cx)
        });
        self.remote_tags.update(cx, |input, cx| {
            input.set_content(metadata.tags.join(", "), cx)
        });
        match profile {
            ConnectionProfile::Local { herdr_path } => {
                let label = self.i18n.text(k::HOSTS_SOURCE_THIS_MAC).to_owned();
                self.remote_label
                    .update(cx, |input, cx| input.set_content(label, cx));
                self.remote_destination
                    .update(cx, |input, cx| input.set_content("", cx));
                self.remote_port
                    .update(cx, |input, cx| input.set_content("", cx));
                self.remote_identity_file
                    .update(cx, |input, cx| input.set_content("", cx));
                self.remote_herdr_path
                    .update(cx, |input, cx| input.set_content(herdr_path.clone(), cx));
                self.remote_advanced_open = herdr_path != "herdr";
            }
            ConnectionProfile::Ssh {
                label,
                destination,
                port,
                identity_file,
                herdr_path,
                ..
            } => {
                self.remote_label
                    .update(cx, |input, cx| input.set_content(label.clone(), cx));
                self.remote_destination
                    .update(cx, |input, cx| input.set_content(destination.clone(), cx));
                self.remote_port.update(cx, |input, cx| {
                    input.set_content(port.map(|port| port.to_string()).unwrap_or_default(), cx)
                });
                self.remote_identity_file.update(cx, |input, cx| {
                    input.set_content(
                        identity_file
                            .as_ref()
                            .map(|path| path.display().to_string())
                            .unwrap_or_default(),
                        cx,
                    )
                });
                self.remote_herdr_path
                    .update(cx, |input, cx| input.set_content(herdr_path.clone(), cx));
                self.remote_advanced_open =
                    port.is_some() || identity_file.is_some() || herdr_path != "herdr";
            }
        }
    }

    pub(crate) fn select_live_profile(&mut self, index: usize, cx: &mut Context<Self>) {
        cx.emit(HostCenterEvent::ProfileSelected(index));
    }
}

fn restore_cancelled_check(health: &mut HashMap<String, HostHealthView>, id: &str) {
    match health.remove(id) {
        Some(HostHealthView::Checking {
            previous: Some(previous),
        }) => {
            health.insert(id.to_owned(), *previous);
        }
        Some(HostHealthView::Checking { previous: None }) => {}
        Some(other) => {
            health.insert(id.to_owned(), other);
        }
        None => {}
    }
}

fn discard_probe_for_host(
    inflight: &mut HashMap<String, Task<()>>,
    queue: &mut VecDeque<(String, ConnectionProfile)>,
    health: &mut HashMap<String, HostHealthView>,
    id: &str,
) {
    inflight.remove(id);
    queue.retain(|(queued_id, _)| queued_id != id);
    health.remove(id);
}

fn restore_cancelled_checks(
    health: HashMap<String, HostHealthView>,
) -> HashMap<String, HostHealthView> {
    health
        .into_iter()
        .filter_map(|(id, view)| match view {
            HostHealthView::Checking { previous } => previous.map(|previous| (id, *previous)),
            other => Some((id, other)),
        })
        .collect()
}

impl Render for HostCenter {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.render_node_manager(cx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manual_profile(id: &str, destination: &str) -> ConnectionProfile {
        ConnectionProfile::Ssh {
            id: id.into(),
            label: destination.into(),
            destination: destination.into(),
            port: None,
            identity_file: None,
            herdr_path: "herdr".into(),
        }
    }

    fn sample_host_state() -> HostPersistState {
        let mut host_metadata = HashMap::new();
        host_metadata.insert(
            "manual-1".into(),
            HostMetadata {
                favorite: true,
                group: Some("lab".into()),
                tags: vec!["gpu".into()],
                ..HostMetadata::default()
            },
        );
        HostPersistState {
            profiles: vec![
                ConnectionProfile::default(),
                manual_profile("manual-1", "alpha.example"),
                ConnectionProfile::Ssh {
                    id: "ssh-config:build".into(),
                    label: "build".into(),
                    destination: "build".into(),
                    port: None,
                    identity_file: None,
                    herdr_path: "herdr".into(),
                },
            ],
            recent_connection_ids: vec!["manual-1".into(), "local".into()],
            host_metadata,
            host_groups: vec!["lab".into(), "edge".into()],
            host_health: HashMap::new(),
        }
    }

    fn parent_appearance() -> AppearanceSettings {
        AppearanceSettings {
            theme_family: "distinct-family".into(),
            mode: AppearanceMode::Light,
            backdrop: BackdropMode::Opaque,
            background_opacity: 77,
            font: TerminalFontSettings {
                family: "Distinct Mono".into(),
                size: 18,
                ligatures: false,
                thicken: true,
                cell_width_percent: 10,
                cell_height_percent: 12,
            },
        }
    }

    #[test]
    fn assembled_settings_include_host_center_catalog() {
        let host = sample_host_state();
        let settings = assemble_settings(&host, parent_appearance(), Language::English);

        assert_eq!(settings.host_metadata, host.host_metadata);
        assert_eq!(settings.host_groups, host.host_groups);
        assert_eq!(settings.recent_connection_ids, host.recent_connection_ids);
        assert_eq!(
            settings.host_metadata.get("manual-1").map(|m| m.favorite),
            Some(true)
        );
    }

    #[test]
    fn assembled_settings_keep_parent_appearance_and_language() {
        let host = sample_host_state();
        let appearance = parent_appearance();
        let settings = assemble_settings(&host, appearance.clone(), Language::English);

        assert_eq!(settings.appearance.theme_family, appearance.theme_family);
        assert_eq!(settings.appearance.mode, appearance.mode);
        assert_eq!(settings.appearance.backdrop, appearance.backdrop);
        assert_eq!(
            settings.appearance.background_opacity,
            appearance.background_opacity
        );
        assert_eq!(settings.appearance.font, appearance.font);
        assert_eq!(settings.language, Language::English);
        assert_ne!(settings.appearance.mode, AppearanceSettings::default().mode);
        assert_ne!(settings.language, Language::default());
    }

    #[test]
    fn assembled_settings_save_only_manual_profiles() {
        let host = sample_host_state();
        let settings = assemble_settings(&host, parent_appearance(), Language::English);

        assert_eq!(settings.connections.len(), 1);
        assert_eq!(settings.connections[0].id(), "manual-1");
        assert!(settings.connections.iter().all(is_saved_profile));
        assert!(
            !settings
                .connections
                .iter()
                .any(|profile| profile.id() == "local" || profile.id().starts_with("ssh-config:"))
        );
    }

    fn ready_health() -> HostHealthView {
        HostHealthView::Checked {
            cached: CachedHostHealth {
                status: HostHealthStatus::Ready,
                checked_at: 1,
                herdr_version: Some("0.8.1".into()),
                session_count: Some(2),
                latency_ms: 12,
            },
            detail: String::new(),
        }
    }

    #[test]
    fn cancelling_checks_restores_the_last_cached_health() {
        let mut health = HashMap::new();
        health.insert(
            "manual-1".into(),
            HostHealthView::Checking {
                previous: Some(Box::new(ready_health())),
            },
        );
        health.insert(
            "manual-2".into(),
            HostHealthView::Checking { previous: None },
        );
        health.insert("local".into(), ready_health());

        let restored = restore_cancelled_checks(health);

        assert!(
            matches!(
                restored.get("manual-1"),
                Some(HostHealthView::Checked { cached, .. })
                    if cached.status == HostHealthStatus::Ready
            ),
            "cached health must come back when the probe is dropped"
        );
        assert!(
            !restored.contains_key("manual-2"),
            "a probe with no prior cache must not stay Checking"
        );
        assert!(matches!(
            restored.get("local"),
            Some(HostHealthView::Checked { .. })
        ));
    }

    #[test]
    fn cancelling_one_host_leaves_other_probes_checking() {
        let mut health = HashMap::new();
        health.insert(
            "manual-1".into(),
            HostHealthView::Checking {
                previous: Some(Box::new(ready_health())),
            },
        );
        health.insert(
            "manual-2".into(),
            HostHealthView::Checking { previous: None },
        );

        restore_cancelled_check(&mut health, "manual-1");

        assert!(matches!(
            health.get("manual-1"),
            Some(HostHealthView::Checked { .. })
        ));
        assert!(matches!(
            health.get("manual-2"),
            Some(HostHealthView::Checking { .. })
        ));
    }

    #[test]
    fn cancelling_a_checked_host_keeps_its_cached_health() {
        let mut health = HashMap::new();
        health.insert("manual-1".into(), ready_health());

        restore_cancelled_check(&mut health, "manual-1");

        assert!(
            matches!(
                health.get("manual-1"),
                Some(HostHealthView::Checked { cached, .. })
                    if cached.status == HostHealthStatus::Ready
            ),
            "a completed cache is not a cancelled probe and must stay put"
        );
    }

    #[test]
    fn saving_an_edited_host_discards_health_from_the_old_connection() {
        let mut health = HashMap::new();
        health.insert(
            "manual-1".into(),
            HostHealthView::Checking {
                previous: Some(Box::new(ready_health())),
            },
        );

        let mut inflight = HashMap::new();
        let mut queue = VecDeque::new();
        discard_probe_for_host(&mut inflight, &mut queue, &mut health, "manual-1");

        assert!(
            !health.contains_key("manual-1"),
            "old current and previous results belong to the pre-edit connection"
        );
    }
}
