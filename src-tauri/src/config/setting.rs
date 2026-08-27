use super::constants::*;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter};
use tauri_plugin_store::StoreExt;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct Setting {
    pub installed: bool,
    pub port: u16,
    pub auto_start: bool,
    pub language: String,
    #[serde(default)]
    pub dsh_pkg_commit: Option<String>,
    /// 已安装 Harness 发行版对应的 GitHub release tag（与 dsh_pkg_commit 配套，
    /// 用于甄别“记录滞后于文件”与“同版本热修”两种不一致）
    #[serde(default)]
    pub dsh_pkg_tag: Option<String>,
    /// 命令行集成开关：安装后在用户 PATH 中注册 `dsh` 命令
    #[serde(default = "default_cli_link_enabled")]
    pub cli_link_enabled: bool,
    /// 预装插件引导是否已完成（确认安装或跳过都算完成，之后不再弹出）
    #[serde(default)]
    pub preinstall_done: bool,
    /// 上次引导结束时的 `preset-plugins.json` 内容指纹。资源文件每次安装都会被
    /// 强制覆盖、旧文件不复存在，只能把「上次看到的内容」记在这里，每次启动再比对：
    /// 内容有变更 → 重新进入预设引导。`None` = 老用户升级（无基线）→ 弹一次建立基线。
    #[serde(default)]
    pub preset_hash: Option<String>,
    /// 旧版 AppData `data/dsh` → 官方 `$DSH_HOME`（~/.dsh）数据迁移是否已完成。
    /// 幂等标记：迁移成功并删除旧目录后置位，避免重复合并。
    #[serde(default)]
    pub dsh_home_migrated: bool,
    /// 当前使用的档案 id（`$DSH_HOME/profiles/<id>`，默认 web）。
    /// 桌面端启动服务与插件管理都以它为准（见 service::profile）。
    #[serde(default = "default_active_profile")]
    pub active_profile: String,
    /// 活动核心的显式选择：`Some("local")` = 用户 CLI 安装的本地核心，
    /// `Some("app")` = 桌面端预打包核心；`None` = 自动（本地核心存在时优先）。
    #[serde(default)]
    pub active_core: Option<String>,
    /// 用户手动设置的服务端口（设置页「端口」输入，见 bridge::config）。
    /// 自动避让递增（配置端口被占 → 逐级顶高，见 workflow::launch）后，启动时
    /// 该端口空闲则回落回用户选择的值；`None` = 从未手动设置，回落目标为默认
    /// 端口（3080/3081）。避免端口只增不减、一路从 3080 漂到 3084+（issue #91）。
    #[serde(default)]
    pub manual_port: Option<u16>,
    /// npm 包源策略（插件安装走哪个 registry）：`auto` 按地域自动选择（大陆
    /// npmmirror、海外官方），`official` 固定官方 npmjs.org，`npmmirror` 固定
    /// 阿里镜像，`custom` 用 [`Setting::npm_registry_custom`] 自定义（含内网
    /// Verdaccio 私服）。`None` = 从未配置，等同于 `auto`。
    #[serde(default)]
    pub npm_registry_mode: Option<String>,
    /// `npm_registry_mode` 为 `custom` 时的 registry URL（如内网
    /// `http://package.onecode.cmict.cloud/repository/npm-group/`）。
    #[serde(default)]
    pub npm_registry_custom: Option<String>,
    /// GitHub 加速策略（git clone github: 插件 / GitHub Release 下载走哪个
    /// 中转）：`auto` 按地域自动选择（大陆 ghfast.top、海外直连），`none` 直连
    /// GitHub，`ghfast` 用 ghfast.top，`ghproxy` 用 ghproxy 通用中转，`custom`
    /// 用 [`Setting::gh_accel_custom`] 自定义前缀（含内网 git 镜像）。
    #[serde(default)]
    pub gh_accel_mode: Option<String>,
    /// `gh_accel_mode` 为 `custom` 时的 GitHub 中转前缀（如内网
    /// `http://git-mirror.corp/`，将被拼为 `http://git-mirror.corp/https://github.com/`）。
    #[serde(default)]
    pub gh_accel_custom: Option<String>,
}

/// 默认档案：桌面端内置的 web 档案
fn default_active_profile() -> String {
    "web".to_string()
}

/// 命令行集成默认开启（开发者工具场景，安装完成即可用）
fn default_cli_link_enabled() -> bool {
    true
}

/// 默认服务端口：debug 构建与生产隔离，避免开发时与已运行的桌面端争用 3080。
pub fn default_port() -> u16 {
    if cfg!(debug_assertions) {
        DSH_DEV_PORT
    } else {
        DSH_PORT
    }
}

impl Default for Setting {
    fn default() -> Self {
        Self {
            installed: false,
            port: default_port(),
            auto_start: true,
            language: "zh-CN".to_string(),
            dsh_pkg_commit: None,
            dsh_pkg_tag: None,
            cli_link_enabled: default_cli_link_enabled(),
            preinstall_done: false,
            preset_hash: None,
            dsh_home_migrated: false,
            active_profile: default_active_profile(),
            active_core: None,
            manual_port: None,
            npm_registry_mode: None,
            npm_registry_custom: None,
            gh_accel_mode: None,
            gh_accel_custom: None,
        }
    }
}

/// Store 持久化文件名：debug 构建与生产隔离（各自独立文件）。
///
/// store（端口、installed、active_core 等）属于「应用数据」而非共用核心——
/// 生产默认 3080、开发默认 3081，共用一份 store 会让两边端口一路漂移
/// （release 读到开发写入的 3081 后把 3080 让出，开发下次又从 3081 漂走）
/// 并相互污染安装/核心等状态。
fn store_dat_file_name() -> &'static str {
    if cfg!(debug_assertions) {
        STORE_DAT_DEV_FILE
    } else {
        STORE_DAT_FILE
    }
}

pub fn set_store_dat_setting(app_handle: &AppHandle, setting: Setting) {
    let store = app_handle
        .store(store_dat_file_name())
        .expect("Failed to load store");
    store.set(STORE_SETTING_KEY, serde_json::to_value(&setting).unwrap());
    store.save().expect("Failed to save store");
    app_handle
        .emit("setting_updated", &serde_json::to_value(&setting).unwrap())
        .expect("Failed to emit event");
}

pub fn get_store_dat_setting(app_handle: &AppHandle) -> Setting {
    let store = app_handle
        .store(store_dat_file_name())
        .expect("Failed to load store");
    let raw = store.get(STORE_SETTING_KEY);
    let value = raw.as_ref().and_then(|v| {
        v.as_str()
            .and_then(|s| serde_json::from_str(s).ok())
            .or_else(|| Some(v.clone()))
    });
    value
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_else(Setting::default)
}

/// 已安装 Harness 发行版对应的 GitHub release commit hash
pub fn get_dsh_pkg_commit(app_handle: &AppHandle) -> Option<String> {
    get_store_dat_setting(app_handle).dsh_pkg_commit
}

/// 记录已安装 Harness 发行版的 GitHub release commit hash
pub fn set_dsh_pkg_commit(app_handle: &AppHandle, commit: String) {
    let mut setting = get_store_dat_setting(app_handle);
    setting.dsh_pkg_commit = Some(commit);
    set_store_dat_setting(app_handle, setting);
}

/// 已安装 Harness 发行版对应的 GitHub release tag
pub fn get_dsh_pkg_tag(app_handle: &AppHandle) -> Option<String> {
    get_store_dat_setting(app_handle).dsh_pkg_tag
}

/// 记录已安装 Harness 发行版的 GitHub release tag
pub fn set_dsh_pkg_tag(app_handle: &AppHandle, tag: String) {
    let mut setting = get_store_dat_setting(app_handle);
    setting.dsh_pkg_tag = Some(tag);
    set_store_dat_setting(app_handle, setting);
}
