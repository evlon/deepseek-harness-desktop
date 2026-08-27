//! 用户可编辑的部署配置：`$DSH_HOME/desktop-config.json`。
//!
//! 用于**不改代码**地覆盖默认下载/镜像/包源地址（公司内网迁移、镜像切换等）。
//! 所有字段可选：缺省时回落到编译期默认（见 [`super::constants`]）。
//!
//! 读取时机：应用启动时调用 [`set_external_config`]（传入 `AppHandle` 解析路径）
//! 一次性读入进程级全局并缓存；后续 URL 解析通过 [`node_mirror_base_override`] 等
//! **纯 getter** 读取，无需在每一处下载/更新调用点传递 `AppHandle`。文件缺失 /
//! 解析失败一律静默按空配置处理，绝不因配置损坏阻断启动。改动需重启应用生效。
//!
//! 读取位置与 `$DSH_HOME` 一致（用户主目录旁、`DSH_HOME` 环境变量优先），既满足
//! 「用户可直接编辑、升级不覆盖」，又与现有数据隔离（debug 用 `~/.dsh.dev`）。

use serde::Deserialize;
use std::fs;
use std::path::PathBuf;
use std::sync::OnceLock;
use tauri::{AppHandle, Runtime};

/// 外部配置文件相对 `$DSH_HOME` 的文件名
const CONFIG_FILE_NAME: &str = "desktop-config.json";

/// 用户可编辑的部署配置（JSON）。字段全可选，`None` = 用编译期默认。
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ExternalConfig {
    /// 覆盖插件安装的 npm registry（`auto` 模式优先用它，如内网 Verdaccio）
    pub npm_registry: Option<String>,
    /// 覆盖 GitHub 中转前缀（`auto` 模式优先用它，如内网 git 镜像）
    pub gh_mirror_prefix: Option<String>,
    /// 覆盖 Node 镜像下载前缀（缺省按地域选 npmmirror / nodejs.org）
    pub node_mirror_base: Option<String>,
    /// 覆盖 pnpm 镜像下载前缀
    pub pnpm_mirror_base: Option<String>,
    /// 覆盖打包 Harness 发行版直连前缀（GitHub Release）
    pub dsh_core_url: Option<String>,
    /// 覆盖打包 Harness 发行版镜像前缀
    pub dsh_mirror_core_url: Option<String>,
    /// 覆盖 GitHub Release 通用中转前缀（ghfast.top 缺省）
    pub ghfast_prefix: Option<String>,
    /// 覆盖 ghproxy 通用中转前缀
    pub ghproxy_prefix: Option<String>,
}

/// 启动时一次性读入的进程级配置（`None` 表示未初始化，视为空配置）。
static LOADED: OnceLock<ExternalConfig> = OnceLock::new();

/// 应用启动时调用：解析 `$DSH_HOME/desktop-config.json` 并缓存。可重复调用，
/// 首次加载生效（幂等）。文件缺失/解析失败按空配置处理，不阻断启动。
pub fn set_external_config<R: Runtime>(app_handle: &AppHandle<R>) {
    let cfg = ExternalConfig::load_from(&config_path(app_handle));
    log::info!(
        "External desktop-config loaded: npm_registry={:?}, gh_prefix={:?}, node_mirror={:?}",
        cfg.npm_registry,
        cfg.gh_mirror_prefix,
        cfg.node_mirror_base
    );
    let _ = LOADED.set(cfg);
}

/// 读取进程级外部配置（未初始化时按空处理，便于单测与启动前的防御性调用）。
fn loaded() -> &'static ExternalConfig {
    LOADED.get().unwrap_or(&EMPTY)
}

static EMPTY: ExternalConfig = ExternalConfig {
    npm_registry: None,
    gh_mirror_prefix: None,
    node_mirror_base: None,
    pnpm_mirror_base: None,
    dsh_core_url: None,
    dsh_mirror_core_url: None,
    ghfast_prefix: None,
    ghproxy_prefix: None,
};

impl ExternalConfig {
    fn load_from(path: &PathBuf) -> Self {
        let raw = match fs::read_to_string(path) {
            Ok(s) => s,
            Err(_) => return Self::default(),
        };
        serde_json::from_str(&raw).unwrap_or_else(|e| {
            log::warn!("desktop-config.json 解析失败，回退默认：{e}");
            Self::default()
        })
    }
}

/// 外部配置文件路径：`$DSH_HOME/desktop-config.json`（与数据目录一致，debug 隔离）。
fn config_path<R: Runtime>(app_handle: &AppHandle<R>) -> PathBuf {
    super::get_dsh_data_path(app_handle).join(CONFIG_FILE_NAME)
}

/// npm registry 外部覆盖（`auto` 模式优先用，缺省 None）
pub fn npm_registry_override() -> Option<String> {
    loaded().npm_registry.clone()
}

/// GitHub 中转前缀外部覆盖（`auto` 模式优先用，缺省 None）
pub fn gh_mirror_prefix_override() -> Option<String> {
    loaded().gh_mirror_prefix.clone()
}

/// Node 镜像下载前缀外部覆盖
pub fn node_mirror_base_override() -> Option<String> {
    loaded().node_mirror_base.clone()
}

/// pnpm 镜像下载前缀外部覆盖
pub fn pnpm_mirror_base_override() -> Option<String> {
    loaded().pnpm_mirror_base.clone()
}

/// 打包 Harness 发行版直连前缀外部覆盖
pub fn dsh_core_url_override() -> Option<String> {
    loaded().dsh_core_url.clone()
}

/// 打包 Harness 发行版镜像前缀外部覆盖
pub fn dsh_mirror_core_url_override() -> Option<String> {
    loaded().dsh_mirror_core_url.clone()
}

/// GitHub Release 通用中转前缀（ghfast.top）外部覆盖
pub fn ghfast_prefix_override() -> Option<String> {
    loaded().ghfast_prefix.clone()
}

/// ghproxy 通用中转前缀外部覆盖
pub fn ghproxy_prefix_override() -> Option<String> {
    loaded().ghproxy_prefix.clone()
}

/// 测试用：把指定配置写入进程级（供各模块单测验证覆盖解析）。
#[cfg(test)]
pub fn _set_for_test(cfg: ExternalConfig) {
    let _ = LOADED.set(cfg);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn tmp_config(label: &str, json: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("dsh-extcfg-{}-{label}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(CONFIG_FILE_NAME);
        std::fs::write(&path, json).unwrap();
        path
    }

    #[test]
    fn parses_camel_case_fields() {
        let p = tmp_config(
            "parse",
            r#"{"npmRegistry":"http://inner:4873/","ghMirrorPrefix":"http://git-mirror/","dshCoreUrl":"http://cdn/releases/"}"#,
        );
        let cfg = ExternalConfig::load_from(&p);
        assert_eq!(cfg.npm_registry.as_deref(), Some("http://inner:4873/"));
        assert_eq!(cfg.gh_mirror_prefix.as_deref(), Some("http://git-mirror/"));
        assert_eq!(cfg.dsh_core_url.as_deref(), Some("http://cdn/releases/"));
    }

    #[test]
    fn invalid_json_falls_back_to_default() {
        let p = tmp_config("bad", "{not json");
        let cfg = ExternalConfig::load_from(&p);
        assert_eq!(cfg.npm_registry, None);
    }

    #[test]
    fn missing_file_is_empty_config() {
        let p = PathBuf::from(std::env::temp_dir()).join(format!("dsh-extcfg-missing-{}", std::process::id()));
        let cfg = ExternalConfig::load_from(&p.join(CONFIG_FILE_NAME));
        assert_eq!(cfg.npm_registry, None);
    }
}
