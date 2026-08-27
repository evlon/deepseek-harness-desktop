//! 插件安装链路的网络加速配置解析：npm registry 源与 GitHub 中转前缀。
//!
//! 与 [`super::region`] 的区别：region 只决定 Node/pnpm/dsh 内核下载的地域镜像；
//! 本模块处理的是**插件安装**（`dsh plugin add` → pnpm / git）走的包源，用户可在
//! 「下载插件」界面选择 `auto` / 官方 / npmmirror / 治理源 / 自定义，以及 GitHub 中转策略。
//!
//! `auto` 策略复用 [`super::detect_region`]：大陆走 npmmirror + ghfast.top，
//! 海外直连官方源，与内核下载保持一致的开箱即用体验。
//!
//! `governance`（治理源）模式：**不硬编码**地址，URL 来自 `desktop-config.json`
//! 的 `npmRegistry`（内网 Verdaccio）；未配置时回落 `auto`。该模式下从内网私服
//! 拉取插件包，依赖树的初次同步由 `scripts/sync-to-verdaccio.mjs` 一次性完成。

use super::setting::Setting;
use super::region::{detect_region, Region};
use super::constants::DSH_MIRROR_PREFIX;
use serde::Serialize;

/// npm registry 策略值
pub mod npm_mode {
    /// 按地域自动选择（大陆 npmmirror、海外官方）
    pub const AUTO: &str = "auto";
    /// 固定官方 npmjs.org
    pub const OFFICIAL: &str = "official";
    /// 固定阿里 npmmirror
    pub const NPM_MIRROR: &str = "npmmirror";
    /// 自定义 URL（含内网 Verdaccio）
    pub const CUSTOM: &str = "custom";
    /// 治理源（内网 Verdaccio）：URL 来自 `desktop-config.json` 的 `npmRegistry`，
    /// 未配置时回落 `auto`；同时触发已装依赖树同步到该私服
    pub const GOVERNANCE: &str = "governance";
}

/// GitHub 加速策略值
pub mod gh_mode {
    /// 按地域自动选择（大陆 ghfast.top、海外直连）
    pub const AUTO: &str = "auto";
    /// 直连 GitHub（不中转）
    pub const NONE: &str = "none";
    /// ghfast.top 中转
    pub const GHFAST: &str = "ghfast";
    /// ghproxy 通用中转（`https://ghproxy.com/`）
    pub const GHPROXY: &str = "ghproxy";
    /// 自定义前缀（含内网 git 镜像）
    pub const CUSTOM: &str = "custom";
}

/// 阿里 npmmirror registry（npm 镜像，全国通用）
pub const NPM_MIRROR_REGISTRY: &str = "https://registry.npmmirror.com/";
/// ghproxy 通用中转前缀（透传 GitHub URL）
pub const GHPROXY_PREFIX: &str = "https://ghproxy.com/";

/// 解析当前 npm registry URL（供插件安装写入 `.npmrc` / 注入 env 用）。
///
/// 返回末尾带 `/` 的完整 registry URL。`auto` 时按地域选择；用户显式选
/// `official` / `npmmirror` / `custom` 则以其为准；未配置（`None`）等同 `auto`。
pub fn npm_registry_url(setting: &Setting) -> String {
    match setting.npm_registry_mode.as_deref() {
        Some(npm_mode::OFFICIAL) => OFFICIAL_NPM_REGISTRY.to_string(),
        Some(npm_mode::NPM_MIRROR) => NPM_MIRROR_REGISTRY.to_string(),
        Some(npm_mode::CUSTOM) => {
            let url = setting
                .npm_registry_custom
                .clone()
                .unwrap_or_default()
                .trim()
                .to_string();
            if url.is_empty() {
                // 自定义但未填 URL：回落自动，避免写出空 registry 破坏安装
                auto_npm_registry()
            } else {
                ensure_trailing_slash(url)
            }
        }
        // 治理源：URL 取 desktop-config.json 的 npmRegistry（内网 Verdaccio）；
        // 未配置则回落 auto，避免用户选了治理源却没填地址时空跑。
        Some(npm_mode::GOVERNANCE) => match super::external::npm_registry_override() {
            Some(u) if !u.trim().is_empty() => ensure_trailing_slash(u.trim().to_string()),
            _ => auto_npm_registry(),
        },
        // auto / 未配置
        _ => auto_npm_registry(),
    }
}

/// `auto` 策略的 npm registry：`desktop-config.json` 显式覆盖优先（如内网
/// Verdaccio），否则大陆 npmmirror、海外官方。
fn auto_npm_registry() -> String {
    if let Some(url) = super::external::npm_registry_override() {
        return ensure_trailing_slash(url);
    }
    match detect_region() {
        Region::Domestic => NPM_MIRROR_REGISTRY.to_string(),
        Region::Overseas => OFFICIAL_NPM_REGISTRY.to_string(),
    }
}

/// 官方 npm registry
const OFFICIAL_NPM_REGISTRY: &str = "https://registry.npmjs.org/";

/// 解析当前 GitHub 中转前缀。返回 `None` 表示直连（不中转）。
///
/// 中转前缀形如 `https://ghfast.top/`，实际重写 `https://github.com/` →
/// `{prefix}https://github.com/`。`auto` 时大陆 ghfast.top、海外 None（直连）。
pub fn gh_mirror_prefix(setting: &Setting) -> Option<String> {
    match setting.gh_accel_mode.as_deref() {
        Some(gh_mode::NONE) => None,
        Some(gh_mode::GHFAST) => Some(ghfast_prefix()),
        Some(gh_mode::GHPROXY) => Some(ghproxy_prefix()),
        Some(gh_mode::CUSTOM) => {
            let prefix = setting
                .gh_accel_custom
                .clone()
                .unwrap_or_default()
                .trim()
                .to_string();
            if prefix.is_empty() {
                // 自定义但未填前缀：回落自动
                auto_gh_prefix()
            } else {
                Some(ensure_trailing_slash(prefix))
            }
        }
        // auto / 未配置
        _ => auto_gh_prefix(),
    }
}

/// `auto` 策略的 GitHub 中转前缀：`desktop-config.json` 显式覆盖优先（如内网
/// git 镜像），否则大陆 ghfast.top、海外直连（None）。
fn auto_gh_prefix() -> Option<String> {
    if let Some(prefix) = super::external::gh_mirror_prefix_override() {
        return Some(ensure_trailing_slash(prefix));
    }
    match detect_region() {
        Region::Domestic => Some(ghfast_prefix()),
        Region::Overseas => None,
    }
}

/// ghfast.top 中转前缀：`desktop-config.json` 的 `ghfastPrefix` 可覆盖。
fn ghfast_prefix() -> String {
    super::external::ghfast_prefix_override()
        .map(ensure_trailing_slash)
        .unwrap_or_else(|| DSH_MIRROR_PREFIX.to_string())
}

/// ghproxy 通用中转前缀：`desktop-config.json` 的 `ghproxyPrefix` 可覆盖。
fn ghproxy_prefix() -> String {
    super::external::ghproxy_prefix_override()
        .map(ensure_trailing_slash)
        .unwrap_or_else(|| GHPROXY_PREFIX.to_string())
}

/// 保证 URL / 前缀以 `/` 结尾（避免拼出 `https://registry.npmmirror.comhttps://...`）。
fn ensure_trailing_slash(s: String) -> String {
    if s.ends_with('/') {
        s
    } else {
        format!("{s}/")
    }
}

/// 前端可读的加速配置视图：暴露当前选择的模式 + 自定义值 + 实际生效的
/// registry URL / GitHub 中转前缀（`auto` 下反映按地域解析后的落地值）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccelConfig {
    /// npm registry 模式（`auto`/`official`/`npmmirror`/`custom`）
    pub npm_registry_mode: String,
    /// 自定义 registry URL（内网私服等）；非 custom 模式可为空
    pub npm_registry_custom: String,
    /// 实际生效的 npm registry URL（末尾带 `/`）
    pub npm_registry_url: String,
    /// GitHub 加速模式（`auto`/`none`/`ghfast`/`ghproxy`/`custom`）
    pub gh_accel_mode: String,
    /// 自定义 GitHub 中转前缀；非 custom 模式可为空
    pub gh_accel_custom: String,
    /// 实际生效的 GitHub 中转前缀；`none`/海外 `auto` 为空串（直连）
    pub gh_accel_prefix: String,
}

impl AccelConfig {
    /// 从持久化设置解析出可展示/生效配置（`None` 模式归一化为 `auto`）。
    pub fn from_setting(setting: &Setting) -> Self {
        let resolved_npm_mode = setting
            .npm_registry_mode
            .clone()
            .unwrap_or_else(|| npm_mode::AUTO.to_string());
        let resolved_gh_mode = setting
            .gh_accel_mode
            .clone()
            .unwrap_or_else(|| gh_mode::AUTO.to_string());
        Self {
            npm_registry_url: npm_registry_url(setting),
            npm_registry_custom: setting.npm_registry_custom.clone().unwrap_or_default(),
            npm_registry_mode: resolved_npm_mode,
            gh_accel_prefix: gh_mirror_prefix(setting).unwrap_or_default(),
            gh_accel_custom: setting.gh_accel_custom.clone().unwrap_or_default(),
            gh_accel_mode: resolved_gh_mode,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn npm_setting(mode: Option<&str>, custom: Option<&str>) -> Setting {
        let mut s = Setting::default();
        s.npm_registry_mode = mode.map(str::to_string);
        s.npm_registry_custom = custom.map(str::to_string);
        s
    }

    fn gh_setting(mode: Option<&str>, custom: Option<&str>) -> Setting {
        let mut s = Setting::default();
        s.gh_accel_mode = mode.map(str::to_string);
        s.gh_accel_custom = custom.map(str::to_string);
        s
    }

    #[test]
    fn official_uses_npmjs_registry() {
        let s = npm_setting(Some(npm_mode::OFFICIAL), None);
        assert_eq!(npm_registry_url(&s), "https://registry.npmjs.org/");
    }

    #[test]
    fn npmmirror_uses_ali_registry() {
        let s = npm_setting(Some(npm_mode::NPM_MIRROR), None);
        assert_eq!(npm_registry_url(&s), "https://registry.npmmirror.com/");
    }

    #[test]
    fn custom_appends_trailing_slash() {
        let s = npm_setting(
            Some(npm_mode::CUSTOM),
            Some("http://package.onecode.cmict.cloud/repository/npm-group"),
        );
        assert_eq!(
            npm_registry_url(&s),
            "http://package.onecode.cmict.cloud/repository/npm-group/"
        );
    }

    #[test]
    fn custom_empty_falls_back_to_auto() {
        let s = npm_setting(Some(npm_mode::CUSTOM), Some("   "));
        // auto 按地域；此处无法断言具体值，只保证非空且不是空串
        assert!(!npm_registry_url(&s).is_empty());
    }

    #[test]
    fn governance_uses_desktop_config_override() {
        // 治理源模式读取 desktop-config.json 的 npmRegistry 作为实际 URL
        // （注意：external::LOADED 是 OnceLock，首个 _set_for_test 生效，本测试
        // 依赖此顺序；回落 auto 的路径与 custom 空 URL 等价，已由
        // custom_empty_falls_back_to_auto 覆盖）
        crate::config::external::_set_for_test(crate::config::ExternalConfig {
            npm_registry: Some("https://registry.ict.cmcc/".to_string()),
            ..Default::default()
        });
        let s = npm_setting(Some(npm_mode::GOVERNANCE), None);
        assert_eq!(
            npm_registry_url(&s),
            "https://registry.ict.cmcc/"
        );
    }

    #[test]
    fn gh_none_returns_none() {
        let s = gh_setting(Some(gh_mode::NONE), None);
        assert_eq!(gh_mirror_prefix(&s), None);
    }

    #[test]
    fn ghfast_returns_prefix() {
        let s = gh_setting(Some(gh_mode::GHFAST), None);
        assert_eq!(gh_mirror_prefix(&s), Some(DSH_MIRROR_PREFIX.to_string()));
    }

    #[test]
    fn ghproxy_returns_prefix() {
        let s = gh_setting(Some(gh_mode::GHPROXY), None);
        assert_eq!(gh_mirror_prefix(&s), Some(GHPROXY_PREFIX.to_string()));
    }

    #[test]
    fn gh_custom_ensures_slash() {
        let s = gh_setting(Some(gh_mode::CUSTOM), Some("http://git-mirror.corp"));
        assert_eq!(gh_mirror_prefix(&s), Some("http://git-mirror.corp/".to_string()));
    }
}
