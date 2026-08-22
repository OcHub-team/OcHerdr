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

    pub(crate) fn close_action(self, kind: &str) -> String {
        match self.locale {
            Locale::English => format!("Close {}", self.kind(kind)),
            Locale::SimplifiedChinese => format!("关闭{}", self.kind(kind)),
        }
    }

    pub(crate) fn close_title(self, kind: &str) -> String {
        format!("{}?", self.close_action(kind))
    }

    pub(crate) fn switch_host_prompt(self, current: &str, next: &str) -> String {
        match self.locale {
            Locale::English => format!("Leave {current} and connect to {next}?"),
            Locale::SimplifiedChinese => format!("离开“{current}”，连接到“{next}”？"),
        }
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

    pub(crate) fn selected_hosts(self, count: usize) -> String {
        match self.locale {
            Locale::English => {
                format!("{count} host{} selected", if count == 1 { "" } else { "s" })
            }
            Locale::SimplifiedChinese => format!("已选择 {count} 台主机"),
        }
    }

    pub(crate) fn checked_ago(self, seconds: u64) -> String {
        match self.locale {
            Locale::English if seconds < 60 => "checked just now".into(),
            Locale::English if seconds < 3_600 => format!("checked {}m ago", seconds / 60),
            Locale::English => format!("checked {}h ago", seconds / 3_600),
            Locale::SimplifiedChinese if seconds < 60 => "刚刚检查".into(),
            Locale::SimplifiedChinese if seconds < 3_600 => {
                format!("{} 分钟前检查", seconds / 60)
            }
            Locale::SimplifiedChinese => format!("{} 小时前检查", seconds / 3_600),
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

fn zh_hans(english: &'static str) -> &'static str {
    match english {
        "Spaces" => "空间",
        "Terminal" => "终端",
        "Status bar" => "状态栏",
        "Tabs" => "标签页",
        "Pane actions" => "窗格操作",
        "Empty terminal" => "空终端",
        "CONNECTIONS" => "会话",
        "SESSIONS" => "会话",
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
        "Remote" => "主机",
        "Hosts" => "主机",
        "Host center" => "主机中心",
        "Connections, organization, and diagnostics" => "连接、组织与诊断",
        "Back to workspace" => "返回工作区",
        "Host filters" => "主机筛选",
        "All hosts" => "全部主机",
        "Favorites" => "收藏",
        "Favorite" => "收藏",
        "Favorited" => "已收藏",
        "Recent" => "最近使用",
        "Needs attention" => "需要处理",
        "Groups" => "分组",
        "Group" => "分组",
        "Tags" => "标签",
        "Sources" => "来源",
        "Select" => "多选",
        "Current" => "当前",
        "hosts" => "台主机",
        "sessions" => "个会话",
        "Adjust the search or choose another filter." => "调整搜索内容，或选择其他筛选条件。",
        "Choose a host to inspect its connection and health." => {
            "选择一台主机以查看连接配置和健康状态。"
        }
        "Organization" => "组织信息",
        "Connection" => "连接配置",
        "Ungrouped" => "未分组",
        "No tags" => "无标签",
        "Add to favorites" => "添加到收藏",
        "Remove from favorites" => "取消收藏",
        "OpenSSH remains the source of keys and trust." => "密钥与信任关系仍由 OpenSSH 管理。",
        "Open in Terminal" => "在终端中打开",
        "Test connection" => "测试连接",
        "Not checked" => "尚未检查",
        "Checking…" => "正在检查…",
        "Ready" => "已就绪",
        "Herdr not ready" => "Herdr 未就绪",
        "Herdr update required" => "需要更新 Herdr",
        "Authentication required" => "需要认证",
        "Host key needs attention" => "主机密钥需要处理",
        "Unreachable" => "无法访问",
        "Check failed" => "检查失败",
        "Run a check to verify SSH and Herdr." => "运行检查以验证 SSH 与 Herdr。",
        "SSH and Herdr are ready." => "SSH 与 Herdr 均已就绪。",
        "SSH works, but Herdr could not be found on this host." => {
            "SSH 可以连接，但在此主机上找不到 Herdr。"
        }
        "Update Herdr or configure a newer executable path." => {
            "请更新 Herdr，或配置较新的可执行文件路径。"
        }
        "Open Terminal to complete authentication, then check again." => {
            "请在终端中完成认证，然后重新检查。"
        }
        "Open Terminal to review and enroll this host key." => "请在终端中检查并登记此主机密钥。",
        "Check the alias, network, VPN, and SSH port." => "请检查别名、网络、VPN 与 SSH 端口。",
        "Review the SSH error, adjust the host, and try again." => {
            "请查看 SSH 错误，调整主机配置后重试。"
        }
        "Edit" => "编辑",
        "Edit host" => "编辑主机",
        "Keep current values" => "保留当前设置",
        "SSH config stays read-only; these are local overrides." => {
            "SSH 配置保持只读；此处只保存本地覆盖项。"
        }
        "Connection changes apply the next time you connect." => "连接设置将在下次连接时生效。",
        "Managed by ~/.ssh/config" => "由 ~/.ssh/config 管理",
        "Optional group" => "可选分组",
        "Comma-separated tags" => "用逗号分隔标签",
        "One group provides the primary location." => "每台主机可归入一个主要分组。",
        "Separate multiple tags with commas." => "多个标签请使用逗号分隔。",
        "Advanced connection overrides" => "高级连接覆盖项",
        "SSH agent still works when empty." => "留空时仍可使用 SSH 代理。",
        "Save & reconnect" => "保存并重新连接",
        "Choose hosts in the list, then apply a lightweight organization action." => {
            "在列表中选择主机，然后应用轻量组织操作。"
        }
        "Apply organization" => "应用分组与标签",
        "Remove local data…" => "移除本地数据…",
        "Remove local data" => "移除本地数据",
        "Remove local host data?" => "移除本地主机数据？",
        "Saved hosts will be removed. SSH config entries keep their OpenSSH definitions and lose only OcHerdr metadata and overrides." => {
            "已保存主机将被移除；SSH 配置项仍保留 OpenSSH 定义，仅清除 OcHerdr 元数据与覆盖项。"
        }
        "Enter a group or at least one tag." => "请输入分组或至少一个标签。",
        "Switch hosts before removing the active host." => "请先切换主机，再移除当前使用的主机。",
        "New host" => "新建主机",
        "Saved host" => "已保存的主机",
        "Select a host" => "选择一台主机",
        "In use" => "使用中",
        "Save" => "保存",
        "Save in OcHerdr" => "保存到 OcHerdr",
        "Advanced" => "高级选项",
        "Hide advanced" => "隐藏高级选项",
        "Hide" => "隐藏",
        "Show" => "显示",
        "SSH config" => "SSH 配置",
        "Saved" => "已保存",
        "Herdr on this computer" => "这台电脑上的 Herdr",
        "Read-only from ~/.ssh/config" => "只读，来自 ~/.ssh/config",
        "Name the machine OcHerdr should open Herdr on." => {
            "给 OcHerdr 要打开 Herdr 的那台机器起个名字。"
        }
        "Changes apply the next time you connect." => "下次连接时才会生效。",
        "Manage hosts" => "管理主机",
        "Manage hosts…" => "管理主机…",
        "Switch host" => "切换主机",
        "Switch host?" => "切换主机？",
        "Switch" => "切换",
        "this host" => "此主机",
        "This Mac cannot be edited." => "无法编辑这台 Mac。",
        "OcHerdr will leave the current Herdr session and attach to the other machine." => {
            "OcHerdr 会离开当前 Herdr 会话，并连接到另一台机器。"
        }
        "PREFIX" => "前缀键",
        "C new tab · ⇧N new workspace · S settings · 1–9 switch tab" => {
            "C 新建标签页 · ⇧N 新建工作区 · S 设置 · 1–9 切换标签页"
        }
        "Connection unavailable" => "连接不可用",
        "No Herdr session" => "没有 Herdr 会话",
        "Refresh" => "刷新",
        "No running Herdr session" => "没有正在运行的 Herdr 会话",
        "Start Herdr locally or open Remote in the top-right." => {
            "请在本机启动 Herdr，或打开右上角的主机列表。"
        }
        "This session has no tabs" => "此会话没有标签页",
        "Create a workspace to open the first terminal." => "新建工作区以打开第一个终端。",
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
        "Terminal type" => "终端字体",
        "Choose the font used by embedded terminals. Ghostty default is JetBrains Mono." => {
            "选择嵌入终端使用的字体。Ghostty 默认是 JetBrains Mono。"
        }
        "JetBrains Mono (Ghostty)" => "JetBrains Mono（Ghostty）",
        "Size" => "字号",
        "Point size used by the terminal grid." => "终端网格使用的点数大小。",
        "Ligatures" => "连字",
        "Programming ligatures such as => and !=." => "编程连字，例如 => 和 !=。",
        "On" => "开",
        "Off" => "关",
        "Thicken" => "加粗描边",
        "Draw a heavier stroke. macOS only." => "加粗笔画，仅 macOS 有效。",
        "Cell width" => "字宽",
        "Tighten or loosen the terminal cell width." => "收紧或放宽终端单元格宽度。",
        "Tight" => "紧凑",
        "Wide" => "宽松",
        "Cell height" => "行高",
        "Change the vertical space of each terminal row." => "调整每一行的垂直间距。",
        "Compact" => "压缩",
        "Relaxed" => "舒展",
        "Loose" => "更宽",
        "Search hosts" => "搜索主机",
        "Local" => "本机",
        "Default" => "默认",
        "CURRENT" => "这台 Mac",
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
        "Copy" => "复制",
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
        "The terminal input stream is no longer available." => "终端输入流已不可用。",
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

        assert_eq!(i18n.text("SESSIONS"), "会话");
        assert_eq!(i18n.text("Hosts"), "主机");
        assert_eq!(i18n.text("Empty terminal"), "空终端");
        assert_eq!(i18n.text("Status bar"), "状态栏");
        assert_eq!(i18n.text("Terminal type"), "终端字体");
        assert_eq!(i18n.text("Ligatures"), "连字");
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
}
