//! Installed Chrome extensions: Web Store download, consent, the per-profile
//! registry on disk, and handing enabled extensions to the shell.
//!
//! Layout under the app data directory:
//! `webext/packages/<id>/<version>.crx` keeps the signed original (or
//! `.zip`, or a `.src/` folder, for an extension installed from a file), and
//! `webext/packages/<id>/<version>-<compat>/` is the tree WebKit loads,
//! rebuilt from the original whenever the compatibility layer changes.
//! `webext/profiles/<profile>.json` records what a profile installed.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use tauri::{Manager, State, WebviewWindow};
use zephium_app::Handle;
use zephium_core::ids::ProfileId;

use crate::{authorize, shutdown_started, CallerPolicy};

pub(crate) struct WebExtensions {
    #[cfg_attr(not(any(target_os = "macos", target_os = "windows")), allow(dead_code))]
    root: PathBuf,
    #[cfg_attr(not(any(target_os = "macos", target_os = "windows")), allow(dead_code))]
    state: Mutex<Option<Pending>>,
}

#[cfg_attr(not(any(target_os = "macos", target_os = "windows")), allow(dead_code))]
struct Pending {
    profile: ProfileId,
    entry: Entry,
    staged: PathBuf,
    original: Original,
}

/// What a package was installed from, kept to rebuild it later.
#[cfg_attr(not(any(target_os = "macos", target_os = "windows")), allow(dead_code))]
enum Original {
    Crx(Vec<u8>),
    Zip(Vec<u8>),
    Folder(PathBuf),
}

/// Which sites an extension may read and change.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "lowercase")]
enum Access {
    /// Every site it asked for.
    #[default]
    All,
    /// Only a tab where the user clicked its button.
    Click,
    /// Only these sites, among those it asked for.
    Sites { sites: Vec<String> },
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct Registry {
    extensions: Vec<Entry>,
}

#[cfg_attr(not(any(target_os = "macos", target_os = "windows")), allow(dead_code))]
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Entry {
    install: String,
    id: String,
    name: String,
    version: String,
    description: String,
    enabled: bool,
    /// Directory under `packages/<id>/` WebKit loads.
    package: String,
    /// Compatibility layer the package was prepared with.
    compat: String,
    /// Compatibility layer WebKit last started the background with.
    #[serde(default)]
    started: String,
    permissions: Vec<String>,
    hosts: Vec<String>,
    #[serde(default)]
    icon: Option<String>,
    /// A newer version held back because it asks for more access.
    #[serde(default)]
    held_update: Option<String>,
    #[serde(default)]
    access: Access,
    /// Installed from a file rather than the Web Store; never updated from
    /// the store, so a developer's own build stays as it is.
    #[serde(default)]
    sideloaded: bool,
}

#[derive(Clone, Debug, Serialize, specta::Type)]
pub(crate) struct WebExtensionReview {
    id: String,
    name: String,
    version: String,
    description: String,
    warnings: Vec<String>,
    icon: Option<String>,
    update: bool,
    /// Installed from a file, so not checked against the Web Store.
    from_file: bool,
}

#[derive(Clone, Debug, Serialize, specta::Type)]
pub(crate) struct WebExtensionView {
    id: String,
    name: String,
    version: String,
    description: String,
    enabled: bool,
    icon: Option<String>,
    /// `running`, `starting`, `failed` or `off`.
    state: String,
    error: Option<String>,
    warnings: Vec<String>,
    /// `all`, `click` or `sites`; meaningful only when `site_scoped`.
    access: String,
    sites: Vec<String>,
    /// Whether the extension asked for access to websites at all.
    site_scoped: bool,
    has_options: bool,
    held_update: Option<String>,
    sideloaded: bool,
}

impl WebExtensions {
    pub(crate) fn new(data_dir: &Path) -> Self {
        Self {
            root: data_dir.join("webext"),
            state: Mutex::new(None),
        }
    }

    /// How many are enabled and how many are installed, for a bug report.
    pub(crate) fn counts(&self) -> (usize, usize) {
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        return imp::counts(self);
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        (0, 0)
    }

    /// Restores every profile's extensions at launch.
    pub(crate) fn restore(&self, shell: &Handle) {
        if let Some(data_dir) = self.root.parent() {
            remove_previous_repository(data_dir);
        }
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        {
            imp::restore(self, shell);
            imp::start_updates(&self.root, shell);
        }
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        let _ = shell;
    }
}

/// Removes the package repository left by the previous extension stack, in
/// the background and best effort. It sealed its directories read-only, so
/// they are made writable first.
fn remove_previous_repository(data_dir: &Path) {
    let path = data_dir.join("extension-repository-v1");
    if !path.exists() {
        return;
    }
    let spawned = std::thread::Builder::new()
        .name("zephium-legacy-extension-cleanup".into())
        .spawn(move || {
            unseal(&path);
            if let Err(error) = std::fs::remove_dir_all(&path) {
                eprintln!("extensions: could not remove the previous repository: {error}");
            }
        });
    if let Err(error) = spawned {
        eprintln!("extensions: could not start the previous repository cleanup: {error}");
    }
}

fn unseal(path: &Path) {
    let Ok(metadata) = std::fs::symlink_metadata(path) else {
        return;
    };
    if metadata.file_type().is_symlink() {
        return;
    }
    #[cfg(unix)]
    let permissions = {
        use std::os::unix::fs::PermissionsExt;
        std::fs::Permissions::from_mode(if metadata.is_dir() { 0o700 } else { 0o600 })
    };
    #[cfg(not(unix))]
    let permissions = {
        let mut permissions = metadata.permissions();
        // Windows only clears the read-only attribute.
        #[allow(clippy::permissions_set_readonly_false)]
        permissions.set_readonly(false);
        permissions
    };
    let _ = std::fs::set_permissions(path, permissions);
    if metadata.is_dir() {
        if let Ok(entries) = std::fs::read_dir(path) {
            for entry in entries.flatten() {
                unseal(&entry.path());
            }
        }
    }
}

const UNAVAILABLE: &str = "Extensions are unavailable in this build.";

/// A recommended extension's icon as a data URL, or `None` while the store
/// can't be reached.
#[tauri::command]
#[specta::specta]
pub(crate) async fn web_extension_catalog_icon(
    caller: WebviewWindow,
    extensions: State<'_, WebExtensions>,
    token: String,
) -> Result<Option<String>, String> {
    if !authorize(&caller, CallerPolicy::Main, "web_extension_catalog_icon") {
        return Err(UNAVAILABLE.into());
    }
    Ok(store_icon::icon(&extensions.root, &token).await)
}

/// Downloads and verifies the extension on the active store page, and
/// returns what the user is asked to approve.
#[tauri::command]
#[specta::specta]
pub(crate) async fn web_extension_prepare(
    caller: WebviewWindow,
    shell: State<'_, Handle>,
    extensions: State<'_, WebExtensions>,
    tab_id: String,
) -> Result<WebExtensionReview, String> {
    if !authorize(&caller, CallerPolicy::Main, "web_extension_prepare")
        || shutdown_started(caller.app_handle())
    {
        return Err(UNAVAILABLE.into());
    }
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    return imp::prepare(&shell, &extensions, &tab_id).await;
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = (shell, extensions, tab_id);
        Err(UNAVAILABLE.into())
    }
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn web_extension_confirm(
    caller: WebviewWindow,
    shell: State<'_, Handle>,
    extensions: State<'_, WebExtensions>,
    id: String,
) -> Result<(), String> {
    if !authorize(&caller, CallerPolicy::Main, "web_extension_confirm")
        || shutdown_started(caller.app_handle())
    {
        return Err(UNAVAILABLE.into());
    }
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    return imp::confirm(&shell, &extensions, &id);
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = (shell, extensions, id);
        Err(UNAVAILABLE.into())
    }
}

#[tauri::command]
#[specta::specta]
pub(crate) fn web_extension_cancel(caller: WebviewWindow, extensions: State<'_, WebExtensions>) {
    if !authorize(&caller, CallerPolicy::Main, "web_extension_cancel") {
        return;
    }
    if let Ok(mut pending) = extensions.state.lock() {
        if let Some(pending) = pending.take() {
            let _ = std::fs::remove_dir_all(pending.staged);
        }
    }
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn web_extension_list(
    caller: WebviewWindow,
    shell: State<'_, Handle>,
    extensions: State<'_, WebExtensions>,
) -> Result<Vec<WebExtensionView>, String> {
    if !authorize(&caller, CallerPolicy::Main, "web_extension_list") {
        return Err(UNAVAILABLE.into());
    }
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    return imp::list(&shell, &extensions).await;
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = (shell, extensions);
        Err(UNAVAILABLE.into())
    }
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn web_extension_set_enabled(
    caller: WebviewWindow,
    shell: State<'_, Handle>,
    extensions: State<'_, WebExtensions>,
    id: String,
    enabled: bool,
) -> Result<(), String> {
    if !authorize(&caller, CallerPolicy::Main, "web_extension_set_enabled") {
        return Err(UNAVAILABLE.into());
    }
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    return imp::set_enabled(&shell, &extensions, &id, enabled).await;
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = (shell, extensions, id, enabled);
        Err(UNAVAILABLE.into())
    }
}

/// Answers an extension's run-time request for access; granted access is
/// kept for the next launch.
#[tauri::command]
#[specta::specta]
pub(crate) fn web_extension_answer_access(
    caller: WebviewWindow,
    shell: State<'_, Handle>,
    extensions: State<'_, WebExtensions>,
    request: zephium_ipc::WebExtensionAccessRequestView,
    allowed: bool,
) -> Result<(), String> {
    if !authorize(&caller, CallerPolicy::Main, "web_extension_answer_access") {
        return Err(UNAVAILABLE.into());
    }
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    return imp::answer_access(&shell, &extensions, request, allowed);
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = (shell, extensions, request, allowed);
        Err(UNAVAILABLE.into())
    }
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn web_extension_uninstall(
    caller: WebviewWindow,
    shell: State<'_, Handle>,
    extensions: State<'_, WebExtensions>,
    id: String,
) -> Result<(), String> {
    if !authorize(&caller, CallerPolicy::Main, "web_extension_uninstall") {
        return Err(UNAVAILABLE.into());
    }
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    return imp::uninstall(&shell, &extensions, &id).await;
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = (shell, extensions, id);
        Err(UNAVAILABLE.into())
    }
}

/// Chooses which sites an extension may use: `all` it asked for, only on
/// `click`, or the listed `sites`.
#[tauri::command]
#[specta::specta]
pub(crate) async fn web_extension_set_access(
    caller: WebviewWindow,
    shell: State<'_, Handle>,
    extensions: State<'_, WebExtensions>,
    id: String,
    mode: String,
    sites: Vec<String>,
) -> Result<(), String> {
    if !authorize(&caller, CallerPolicy::Main, "web_extension_set_access") {
        return Err(UNAVAILABLE.into());
    }
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    return imp::set_access(&shell, &extensions, &id, &mode, sites).await;
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = (shell, extensions, id, mode, sites);
        Err(UNAVAILABLE.into())
    }
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn web_extension_open_options(
    caller: WebviewWindow,
    shell: State<'_, Handle>,
    id: String,
) -> Result<(), String> {
    if !authorize(&caller, CallerPolicy::Main, "web_extension_open_options") {
        return Err(UNAVAILABLE.into());
    }
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    return imp::open_options(&shell, &id).await;
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = (shell, id);
        Err(UNAVAILABLE.into())
    }
}

/// Lets the user pick a `.crx` or `.zip` file, or with `folder` an unpacked
/// extension, and returns what they are asked to approve; `None` when they
/// cancel the picker.
#[tauri::command]
#[specta::specta]
pub(crate) async fn web_extension_choose_file(
    caller: WebviewWindow,
    shell: State<'_, Handle>,
    extensions: State<'_, WebExtensions>,
    folder: bool,
) -> Result<Option<WebExtensionReview>, String> {
    if !authorize(&caller, CallerPolicy::Main, "web_extension_choose_file")
        || shutdown_started(caller.app_handle())
    {
        return Err(UNAVAILABLE.into());
    }
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    return imp::choose_file(caller.app_handle(), &shell, &extensions, folder).await;
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = (shell, extensions, folder);
        Err(UNAVAILABLE.into())
    }
}

/// Reviews an extension file dropped on the browser.
#[tauri::command]
#[specta::specta]
pub(crate) async fn web_extension_prepare_file(
    caller: WebviewWindow,
    shell: State<'_, Handle>,
    extensions: State<'_, WebExtensions>,
    path: String,
) -> Result<WebExtensionReview, String> {
    if !authorize(&caller, CallerPolicy::Main, "web_extension_prepare_file")
        || shutdown_started(caller.app_handle())
    {
        return Err(UNAVAILABLE.into());
    }
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    return imp::prepare_file(&shell, &extensions, PathBuf::from(path)).await;
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = (shell, extensions, path);
        Err(UNAVAILABLE.into())
    }
}

/// Reviews the newest store version of an extension whose update waits for
/// the user's approval.
#[tauri::command]
#[specta::specta]
pub(crate) async fn web_extension_review_update(
    caller: WebviewWindow,
    shell: State<'_, Handle>,
    extensions: State<'_, WebExtensions>,
    id: String,
) -> Result<WebExtensionReview, String> {
    if !authorize(&caller, CallerPolicy::Main, "web_extension_review_update")
        || shutdown_started(caller.app_handle())
    {
        return Err(UNAVAILABLE.into());
    }
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    return imp::review_update(&shell, &extensions, &id).await;
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = (shell, extensions, id);
        Err(UNAVAILABLE.into())
    }
}

/// An extension package among files dropped on the browser: one `.crx` or
/// `.zip` file, or one folder with a manifest.
pub(crate) fn dropped_package(paths: &[PathBuf]) -> Option<String> {
    let [path] = paths else {
        return None;
    };
    let package = match path.extension().and_then(|extension| extension.to_str()) {
        Some("crx" | "zip") => path.is_file(),
        _ => path.join("manifest.json").is_file(),
    };
    package.then(|| path.to_string_lossy().into_owned())
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
mod imp;
mod store_icon;

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn removes_the_previous_repository_even_when_sealed() {
        use std::os::unix::fs::PermissionsExt;
        let data = tempfile::tempdir().unwrap();
        let sealed = data.path().join("extension-repository-v1/objects");
        std::fs::create_dir_all(&sealed).unwrap();
        std::fs::write(sealed.join("package"), b"old").unwrap();
        std::fs::set_permissions(&sealed, std::fs::Permissions::from_mode(0o500)).unwrap();

        let path = data.path().join("extension-repository-v1");
        unseal(&path);
        std::fs::remove_dir_all(&path).unwrap();
        assert!(!path.exists());
    }
}
