//! The shared desktop implementation: the registry on disk and handing extensions
//! to the shell.

mod install;
mod response;
mod updates;

pub(in crate::webext) use install::{choose_file, confirm, prepare, prepare_file, review_update};
pub(in crate::webext) use updates::start_updates;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use zephium_app::{Handle, WebExtensionStatus};
use zephium_core::ids::{ExtensionInstallId, ItemId, ProfileId};
use zephium_core::ports::engine::WebExtensionLoad;
use zephium_webext::manifest::Manifest;
use zephium_webext::{archive, crx, permissions, ExtensionId};

use super::{Access, Entry, Registry, WebExtensionView, WebExtensions};

const MAX_PACKAGE_BYTES: u64 = 128 * 1024 * 1024;

mod platform;
use platform::{compat_revision, prepare_package};

impl WebExtensions {
    fn packages(&self, id: &str) -> PathBuf {
        self.root.join("packages").join(id)
    }

    fn registry_path(&self, profile: ProfileId) -> PathBuf {
        self.root.join("profiles").join(format!("{profile}.json"))
    }

    fn registry(&self, profile: ProfileId) -> Registry {
        std::fs::read(self.registry_path(profile))
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    fn save(&self, profile: ProfileId, registry: &Registry) -> Result<(), String> {
        let path = self.registry_path(profile);
        let parent = path.parent().ok_or("invalid registry path")?;
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        let temporary = path.with_extension("json.tmp");
        let bytes = serde_json::to_vec_pretty(registry).map_err(|e| e.to_string())?;
        std::fs::write(&temporary, bytes).map_err(|e| e.to_string())?;
        std::fs::rename(&temporary, &path).map_err(|e| e.to_string())
    }

    fn profiles(&self) -> Vec<ProfileId> {
        std::fs::read_dir(self.root.join("profiles"))
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|entry| {
                let name = entry.file_name().into_string().ok()?;
                ProfileId::parse(name.strip_suffix(".json")?)
            })
            .collect()
    }

    /// Hands a profile's enabled extensions to the shell, rebuilding any
    /// package prepared by an older compatibility layer.
    fn load(
        &self,
        entry: &Entry,
        install: ExtensionInstallId,
        start_background: bool,
    ) -> WebExtensionLoad {
        WebExtensionLoad {
            install,
            extension_id: entry.id.clone(),
            root: self.packages(&entry.id).join(&entry.package),
            permissions: entry.permissions.clone(),
            match_patterns: granted_sites(entry),
            start_background,
        }
    }

    /// Deletes package files no registry refers to: removed extensions,
    /// which are erased by the browser before their files go, and builds
    /// replaced by updates or compatibility-layer changes.
    fn prune(&self) {
        let mut kept: HashMap<String, Vec<String>> = HashMap::new();
        for profile in self.profiles() {
            for entry in self.registry(profile).extensions {
                let files = kept.entry(entry.id.clone()).or_default();
                files.push(entry.package.clone());
                for original in ["crx", "zip", "src"] {
                    files.push(format!("{}.{original}", entry.version));
                }
            }
        }
        let packages = self.root.join("packages");
        std::thread::spawn(move || prune_packages(&packages, &kept));
    }

    fn apply(&self, shell: &Handle, profile: ProfileId, registry: &mut Registry) {
        let mut changed = false;
        let mut loads = Vec::new();
        for entry in registry.extensions.iter_mut() {
            // Windows removal uses the native ID, not a prepared document.
            // Leave disabled packages untouched during startup; enabling one
            // comes through this same path and prepares its current grants then.
            #[cfg(target_os = "windows")]
            if !entry.enabled {
                continue;
            }
            let revision = compat_revision(&entry.access);
            if entry.compat != revision {
                match self.rebuild(entry, &revision) {
                    Ok(()) => changed = true,
                    Err(error) => {
                        eprintln!("extensions: could not rebuild {}: {error}", entry.id);
                        continue;
                    }
                }
            }
            if !entry.enabled {
                continue;
            }
            let Some(install) = ExtensionInstallId::parse(&entry.install) else {
                continue;
            };
            let start_background = entry.started != revision;
            if start_background {
                entry.started = revision.clone();
                changed = true;
            }
            loads.push(self.load(entry, install, start_background));
        }
        if changed {
            let _ = self.save(profile, registry);
        }
        shell.dispatch(zephium_app::Command::SetWebExtensions {
            profile,
            extensions: loads,
        });
    }

    fn rebuild(&self, entry: &mut Entry, revision: &str) -> Result<(), String> {
        let packages = self.packages(&entry.id);
        let package = format!("{}-{revision}", entry.version);
        let target = packages.join(&package);
        #[cfg(target_os = "windows")]
        if zephium_webext::windows::is_prepared(&target, revision) {
            entry.package = package;
            entry.compat = revision.to_owned();
            return Ok(());
        }
        let _ = std::fs::remove_dir_all(&target);
        let limits = archive::Limits::default();
        let original = |kind: &str| packages.join(format!("{}.{kind}", entry.version));
        if original("crx").is_file() {
            let bytes = std::fs::read(original("crx")).map_err(|e| e.to_string())?;
            let expected = ExtensionId::parse(&entry.id).ok_or("invalid extension id")?;
            let verified = crx::verify(&bytes, Some(&expected)).map_err(|e| e.to_string())?;
            archive::extract(verified.zip, &target, &limits).map_err(|e| e.to_string())?;
            platform::record_signed_key(&target, verified.public_key)?;
        } else if original("zip").is_file() {
            let bytes = std::fs::read(original("zip")).map_err(|e| e.to_string())?;
            archive::extract(&bytes, &target, &limits).map_err(|e| e.to_string())?;
        } else {
            archive::copy_dir(&original("src"), &target, &limits).map_err(|e| e.to_string())?;
        }
        if let Err(error) = prepare_package(&target, &entry.access) {
            let _ = std::fs::remove_dir_all(&target);
            return Err(error.to_string());
        }
        let old = std::mem::replace(&mut entry.package, package);
        if cfg!(target_os = "macos") && old != entry.package {
            let _ = std::fs::remove_dir_all(packages.join(old));
        }
        entry.compat = revision.to_owned();
        Ok(())
    }
}

/// The sites an extension is granted under the user's access choice.
fn granted_sites(entry: &Entry) -> Vec<String> {
    match &entry.access {
        Access::All => entry.hosts.clone(),
        Access::Click => Vec::new(),
        Access::Sites { sites } => sites
            .iter()
            .flat_map(|site| [format!("*://{site}/*"), format!("*://*.{site}/*")])
            .collect(),
    }
}

/// A site as the user typed it, reduced to its host: `https://www.x.com/a`
/// becomes `www.x.com`.
fn site_host(site: &str) -> Option<String> {
    let site = site.trim();
    let site = site.split_once("://").map_or(site, |(_, rest)| rest);
    let host = site.split(['/', '?', '#']).next()?.split(':').next()?;
    let host = host.trim_start_matches("*.").to_ascii_lowercase();
    let valid = !host.is_empty()
        && host
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'.' || byte == b'-');
    valid.then_some(host)
}

#[cfg(test)]
#[test]
fn sites_reduce_to_hosts() {
    assert_eq!(
        site_host("https://www.github.com/a?b").as_deref(),
        Some("www.github.com")
    );
    assert_eq!(site_host("*.example.com").as_deref(), Some("example.com"));
    assert_eq!(site_host("localhost:8080").as_deref(), Some("localhost"));
    assert_eq!(site_host("  "), None);
    assert_eq!(site_host("a b.com"), None);
}

fn prune_packages(packages: &Path, kept: &HashMap<String, Vec<String>>) {
    for extension in std::fs::read_dir(packages).into_iter().flatten().flatten() {
        let name = extension.file_name().to_string_lossy().into_owned();
        let Some(files) = kept.get(&name) else {
            let _ = std::fs::remove_dir_all(extension.path());
            continue;
        };
        for file in std::fs::read_dir(extension.path())
            .into_iter()
            .flatten()
            .flatten()
        {
            if files.iter().any(|kept| file.file_name() == kept.as_str()) {
                continue;
            }
            let path = file.path();
            let _ = if path.is_dir() {
                std::fs::remove_dir_all(path)
            } else {
                std::fs::remove_file(path)
            };
        }
    }
}

#[cfg(test)]
#[test]
fn pruning_keeps_only_the_packages_registries_use() {
    let packages = tempfile::tempdir().unwrap();
    for path in ["kept/1.2-aaaa", "kept/1.1-aaaa", "removed/1.0-aaaa"] {
        std::fs::create_dir_all(packages.path().join(path)).unwrap();
    }
    for path in ["kept/1.2.crx", "kept/1.1.crx", "removed/1.0.crx"] {
        std::fs::write(packages.path().join(path), b"crx").unwrap();
    }
    let kept = HashMap::from([(
        "kept".to_string(),
        vec!["1.2-aaaa".to_string(), "1.2.crx".to_string()],
    )]);
    prune_packages(packages.path(), &kept);
    let mut left: Vec<_> = std::fs::read_dir(packages.path().join("kept"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    left.sort();
    assert_eq!(left, ["1.2-aaaa", "1.2.crx"]);
    assert!(!packages.path().join("removed").exists());
}

/// How many extensions are on and how many are installed, over every profile.
pub(in crate::webext) fn counts(extensions: &WebExtensions) -> (usize, usize) {
    extensions
        .profiles()
        .into_iter()
        .flat_map(|profile| extensions.registry(profile).extensions)
        .fold((0, 0), |(enabled, total), entry| {
            (enabled + usize::from(entry.enabled), total + 1)
        })
}

pub(in crate::webext) fn restore(extensions: &WebExtensions, shell: &Handle) {
    for profile in extensions.profiles() {
        let mut registry = extensions.registry(profile);
        extensions.apply(shell, profile, &mut registry);
    }
    extensions.prune();
}

async fn target(
    shell: &Handle,
    tab: Option<ItemId>,
) -> Result<zephium_app::WebExtensionTarget, String> {
    let receiver = shell.web_extension_target(tab);
    tauri::async_runtime::spawn_blocking(move || receiver.recv_timeout(Duration::from_secs(2)))
        .await
        .map_err(|_| "The browser is unavailable.")?
        .map_err(|_| "The browser did not respond.")?
        .ok_or_else(|| "Extensions can't be installed in this window.".to_owned())
}

pub(in crate::webext) async fn list(
    shell: &Handle,
    extensions: &WebExtensions,
) -> Result<Vec<WebExtensionView>, String> {
    // Before the session exists there is nothing to list yet; the build
    // still supports extensions.
    let Ok(profile) = target(shell, None).await.map(|target| target.profile) else {
        return Ok(Vec::new());
    };
    let receiver = shell.web_extension_status(profile);
    let status: HashMap<String, WebExtensionStatus> =
        tauri::async_runtime::spawn_blocking(move || receiver.recv_timeout(Duration::from_secs(2)))
            .await
            .ok()
            .and_then(Result::ok)
            .unwrap_or_default()
            .into_iter()
            .map(|(install, status)| (install.to_string(), status))
            .collect();
    let root = extensions.root.clone();
    Ok(extensions
        .registry(profile)
        .extensions
        .into_iter()
        .map(|entry| {
            let (state, error) = match (entry.enabled, status.get(&entry.install)) {
                (false, _) => ("off", None),
                (true, Some(WebExtensionStatus::Running(_))) => ("running", None),
                (true, Some(WebExtensionStatus::Failed(error))) => ("failed", Some(error.clone())),
                (true, _) => ("starting", None),
            };
            let manifest =
                Manifest::load(&root.join("packages").join(&entry.id).join(&entry.package)).ok();
            let warnings = manifest
                .as_ref()
                .map(permissions::warnings)
                .unwrap_or_default();
            let has_options = manifest.as_ref().is_some_and(|manifest| {
                let raw = manifest.raw();
                raw.get("options_page").is_some()
                    || raw
                        .get("options_ui")
                        .and_then(|ui| ui.get("page"))
                        .is_some()
            });
            let (access, sites) = match &entry.access {
                Access::All => ("all", Vec::new()),
                Access::Click => ("click", Vec::new()),
                Access::Sites { sites } => ("sites", sites.clone()),
            };
            WebExtensionView {
                id: entry.id,
                name: entry.name,
                version: entry.version,
                description: entry.description,
                enabled: entry.enabled,
                icon: entry.icon,
                state: state.to_owned(),
                error,
                warnings,
                access: access.to_owned(),
                sites,
                site_scoped: !entry.hosts.is_empty(),
                has_options,
                held_update: entry.held_update,
                sideloaded: entry.sideloaded,
            }
        })
        .collect())
}

pub(in crate::webext) async fn set_enabled(
    shell: &Handle,
    extensions: &WebExtensions,
    id: &str,
    enabled: bool,
) -> Result<(), String> {
    let profile = target(shell, None).await?.profile;
    let mut registry = extensions.registry(profile);
    let entry = registry
        .extensions
        .iter_mut()
        .find(|entry| entry.id == id)
        .ok_or("That extension isn't installed.")?;
    entry.enabled = enabled;
    extensions.save(profile, &registry)?;
    extensions.apply(shell, profile, &mut registry);
    Ok(())
}

pub(in crate::webext) async fn set_access(
    shell: &Handle,
    extensions: &WebExtensions,
    id: &str,
    mode: &str,
    sites: Vec<String>,
) -> Result<(), String> {
    let access = match mode {
        "all" => Access::All,
        "click" if !cfg!(target_os = "windows") => Access::Click,
        "sites" => {
            let mut sites: Vec<String> = sites.iter().filter_map(|site| site_host(site)).collect();
            sites.sort();
            sites.dedup();
            Access::Sites { sites }
        }
        _ => return Err("Unknown site access.".into()),
    };
    let profile = target(shell, None).await?.profile;
    let mut registry = extensions.registry(profile);
    let entry = registry
        .extensions
        .iter_mut()
        .find(|entry| entry.id == id)
        .ok_or("That extension isn't installed.")?;
    entry.access = access;
    extensions.save(profile, &registry)?;
    extensions.apply(shell, profile, &mut registry);
    Ok(())
}

pub(in crate::webext) async fn open_options(shell: &Handle, id: &str) -> Result<(), String> {
    let profile = target(shell, None).await?.profile;
    shell.dispatch(zephium_app::Command::OpenWebExtensionOptions {
        profile,
        extension_id: id.to_owned(),
    });
    Ok(())
}

pub(in crate::webext) fn answer_access(
    shell: &Handle,
    extensions: &WebExtensions,
    request: zephium_ipc::WebExtensionAccessRequestView,
    allowed: bool,
) -> Result<(), String> {
    let profile = ProfileId::parse(&request.profile_id).ok_or("Invalid profile.")?;
    let number: u64 = request.request.parse().map_err(|_| "Invalid request.")?;
    if allowed {
        let mut registry = extensions.registry(profile);
        if let Some(entry) = registry
            .extensions
            .iter_mut()
            .find(|entry| entry.id == request.extension_id)
        {
            for permission in request.permissions {
                if !entry.permissions.contains(&permission) {
                    entry.permissions.push(permission);
                }
            }
            for pattern in request.patterns {
                if !entry.hosts.contains(&pattern) {
                    entry.hosts.push(pattern);
                }
            }
            extensions.save(profile, &registry)?;
        }
    }
    shell.dispatch(zephium_app::Command::AnswerWebExtensionAccess {
        profile,
        request: number,
        allowed,
    });
    Ok(())
}

pub(in crate::webext) async fn uninstall(
    shell: &Handle,
    extensions: &WebExtensions,
    id: &str,
) -> Result<(), String> {
    let profile = target(shell, None).await?.profile;
    let mut registry = extensions.registry(profile);
    let Some(position) = registry.extensions.iter().position(|entry| entry.id == id) else {
        return Err("That extension isn't installed.".into());
    };
    let entry = registry.extensions.remove(position);
    extensions.save(profile, &registry)?;
    // Its package files stay until the next launch: the browser may need
    // them to load the extension once more to erase its data.
    if let Some(install) = ExtensionInstallId::parse(&entry.install) {
        shell.dispatch(zephium_app::Command::RemoveWebExtension {
            profile,
            extension: Box::new(extensions.load(&entry, install, false)),
        });
    }
    extensions.apply(shell, profile, &mut registry);
    Ok(())
}
