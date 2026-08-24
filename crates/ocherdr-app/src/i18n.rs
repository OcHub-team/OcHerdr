//! Translation lookup.
//!
//! Catalogs live in `crates/ocherdr-app/i18n/<locale>/*.toml`, one flat
//! `key = "text"` table per file, and `build.rs` compiles them into the static
//! tables included below. Translators edit TOML and never touch Rust.
//!
//! The build script rejects a catalog that is missing a key or whose
//! placeholders disagree with the reference catalog, so those are build
//! failures rather than blank labels. Unused keys fail clippy: generated
//! constants are `pub(crate)`, this is a binary crate, and CI denies warnings.
//!
//! Only user-visible prose belongs in a catalog. Element ids, settings keys
//! and any string compared with `==` are identifiers — translating them
//! changes behaviour.

use ocherdr_core::{AgentNameError, AgentStatus};
use serde::{Deserialize, Serialize};

use crate::tf;

include!(concat!(env!("OUT_DIR"), "/i18n_generated.rs"));

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Language {
    #[default]
    System,
    English,
    SimplifiedChinese,
}

impl Language {
    pub(crate) const fn index(self) -> usize {
        match self {
            Self::System => 0,
            Self::English => 1,
            Self::SimplifiedChinese => 2,
        }
    }

    pub(crate) const fn from_index(index: usize) -> Self {
        match index {
            1 => Self::English,
            2 => Self::SimplifiedChinese,
            _ => Self::System,
        }
    }

    pub(crate) const fn as_config(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::English => "en",
            Self::SimplifiedChinese => "zh-Hans",
        }
    }

    pub(crate) fn from_config(value: &str) -> Option<Self> {
        match value.trim() {
            "system" => Some(Self::System),
            "en" => Some(Self::English),
            "zh-Hans" | "zh-hans" => Some(Self::SimplifiedChinese),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Locale {
    English,
    SimplifiedChinese,
}

impl Locale {
    fn from_identifier(identifier: &str) -> Self {
        let identifier = identifier.to_ascii_lowercase().replace('_', "-");
        if identifier == "zh"
            || identifier.starts_with("zh-hans")
            || identifier.starts_with("zh-cn")
            || identifier.starts_with("zh-sg")
        {
            Self::SimplifiedChinese
        } else {
            Self::English
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct I18n {
    preference: Language,
    locale: Locale,
}

impl I18n {
    pub(crate) fn new(preference: Language) -> Self {
        let locale = match preference {
            Language::System => sys_locale::get_locale()
                .as_deref()
                .map(Locale::from_identifier)
                .unwrap_or(Locale::English),
            Language::English => Locale::English,
            Language::SimplifiedChinese => Locale::SimplifiedChinese,
        };
        Self { preference, locale }
    }

    /// Set ochub-ui's global locale. Call at startup before the component
    /// registry is installed; `set_preference` calls this when the user
    /// changes language.
    pub(crate) fn install(preference: Language) {
        Self::new(preference).install_component_locale();
    }

    pub(crate) const fn preference(self) -> Language {
        self.preference
    }

    pub(crate) fn set_preference(&mut self, preference: Language) {
        *self = Self::new(preference);
        self.install_component_locale();
    }

    fn install_component_locale(self) {
        ochub_ui::i18n::install(match self.locale {
            Locale::English => ochub_ui::i18n::Locale::En,
            Locale::SimplifiedChinese => ochub_ui::i18n::Locale::Zh,
        });
    }

    /// The raw template for a key in this instance's locale.
    #[inline]
    pub(crate) fn raw(self, key: Key) -> &'static str {
        let index = key.index();
        match self.locale {
            Locale::English => EN[index],
            Locale::SimplifiedChinese => ZH[index],
        }
    }

    #[inline]
    pub(crate) fn text(self, key: Key) -> &'static str {
        self.raw(key)
    }

    pub(crate) fn agent_status(self, status: AgentStatus) -> &'static str {
        self.text(match status {
            AgentStatus::Idle => k::TERMINAL_AGENT_IDLE,
            AgentStatus::Working => k::TERMINAL_AGENT_WORKING,
            AgentStatus::Blocked => k::TERMINAL_AGENT_BLOCKED,
            AgentStatus::Done => k::TERMINAL_AGENT_DONE,
            AgentStatus::Unknown => k::TERMINAL_AGENT_UNKNOWN,
        })
    }

    pub(crate) fn agent_location(self, workspace: &str, tab: &str, pane: &str) -> String {
        tf!(
            self,
            k::AGENT_LOCATION,
            workspace = workspace,
            tab = tab,
            pane = pane
        )
    }

    pub(crate) fn agent_name_error(self, error: AgentNameError) -> &'static str {
        self.text(match error {
            AgentNameError::FirstCharacter => k::AGENT_NAME_INVALID_FIRST,
            AgentNameError::InvalidCharacter => k::AGENT_NAME_INVALID_CHAR,
            AgentNameError::TooLong => k::AGENT_NAME_INVALID_LENGTH,
        })
    }

    pub(crate) fn close_action(self, kind: Key) -> String {
        tf!(self, k::COMMON_CLOSE_ACTION, kind = self.text(kind))
    }

    pub(crate) fn close_title(self, kind: Key) -> String {
        tf!(self, k::COMMON_CLOSE_TITLE, kind = self.text(kind))
    }

    pub(crate) fn switch_host_prompt(self, current: &str, next: &str) -> String {
        tf!(self, k::HOSTS_SWITCH_PROMPT, current = current, next = next)
    }

    pub(crate) fn close_prompt(self, label: &str) -> String {
        tf!(self, k::COMMON_CLOSE_PROMPT, label = label)
    }

    pub(crate) fn remove_worktree_prompt(self, label: &str) -> String {
        tf!(self, k::WORKTREE_REMOVE_PROMPT, label = label)
    }

    pub(crate) fn force_remove_worktree_prompt(self, label: &str) -> String {
        tf!(self, k::WORKTREE_REMOVE_FORCE_PROMPT, label = label)
    }

    pub(crate) fn force_remove_worktree_detail(self, error: &str) -> String {
        tf!(self, k::WORKTREE_REMOVE_FORCE_BODY, error = error)
    }

    pub(crate) fn rename_title(self, kind: Key) -> String {
        tf!(self, k::COMMON_RENAME_TITLE, kind = self.text(kind))
    }

    pub(crate) fn remove_node_prompt(self, node_name: &str) -> String {
        tf!(self, k::HOSTS_REMOVE_NODE_PROMPT, name = node_name)
    }

    pub(crate) fn selected_hosts(self, count: usize) -> String {
        let key = if count == 1 {
            k::HOSTS_SELECTED_ONE
        } else {
            k::HOSTS_SELECTED_OTHER
        };
        tf!(self, key, count = count)
    }

    pub(crate) fn checked_ago(self, seconds: u64) -> String {
        if seconds < 60 {
            self.text(k::HOSTS_CHECKED_JUST_NOW).to_owned()
        } else if seconds < 3_600 {
            tf!(self, k::HOSTS_CHECKED_MINUTES, minutes = seconds / 60)
        } else {
            tf!(self, k::HOSTS_CHECKED_HOURS, hours = seconds / 3_600)
        }
    }

    pub(crate) fn use_theme_label(self, theme_name: &str) -> String {
        tf!(self, k::APPEARANCE_THEME_USE, name = theme_name)
    }

    pub(crate) fn missing_theme_detail(self, name: &str) -> String {
        tf!(self, k::NOTIFY_DETAIL_MISSING_THEME, name = name)
    }

    pub(crate) fn running_operation(self, method: &str) -> String {
        tf!(self, k::NOTIFY_RUNNING, method = method)
    }

    pub(crate) fn herdr_status(
        self,
        version: &str,
        protocol: u32,
        workspace_count: usize,
    ) -> String {
        self.format_herdr_status(
            version,
            protocol,
            workspace_count,
            k::TERMINAL_HERDR_SUBSCRIPTION,
        )
    }

    pub(crate) fn herdr_snapshot_status(
        self,
        version: &str,
        protocol: u32,
        workspace_count: usize,
    ) -> String {
        self.format_herdr_status(
            version,
            protocol,
            workspace_count,
            k::TERMINAL_HERDR_SNAPSHOT,
        )
    }

    fn format_herdr_status(
        self,
        version: &str,
        protocol: u32,
        workspace_count: usize,
        link: Key,
    ) -> String {
        let link = self.text(link);
        let key = if workspace_count == 1 {
            k::TERMINAL_HERDR_STATUS_ONE
        } else {
            k::TERMINAL_HERDR_STATUS_OTHER
        };
        tf!(
            self,
            key,
            version = version,
            protocol = protocol,
            link = link,
            count = workspace_count
        )
    }
}

/// Substitute `{name}` placeholders, honouring `{{` and `}}` as escapes.
///
/// A placeholder with no matching argument is left as written rather than
/// silently dropped, so a mistake shows up in the UI as `{count}` instead of a
/// sentence with a hole in it. `build.rs` already guarantees the locales agree
/// on which placeholders exist.
pub(crate) fn format_named(template: &str, args: &[(&str, String)]) -> String {
    let mut out = String::with_capacity(template.len());
    let bytes = template.as_bytes();
    let mut literal_start = 0;
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'{' if bytes.get(i + 1) == Some(&b'{') => {
                out.push_str(&template[literal_start..i]);
                out.push('{');
                i += 2;
                literal_start = i;
            }
            b'}' if bytes.get(i + 1) == Some(&b'}') => {
                out.push_str(&template[literal_start..i]);
                out.push('}');
                i += 2;
                literal_start = i;
            }
            b'{' => {
                let start = i + 1;
                let Some(relative_end) = bytes[start..].iter().position(|byte| *byte == b'}')
                else {
                    break;
                };
                let end = start + relative_end;
                let name = &template[start..end];
                out.push_str(&template[literal_start..i]);
                match args.iter().find(|(key, _)| *key == name) {
                    Some((_, value)) => out.push_str(value),
                    None => out.push_str(&template[i..=end]),
                }
                i = end + 1;
                literal_start = i;
            }
            _ => i += 1,
        }
    }
    out.push_str(&template[literal_start..]);
    out
}

/// A translated string with named arguments: `tf!(i18n, k::HOSTS_SELECTED_ONE, count = n)`.
///
/// Argument names must match the `{name}` placeholders in the catalog. Values
/// only need to implement `Display`.
#[macro_export]
macro_rules! tf {
    ($i18n:expr, $key:expr $(, $name:ident = $value:expr)* $(,)?) => {
        $crate::i18n::format_named(
            $i18n.raw($key),
            &[$((stringify!($name), $value.to_string())),*],
        )
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_only_simplified_chinese_locales() {
        assert_eq!(
            Locale::from_identifier("zh-Hans-SG"),
            Locale::SimplifiedChinese
        );
        assert_eq!(Locale::from_identifier("zh_CN"), Locale::SimplifiedChinese);
        assert_eq!(Locale::from_identifier("zh-Hant-TW"), Locale::English);
        assert_eq!(Locale::from_identifier("en-SG"), Locale::English);
    }

    #[test]
    fn language_round_trips_through_settings_format() {
        let json = serde_json::to_string(&Language::SimplifiedChinese).unwrap();
        assert_eq!(json, "\"simplified_chinese\"");
        assert_eq!(
            serde_json::from_str::<Language>(&json).unwrap(),
            Language::SimplifiedChinese
        );
    }

    #[test]
    fn checked_ago_picks_just_now_minutes_or_hours() {
        let english = I18n::new(Language::English);
        let chinese = I18n::new(Language::SimplifiedChinese);

        assert_eq!(english.checked_ago(0), "checked just now");
        assert_eq!(english.checked_ago(59), "checked just now");
        assert_eq!(english.checked_ago(60), "checked 1m ago");
        assert_eq!(english.checked_ago(3_599), "checked 59m ago");
        assert_eq!(english.checked_ago(3_600), "checked 1h ago");

        assert_eq!(chinese.checked_ago(0), "刚刚检查");
        assert_eq!(chinese.checked_ago(59), "刚刚检查");
        assert_eq!(chinese.checked_ago(60), "1 分钟前检查");
        assert_eq!(chinese.checked_ago(3_599), "59 分钟前检查");
        assert_eq!(chinese.checked_ago(3_600), "1 小时前检查");
    }

    #[test]
    fn selected_hosts_uses_english_plural_only() {
        let english = I18n::new(Language::English);
        let chinese = I18n::new(Language::SimplifiedChinese);

        assert_eq!(english.selected_hosts(1), "1 host selected");
        assert_eq!(english.selected_hosts(0), "0 hosts selected");
        assert_eq!(english.selected_hosts(2), "2 hosts selected");
        assert_eq!(chinese.selected_hosts(1), "已选择 1 台主机");
        assert_eq!(chinese.selected_hosts(0), "已选择 0 台主机");
        assert_eq!(chinese.selected_hosts(2), "已选择 2 台主机");
    }
}
