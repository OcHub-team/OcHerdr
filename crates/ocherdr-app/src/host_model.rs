use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RemoteForm {
    Create,
    Edit(usize),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ConnectionSource {
    ThisMac,
    Saved,
    SshConfig,
}

impl ConnectionSource {
    pub(crate) fn label(self, i18n: I18n) -> &'static str {
        i18n.text(match self {
            Self::ThisMac => k::HOSTS_SOURCE_THIS_MAC,
            Self::Saved => k::HOSTS_SOURCE_SAVED_SHORT,
            Self::SshConfig => k::HOSTS_SOURCE_SSH_CONFIG,
        })
    }

    pub(crate) fn description(self, i18n: I18n) -> &'static str {
        i18n.text(match self {
            Self::ThisMac => k::HOSTS_SOURCE_THIS_MAC_DESCRIPTION,
            Self::Saved => k::HOSTS_SOURCE_SAVED,
            Self::SshConfig => k::HOSTS_SOURCE_SSH_CONFIG_READONLY,
        })
    }
}

pub(crate) fn connection_source(profile: &ConnectionProfile) -> ConnectionSource {
    if matches!(profile, ConnectionProfile::Local { .. }) {
        ConnectionSource::ThisMac
    } else if profile.id().starts_with("manual-") {
        ConnectionSource::Saved
    } else {
        ConnectionSource::SshConfig
    }
}

pub(crate) fn is_saved_profile(profile: &ConnectionProfile) -> bool {
    profile.id().starts_with("manual-")
}

pub(crate) fn ssh_destination(profile: &ConnectionProfile) -> Option<&str> {
    match profile {
        ConnectionProfile::Ssh { destination, .. } => Some(destination.as_str()),
        ConnectionProfile::Local { .. } => None,
    }
}

pub(crate) fn ssh_config_covered_by_saved(
    profiles: &[ConnectionProfile],
    destination: &str,
) -> bool {
    profiles
        .iter()
        .any(|profile| is_saved_profile(profile) && ssh_destination(profile) == Some(destination))
}

pub(crate) fn remember_recent(recents: &mut Vec<String>, id: &str) {
    recents.retain(|existing| existing != id);
    recents.insert(0, id.to_owned());
    recents.truncate(8);
}

pub(crate) fn normalize_recent_host_id(id: &str, profiles: &[ConnectionProfile]) -> Option<String> {
    if profiles.iter().any(|profile| profile.id() == id) {
        return Some(id.to_owned());
    }
    let legacy_alias = id
        .strip_prefix("ssh-")
        .and_then(|rest| rest.split_once('-').map(|(_, alias)| alias))?;
    profiles
        .iter()
        .find(|profile| ssh_destination(profile) == Some(legacy_alias))
        .map(|profile| profile.id().to_owned())
}

pub(crate) fn parse_host_tags(value: &str) -> Vec<String> {
    let mut tags = Vec::new();
    for tag in value
        .split(',')
        .map(str::trim)
        .filter(|tag| !tag.is_empty())
    {
        if !tags
            .iter()
            .any(|existing: &String| existing.eq_ignore_ascii_case(tag))
        {
            tags.push(tag.to_owned());
        }
    }
    tags
}

pub(crate) fn switch_requires_confirm(from: usize, to: usize, live_session: bool) -> bool {
    from != to && live_session
}

pub(crate) fn profile_index_by_id(profiles: &[ConnectionProfile], id: &str) -> Option<usize> {
    profiles.iter().position(|profile| profile.id() == id)
}

pub(crate) fn confirmed_host_index(
    overlay: &Overlay,
    profiles: &[ConnectionProfile],
) -> Option<usize> {
    let id = match overlay {
        Overlay::ConfirmSwitchProfile { id, .. } | Overlay::ConfirmRemoveProfile(id) => id.as_str(),
        _ => return None,
    };
    profile_index_by_id(profiles, id)
}

pub(crate) fn next_manual_profile_id(profiles: &[ConnectionProfile]) -> u64 {
    profiles
        .iter()
        .filter_map(|profile| profile.id().strip_prefix("manual-"))
        .filter_map(|suffix| suffix.parse::<u64>().ok())
        .max()
        .unwrap_or(0)
        + 1
}

pub(crate) fn profile_display_label(profile: &ConnectionProfile, i18n: I18n) -> String {
    if matches!(profile, ConnectionProfile::Local { .. }) {
        i18n.text(k::HOSTS_SOURCE_THIS_MAC).to_owned()
    } else {
        profile.label().to_owned()
    }
}

pub(crate) fn unix_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

pub(crate) fn profile_endpoint(profile: &ConnectionProfile) -> String {
    match profile {
        ConnectionProfile::Local { .. } => "localhost".into(),
        ConnectionProfile::Ssh {
            destination, port, ..
        } => port.map_or_else(
            || destination.clone(),
            |port| format!("{destination}:{port}"),
        ),
    }
}

pub(crate) fn profile_matches_search(profile: &ConnectionProfile, query: &str, i18n: I18n) -> bool {
    if query.is_empty() {
        return true;
    }
    profile.label().to_lowercase().contains(query)
        || profile_endpoint(profile).to_lowercase().contains(query)
        || connection_source(profile)
            .label(i18n)
            .to_lowercase()
            .contains(query)
        || connection_source(profile)
            .description(i18n)
            .to_lowercase()
            .contains(query)
}

pub(crate) fn ssh_config_entry_is_hidden(
    profiles: &[ConnectionProfile],
    profile: &ConnectionProfile,
) -> bool {
    connection_source(profile) == ConnectionSource::SshConfig
        && ssh_destination(profile)
            .is_some_and(|destination| ssh_config_covered_by_saved(profiles, destination))
}

pub(crate) fn host_display_label_for(
    profile: &ConnectionProfile,
    metadata: Option<&HostMetadata>,
    i18n: I18n,
) -> String {
    metadata
        .and_then(|metadata| metadata.display_name.clone())
        .unwrap_or_else(|| profile_display_label(profile, i18n))
}

pub(crate) fn host_fits_filter(
    profile: &ConnectionProfile,
    filter: &HostFilter,
    metadata: Option<&HostMetadata>,
    recent_ids: &[String],
    orphaned: &HashSet<String>,
    health: &HashMap<String, HostHealthView>,
) -> bool {
    match filter {
        HostFilter::All => true,
        HostFilter::Favorites => metadata.is_some_and(|value| value.favorite),
        HostFilter::Recent => recent_ids.iter().any(|id| id == profile.id()),
        HostFilter::Attention => {
            orphaned.contains(profile.id())
                || health.get(profile.id()).is_some_and(|health| match health {
                    HostHealthView::Checking { .. } => false,
                    HostHealthView::Checked { cached, .. } => {
                        cached.status != HostHealthStatus::Ready
                    }
                })
        }
        HostFilter::Source(source) => connection_source(profile) == *source,
        HostFilter::Group(group) => {
            metadata.and_then(|value| value.group.as_deref()) == Some(group.as_str())
        }
        HostFilter::Tag(tag) => {
            metadata.is_some_and(|value| value.tags.iter().any(|candidate| candidate == tag))
        }
    }
}

pub(crate) struct HostCatalog<'a> {
    pub(crate) profiles: &'a [ConnectionProfile],
    pub(crate) metadata: &'a HashMap<String, HostMetadata>,
    pub(crate) recent_ids: &'a [String],
    pub(crate) orphaned: &'a HashSet<String>,
    pub(crate) health: &'a HashMap<String, HostHealthView>,
}

pub(crate) fn visible_host_indices(
    catalog: &HostCatalog<'_>,
    filter: &HostFilter,
    query: &str,
    current_index: usize,
    i18n: I18n,
) -> Vec<usize> {
    let query = query.trim().to_lowercase();
    let recent_positions = catalog
        .recent_ids
        .iter()
        .enumerate()
        .map(|(position, id)| (id.as_str(), position))
        .collect::<HashMap<_, _>>();
    let mut indexes = catalog
        .profiles
        .iter()
        .enumerate()
        .filter(|(_, profile)| {
            if ssh_config_entry_is_hidden(catalog.profiles, profile) {
                return false;
            }
            let meta = catalog.metadata.get(profile.id());
            let search_matches = profile_matches_search(profile, &query, i18n)
                || meta.is_some_and(|metadata| {
                    metadata
                        .display_name
                        .as_deref()
                        .is_some_and(|name| name.to_lowercase().contains(&query))
                        || metadata
                            .group
                            .as_deref()
                            .is_some_and(|group| group.to_lowercase().contains(&query))
                        || metadata
                            .tags
                            .iter()
                            .any(|tag| tag.to_lowercase().contains(&query))
                });
            search_matches
                && host_fits_filter(
                    profile,
                    filter,
                    meta,
                    catalog.recent_ids,
                    catalog.orphaned,
                    catalog.health,
                )
        })
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    indexes.sort_by_key(|index| {
        let profile = &catalog.profiles[*index];
        let meta = catalog.metadata.get(profile.id());
        (
            usize::from(*index != current_index),
            usize::from(!meta.is_some_and(|value| value.favorite)),
            recent_positions
                .get(profile.id())
                .copied()
                .unwrap_or(usize::MAX),
            host_display_label_for(profile, meta, i18n).to_lowercase(),
        )
    });
    indexes
}
