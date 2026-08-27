use serde::{Deserialize, Serialize};

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
        let i18n = Self { preference, locale };
        i18n.install_component_locale();
        i18n
    }

    pub(crate) const fn preference(self) -> Language {
        self.preference
    }

    pub(crate) fn set_preference(&mut self, preference: Language) {
        *self = Self::new(preference);
    }

    fn install_component_locale(self) {
        ochub_ui::i18n::install(match self.locale {
            Locale::English => ochub_ui::i18n::Locale::En,
            Locale::SimplifiedChinese => ochub_ui::i18n::Locale::Zh,
        });
    }

    pub(crate) fn text(self, english: &'static str) -> &'static str {
        match self.locale {
            Locale::English => english,
            Locale::SimplifiedChinese => zh_hans(english),
        }
    }

    pub(crate) fn empty_session_hint(self) -> &'static str {
        self.text(empty_session_hint_english())
    }

    pub(crate) fn close_action(self, kind: &str) -> String {
        match self.locale {
            Locale::English => format!("Close {}", self.kind(kind)),
            Locale::SimplifiedChinese => format!("关闭{}", self.kind(kind)),
        }
    }

    pub(crate) fn close_title(self, kind: &str) -> String {
        format!("{}?", self.close_action(kind))
    }

    pub(crate) fn close_prompt(self, label: &str) -> String {
        match self.locale {
            Locale::English => format!("Close {label}?"),
            Locale::SimplifiedChinese => format!("关闭“{label}”？"),
        }
    }

    pub(crate) fn rename_title(self, kind: &str) -> String {
        match self.locale {
            Locale::English => format!("Rename {}", self.kind(kind)),
            Locale::SimplifiedChinese => format!("重命名{}", self.kind(kind)),
        }
    }

    pub(crate) fn remove_node_prompt(self, node_name: &str) -> String {
        match self.locale {
            Locale::English => format!("Remove {node_name} from OcHerdr?"),
            Locale::SimplifiedChinese => format!("从 OcHerdr 中移除“{node_name}”？"),
        }
    }

    pub(crate) fn use_theme_label(self, theme_name: &str) -> String {
        match self.locale {
            Locale::English => format!("Use {theme_name} theme"),
            Locale::SimplifiedChinese => format!("使用 {theme_name} 主题"),
        }
    }

    pub(crate) fn running_operation(self, method: &str) -> String {
        match self.locale {
            Locale::English => format!("Running {method}…"),
            Locale::SimplifiedChinese => format!("正在执行 {method}…"),
        }
    }

    pub(crate) fn herdr_status(
        self,
        version: &str,
        protocol: u32,
        subscription: bool,
        workspace_count: usize,
    ) -> String {
        match self.locale {
            Locale::English => format!(
                "Herdr {version} · protocol {protocol} · connected · {} · {workspace_count} workspace{}",
                if subscription {
                    "subscription active"
                } else {
                    "snapshot"
                },
                if workspace_count == 1 { "" } else { "s" },
            ),
            Locale::SimplifiedChinese => format!(
                "Herdr {version} · 协议 {protocol} · 已连接 · {} · {workspace_count} 个工作区",
                if subscription {
                    "实时订阅"
                } else {
                    "状态快照"
                },
            ),
        }
    }

    fn kind(self, kind: &str) -> &str {
        match (self.locale, kind) {
            (Locale::SimplifiedChinese, "workspace") => "工作区",
            (Locale::SimplifiedChinese, "tab") => "标签页",
            (Locale::SimplifiedChinese, "pane") => "窗格",
            (Locale::SimplifiedChinese, _) => "项目",
            (_, "workspace") => "workspace",
            (_, "tab") => "tab",
            (_, "pane") => "pane",
            _ => "item",
        }
    }
}

pub(crate) fn uses_macos_shortcuts() -> bool {
    cfg!(target_os = "macos")
}

pub(crate) fn new_tab_shortcut() -> &'static str {
    if uses_macos_shortcuts() {
        "⌘T"
    } else {
        "Ctrl-T"
    }
}

pub(crate) fn close_tab_shortcut() -> &'static str {
    if uses_macos_shortcuts() {
        "⌘W"
    } else {
        "Ctrl-W"
    }
}

fn empty_session_hint_english() -> &'static str {
    if uses_macos_shortcuts() {
        "Press ⌘T to open your first terminal."
    } else {
        "Press Ctrl-T to open your first terminal."
    }
}

fn zh_hans(english: &'static str) -> &'static str {
    match english {
        "Spaces" => "空间",
        "CONNECTIONS" => "连接",
        "WORKSPACES" => "工作区",
        "New workspace" => "新建工作区",
        "new" => "新建",
        "AGENTS" => "智能体",
        "STATUS" => "状态",
        "idle" => "空闲",
        "working" => "工作中",
        "blocked" => "受阻",
        "done" => "已完成",
        "unknown" => "未知",
        "Close tab" => "关闭标签页",
        "New tab" => "新建标签页",
        "Split pane right" => "向右拆分窗格",
        "Split pane down" => "向下拆分窗格",
        "Zoom pane" => "缩放窗格",
        "Close pane" => "关闭窗格",
        "Appearance" => "外观",
        "Herdr settings" => "Herdr 设置",
        "Remote" => "远程连接",
        "PREFIX" => "前缀键",
        "C new tab · ⇧N new workspace · S settings · 1–9 switch tab" => {
            "C 新建标签页 · ⇧N 新建工作区 · S 设置 · 1–9 切换标签页"
        }
        "Connection unavailable" => "连接不可用",
        "No Herdr session" => "没有 Herdr 会话",
        "Refresh" => "刷新",
        "No running Herdr session" => "没有正在运行的 Herdr 会话",
        "Start Herdr locally or open Remote in the top-right." => {
            "请在本机启动 Herdr，或打开右上角的远程连接。"
        }
        "This session has no tabs" => "此会话没有标签页",
        "Create a workspace to open the first terminal." => "新建工作区以打开第一个终端。",
        "Press ⌘T to open your first terminal." => "按 ⌘T 打开第一个终端。",
        "Press Cmd-T to open your first terminal." => "按 Cmd-T 打开第一个终端。",
        "Press Ctrl-T to open your first terminal." => "按 Ctrl-T 打开第一个终端。",
        "Connecting…" => "正在连接…",
        "System" => "跟随系统",
        "English" => "English",
        "Simplified Chinese" => "简体中文",
        "Language" => "语言",
        "Choose the language used by OcHerdr." => "选择 OcHerdr 的界面语言。",
        "Light" => "浅色",
        "Dark" => "深色",
        "Opaque" => "不透明",
        "Clear" => "透明",
        "Blur" => "毛玻璃",
        "Done" => "完成",
        "Close" => "关闭",
        "Theme" => "主题",
        "Choose a color family. Each family includes light and dark variants." => {
            "选择配色系列；每个系列均包含浅色和深色版本。"
        }
        "Light and dark variants" => "包含浅色和深色版本",
        "Follow macOS or pin a variant." => "跟随 macOS，或固定使用一种外观。",
        "Window background" => "窗口背景",
        "Clear keeps true transparency; Blur uses the native macOS backdrop." => {
            "透明模式保留真实透明效果；毛玻璃使用 macOS 原生背景材质。"
        }
        "Background opacity" => "背景不透明度",
        "Applied to terminal and shell surfaces when transparency is enabled." => {
            "启用透明背景时应用于终端及应用表面。"
        }
        "Search hosts" => "搜索主机",
        "Local" => "本机",
        "Default" => "默认",
        "CURRENT" => "当前",
        "SAVED" => "已保存",
        "SSH CONFIG" => "SSH 配置",
        "This Mac" => "这台 Mac",
        "Saved in OcHerdr" => "保存在 OcHerdr 中",
        "Imported from ~/.ssh/config" => "从 ~/.ssh/config 导入",
        "No matching hosts" => "没有匹配的主机",
        "Try a host name, SSH alias, or address." => "尝试输入主机名、SSH 别名或地址。",
        "Remote connections" => "远程连接",
        "New SSH" => "新建 SSH",
        "Select a connection" => "选择一个连接",
        "System default" => "系统默认",
        "SSH config or agent" => "SSH 配置或代理",
        "Connected" => "已连接",
        "Source" => "来源",
        "Identity" => "身份文件",
        "Herdr command" => "Herdr 命令",
        "Uses OpenSSH config, keys, agent, and known_hosts." => {
            "使用 OpenSSH 配置、密钥、代理和 known_hosts。"
        }
        "Remove saved host" => "移除已保存的主机",
        "Reconnect" => "重新连接",
        "Connect" => "连接",
        "Cancel" => "取消",
        "Save & connect" => "保存并连接",
        "New SSH connection" => "新建 SSH 连接",
        "Save a host, then discover its Herdr sessions." => "保存主机，然后发现其中的 Herdr 会话。",
        "Label" => "名称",
        "Name shown in OcHerdr." => "显示在 OcHerdr 中的名称。",
        "Port" => "端口",
        "Uses SSH config when empty." => "留空时使用 SSH 配置。",
        "Destination" => "目标地址",
        "SSH alias or user@host from ~/.ssh/config." => {
            "填写 ~/.ssh/config 中的 SSH 别名或 user@host。"
        }
        "Identity file" => "身份文件",
        "Optional key path; SSH agent still works." => "可选密钥路径；仍可使用 SSH 代理。",
        "Remote command or path." => "远程命令或路径。",
        "Production" => "生产环境",
        "user@example.com or SSH alias" => "user@example.com 或 SSH 别名",
        "22 (optional)" => "22（可选）",
        "~/.ssh/id_ed25519 (optional)" => "~/.ssh/id_ed25519（可选）",
        "Name" => "名称",
        "this node" => "此节点",
        "this item" => "此项目",
        "Remove node" => "移除节点",
        "Remove SSH node?" => "移除 SSH 节点？",
        "This only removes the saved node profile. SSH keys and ~/.ssh/config are not changed." => {
            "这只会移除已保存的节点配置，不会修改 SSH 密钥或 ~/.ssh/config。"
        }
        "Processes owned by this Herdr hierarchy item may be terminated. Closing a final tab also closes its workspace." => {
            "此 Herdr 层级项目拥有的进程可能会终止；关闭最后一个标签页也会关闭其工作区。"
        }
        "Rename" => "重命名",
        "Leave empty to clear the custom pane name." => "留空可清除自定义窗格名称。",
        "Saved directly to the active Herdr session." => "名称将直接保存到当前 Herdr 会话。",
        "Rename pane" => "重命名窗格",
        "Split right" => "向右拆分",
        "Split down" => "向下拆分",
        "Zoom" => "缩放",
        "Indicators" => "状态标记",
        "Sound" => "声音",
        "Toast" => "通知弹窗",
        "Pane labels" => "窗格标签",
        "Integrations" => "集成",
        "theme" => "主题",
        "Themes exposed by Herdr's native TUI settings." => "Herdr 原生 TUI 设置提供的主题。",
        "agent status indicators" => "智能体状态标记",
        "Choose the symbols used for agent state in the TUI." => {
            "选择 TUI 中用于表示智能体状态的符号。"
        }
        "sound alerts" => "声音提醒",
        "Play sounds when agents change state in the background." => {
            "智能体在后台改变状态时播放提示音。"
        }
        "notification popups" => "通知弹窗",
        "Choose where background notifications are delivered." => "选择后台通知的发送位置。",
        "agent border labels" => "智能体边框标签",
        "Show detected agent names in split-pane borders." => {
            "在拆分窗格边框中显示检测到的智能体名称。"
        }
        "agent integrations" => "智能体集成",
        "Let supported agents report state directly to Herdr." => {
            "允许受支持的智能体直接向 Herdr 报告状态。"
        }
        "dark" => "深色",
        "light" => "浅色",
        "inherit host colors" => "继承宿主颜色",
        "compact color status" => "紧凑彩色状态",
        "shape and color status" => "形状和彩色状态",
        "on" => "开启",
        "off" => "关闭",
        "enable alerts" => "启用提醒",
        "silence alerts" => "关闭提醒",
        "disabled" => "已禁用",
        "inside herdr" => "Herdr 内",
        "TUI popup" => "TUI 弹窗",
        "via terminal" => "通过终端",
        "terminal notification" => "终端通知",
        "via system" => "通过系统",
        "macOS notification" => "macOS 通知",
        "show labels" => "显示标签",
        "hide labels" => "隐藏标签",
        "integration target" => "集成目标",
        "Open native TUI" => "打开原生 TUI",
        "This mirrors Herdr's TUI settings surface. Protocol 20 does not expose live setting values; open the native TUI and press Ctrl+B, then S to inspect or apply the selected session." => {
            "此处映射 Herdr 的 TUI 设置界面。协议 20 不提供实时设置值；请打开原生 TUI，再按 Ctrl+B、S 查看或应用当前会话设置。"
        }
        "Discovering Herdr sessions…" => "正在发现 Herdr 会话…",
        "Workspace and tab names cannot be empty." => "工作区和标签页名称不能为空。",
        "SSH destination is required." => "必须填写 SSH 目标地址。",
        "SSH port must be a number from 1 to 65535." => "SSH 端口必须是 1 到 65535 之间的数字。",
        "No Herdr session is selected." => "尚未选择 Herdr 会话。",
        "Waiting for terminal frame…" => "正在等待终端画面…",
        _ => english,
    }
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
    fn chinese_catalog_translates_connections_and_falls_back_to_english() {
        let i18n = I18n::new(Language::SimplifiedChinese);

        assert_eq!(i18n.text("CONNECTIONS"), "连接");
        assert_eq!(i18n.text("OcHerdr"), "OcHerdr");
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
    fn empty_session_hint_uses_platform_new_tab_shortcut() {
        let english = I18n::new(Language::English);
        let chinese = I18n::new(Language::SimplifiedChinese);

        assert_eq!(english.empty_session_hint(), empty_session_hint_english());
        assert_eq!(
            chinese.empty_session_hint(),
            zh_hans(empty_session_hint_english())
        );
        assert_eq!(
            english.empty_session_hint().contains('⌘')
                || english.empty_session_hint().contains("Cmd-T"),
            cfg!(target_os = "macos")
        );
        assert_eq!(
            english.empty_session_hint().contains("Ctrl-T"),
            !cfg!(target_os = "macos")
        );
        assert_eq!(
            new_tab_shortcut(),
            if cfg!(target_os = "macos") {
                "⌘T"
            } else {
                "Ctrl-T"
            }
        );
    }
}
