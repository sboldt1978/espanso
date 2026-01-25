use std::{
    cell::RefCell,
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    time::SystemTime,
};

use anyhow::{Context, Result};
use espanso_config::config::Config;
use espanso_engine::process::{
    OpenFileMenuBuildResult, OpenFileMenuItem, OpenFileMenuProvider, OpenFileMenuScope,
};
use serde::{Deserialize, Serialize};

use crate::path::Paths;

const RECENT_FILES_DB_NAME: &str = "open-file-recent.json";

pub struct OpenFileMenuProviderAdapter<'a> {
    paths: &'a Paths,
    config: std::sync::Arc<dyn Config>,
    state: RefCell<OpenFileMenuState>,
}

impl<'a> OpenFileMenuProviderAdapter<'a> {
    pub fn new(paths: &'a Paths, config: std::sync::Arc<dyn Config>) -> Self {
        let config_root = paths.config.join("config");
        let matches_root = paths.config.join("match");
        let packages_root = paths.packages.clone();
        let matches_exclude = packages_root
            .strip_prefix(&matches_root)
            .ok()
            .map(|_| packages_root.clone());

        Self {
            paths,
            config,
            state: RefCell::new(OpenFileMenuState {
                cache: OpenFileMenuCache {
                    config: CachedTree::new(config_root, None),
                    matches: CachedTree::new(matches_root, matches_exclude),
                    packages: CachedTree::new(packages_root, None),
                },
                recent_state: None,
            }),
        }
    }

    fn recent_state_path(&self) -> PathBuf {
        self.paths.config.join(RECENT_FILES_DB_NAME)
    }
}

impl OpenFileMenuProvider for OpenFileMenuProviderAdapter<'_> {
    fn build_open_file_menu(&self, start_id: u32) -> OpenFileMenuBuildResult {
        let mut state = self.state.borrow_mut();
        state.cache.refresh();
        state.ensure_recent_state(&self.recent_state_path());

        let global_limit = self.config.open_file_menu_recent_files_count();
        let per_scope_limit = self.config.open_file_menu_recent_files_per_scope_count();
        let recent_state = state.recent_state.as_ref().cloned().unwrap_or_default();

        let mut id_gen = IdGenerator::new(start_id);
        let mut item_map = HashMap::new();

        let config_recent = collect_scope_recent(
            &recent_state.config,
            &state.cache.config.root,
            OpenFileMenuScope::Config,
            per_scope_limit,
        );
        let matches_recent = collect_scope_recent(
            &recent_state.matches,
            &state.cache.matches.root,
            OpenFileMenuScope::Matches,
            per_scope_limit,
        );
        let packages_recent = collect_scope_recent(
            &recent_state.packages,
            &state.cache.packages.root,
            OpenFileMenuScope::Packages,
            per_scope_limit,
        );

        let config_items = build_scope_menu(
            &state.cache.config.entries,
            OpenFileMenuScope::Config,
            &config_recent,
            &mut id_gen,
            &mut item_map,
        );
        let matches_items = build_scope_menu(
            &state.cache.matches.entries,
            OpenFileMenuScope::Matches,
            &matches_recent,
            &mut id_gen,
            &mut item_map,
        );
        let packages_items = build_scope_menu(
            &state.cache.packages.entries,
            OpenFileMenuScope::Packages,
            &packages_recent,
            &mut id_gen,
            &mut item_map,
        );

        let global_recent = collect_global_recent(&recent_state.global, &state.cache, global_limit);

        let mut items = vec![
            espanso_engine::event::ui::MenuItem::Sub(espanso_engine::event::ui::SubMenuItem {
                label: "Config".to_string(),
                items: config_items,
            }),
            espanso_engine::event::ui::MenuItem::Sub(espanso_engine::event::ui::SubMenuItem {
                label: "Matches".to_string(),
                items: matches_items,
            }),
            espanso_engine::event::ui::MenuItem::Sub(espanso_engine::event::ui::SubMenuItem {
                label: "Packages".to_string(),
                items: packages_items,
            }),
        ];

        if !global_recent.is_empty() {
            items.push(espanso_engine::event::ui::MenuItem::Separator);
            items.push(espanso_engine::event::ui::MenuItem::Simple(
                espanso_engine::event::ui::SimpleMenuItem {
                    id: id_gen.next_id(),
                    label: "Recently opened (All)".to_string(),
                    enabled: false,
                },
            ));

            for entry in global_recent {
                let id = id_gen.next_id();
                item_map.insert(
                    id,
                    OpenFileMenuItem {
                        path: entry.path.clone(),
                        scope: entry.scope,
                    },
                );
                items.push(espanso_engine::event::ui::MenuItem::Simple(
                    espanso_engine::event::ui::SimpleMenuItem {
                        id,
                        label: entry.label,
                        enabled: true,
                    },
                ));
            }
        }

        OpenFileMenuBuildResult { items, item_map }
    }

    fn open_file(&self, item: &OpenFileMenuItem) -> Result<()> {
        let editor_path = self
            .config
            .open_file_menu_yaml_editor_path()
            .and_then(|path| (!path.trim().is_empty()).then_some(path));

        if is_yaml_file(&item.path) {
            if let Some(editor) = editor_path.as_deref() {
                open_with_editor(editor, &item.path)?;
            } else {
                open_with_default_app(&item.path)?;
            }
        } else {
            open_with_default_app(&item.path)?;
        }

        let mut state = self.state.borrow_mut();
        let recent_state = state.recent_state_mut(self.recent_state_path());
        let global_limit = self.config.open_file_menu_recent_files_count();
        let per_scope_limit = self.config.open_file_menu_recent_files_per_scope_count();
        record_recent(
            recent_state,
            item.scope,
            &item.path,
            global_limit,
            per_scope_limit,
        );
        save_recent_state(self.recent_state_path(), recent_state)?;

        Ok(())
    }
}

struct OpenFileMenuState {
    cache: OpenFileMenuCache,
    recent_state: Option<OpenFileRecentState>,
}

impl OpenFileMenuState {
    fn ensure_recent_state(&mut self, path: &Path) {
        if self.recent_state.is_none() {
            self.recent_state = Some(load_recent_state(path));
        }
    }

    fn recent_state_mut(&mut self, path: PathBuf) -> &mut OpenFileRecentState {
        if self.recent_state.is_none() {
            self.recent_state = Some(load_recent_state(&path));
        }
        self.recent_state.as_mut().expect("recent state missing")
    }
}

struct OpenFileMenuCache {
    config: CachedTree,
    matches: CachedTree,
    packages: CachedTree,
}

impl OpenFileMenuCache {
    fn refresh(&mut self) {
        self.config.refresh();
        self.matches.refresh();
        self.packages.refresh();
    }
}

struct CachedTree {
    root: PathBuf,
    exclude: Option<PathBuf>,
    entries: Vec<FileNode>,
    last_modified: Option<SystemTime>,
    initialized: bool,
}

impl CachedTree {
    fn new(root: PathBuf, exclude: Option<PathBuf>) -> Self {
        Self {
            root,
            exclude,
            entries: Vec::new(),
            last_modified: None,
            initialized: false,
        }
    }

    fn refresh(&mut self) {
        if !self.root.exists() {
            self.entries.clear();
            self.last_modified = None;
            self.initialized = true;
            return;
        }

        let current_modified = fs::metadata(&self.root)
            .and_then(|meta| meta.modified())
            .ok();

        if !self.initialized || self.last_modified != current_modified {
            self.entries = read_tree(&self.root, self.exclude.as_deref());
            self.last_modified = current_modified;
            self.initialized = true;
        }
    }
}

#[derive(Clone)]
enum FileNode {
    File {
        name: String,
        path: PathBuf,
    },
    Dir {
        name: String,
        children: Vec<FileNode>,
    },
}

#[derive(Default, Serialize, Deserialize, Clone)]
struct OpenFileRecentState {
    global: Vec<String>,
    config: Vec<String>,
    matches: Vec<String>,
    packages: Vec<String>,
}

#[derive(Clone)]
struct RecentEntry {
    path: PathBuf,
    label: String,
    scope: OpenFileMenuScope,
}

struct IdGenerator {
    next: u32,
}

impl IdGenerator {
    fn new(start: u32) -> Self {
        Self { next: start }
    }

    fn next_id(&mut self) -> u32 {
        let id = self.next;
        self.next = self.next.saturating_add(1);
        id
    }
}

fn read_tree(root: &Path, exclude: Option<&Path>) -> Vec<FileNode> {
    let mut entries = Vec::new();
    let dir_entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(_) => return entries,
    };

    for entry in dir_entries.flatten() {
        let path = entry.path();
        if exclude.is_some_and(|excluded| path.starts_with(excluded)) {
            continue;
        }

        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') {
            continue;
        }
        if path.is_dir() {
            let children = read_tree(&path, exclude);
            if !children.is_empty() {
                entries.push(FileNode::Dir { name, children });
            }
        } else if path.is_file() {
            entries.push(FileNode::File { name, path });
        }
    }

    entries.sort_by(|left, right| {
        let left_key = node_sort_key(left);
        let right_key = node_sort_key(right);
        left_key.cmp(&right_key)
    });

    entries
}

fn node_sort_key(node: &FileNode) -> (u8, String) {
    match node {
        FileNode::Dir { name, .. } => (0, name.to_lowercase()),
        FileNode::File { name, .. } => (1, name.to_lowercase()),
    }
}

fn build_scope_menu(
    entries: &[FileNode],
    scope: OpenFileMenuScope,
    recent_entries: &[RecentEntry],
    id_gen: &mut IdGenerator,
    item_map: &mut HashMap<u32, OpenFileMenuItem>,
) -> Vec<espanso_engine::event::ui::MenuItem> {
    let mut items = Vec::new();
    let tree_items = build_tree_menu(entries, scope, id_gen, item_map);

    if !recent_entries.is_empty() {
        let mut recent_items = Vec::new();

        for entry in recent_entries {
            let id = id_gen.next_id();
            item_map.insert(
                id,
                OpenFileMenuItem {
                    path: entry.path.clone(),
                    scope,
                },
            );
            recent_items.push(espanso_engine::event::ui::MenuItem::Simple(
                espanso_engine::event::ui::SimpleMenuItem {
                    id,
                    label: entry.label.clone(),
                    enabled: true,
                },
            ));
        }

        items.push(espanso_engine::event::ui::MenuItem::Sub(
            espanso_engine::event::ui::SubMenuItem {
                label: "Recently opened".to_string(),
                items: recent_items,
            },
        ));

        if !tree_items.is_empty() {
            items.push(espanso_engine::event::ui::MenuItem::Separator);
        }
    }

    items.extend(tree_items);
    items
}

fn build_tree_menu(
    entries: &[FileNode],
    scope: OpenFileMenuScope,
    id_gen: &mut IdGenerator,
    item_map: &mut HashMap<u32, OpenFileMenuItem>,
) -> Vec<espanso_engine::event::ui::MenuItem> {
    let mut items = Vec::new();

    for entry in entries {
        match entry {
            FileNode::Dir { name, children, .. } => {
                let children_items = build_tree_menu(children, scope, id_gen, item_map);
                if !children_items.is_empty() {
                    items.push(espanso_engine::event::ui::MenuItem::Sub(
                        espanso_engine::event::ui::SubMenuItem {
                            label: name.clone(),
                            items: children_items,
                        },
                    ));
                }
            }
            FileNode::File { name, path } => {
                let id = id_gen.next_id();
                item_map.insert(
                    id,
                    OpenFileMenuItem {
                        path: path.clone(),
                        scope,
                    },
                );
                items.push(espanso_engine::event::ui::MenuItem::Simple(
                    espanso_engine::event::ui::SimpleMenuItem {
                        id,
                        label: name.clone(),
                        enabled: true,
                    },
                ));
            }
        }
    }

    items
}

fn collect_scope_recent(
    entries: &[String],
    root: &Path,
    scope: OpenFileMenuScope,
    limit: usize,
) -> Vec<RecentEntry> {
    let mut recent_entries = Vec::new();

    for entry in entries {
        if recent_entries.len() >= limit {
            break;
        }

        let path = PathBuf::from(entry);
        if !path.exists() {
            continue;
        }

        if is_hidden_path(&path) {
            continue;
        }

        if !path.starts_with(root) {
            continue;
        }

        let label = relative_label(root, &path);
        recent_entries.push(RecentEntry { path, label, scope });
    }

    recent_entries
}

fn collect_global_recent(
    entries: &[String],
    cache: &OpenFileMenuCache,
    limit: usize,
) -> Vec<RecentEntry> {
    let mut recent_entries = Vec::new();

    for entry in entries {
        if recent_entries.len() >= limit {
            break;
        }

        let path = PathBuf::from(entry);
        if !path.exists() {
            continue;
        }

        if is_hidden_path(&path) {
            continue;
        }

        let scope = if path.starts_with(&cache.config.root) {
            OpenFileMenuScope::Config
        } else if path.starts_with(&cache.packages.root) {
            OpenFileMenuScope::Packages
        } else if path.starts_with(&cache.matches.root) {
            OpenFileMenuScope::Matches
        } else {
            continue;
        };

        let root = match scope {
            OpenFileMenuScope::Config => &cache.config.root,
            OpenFileMenuScope::Matches => &cache.matches.root,
            OpenFileMenuScope::Packages => &cache.packages.root,
        };

        let relative = relative_label(root, &path);
        let label = format!("{}: {}", scope_label(scope), relative);
        recent_entries.push(RecentEntry { path, label, scope });
    }

    recent_entries
}

fn relative_label(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .to_string()
}

fn scope_label(scope: OpenFileMenuScope) -> &'static str {
    match scope {
        OpenFileMenuScope::Config => "Config",
        OpenFileMenuScope::Matches => "Matches",
        OpenFileMenuScope::Packages => "Packages",
    }
}

fn is_hidden_path(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .map(|name| name.starts_with('.'))
        .unwrap_or(false)
}

fn load_recent_state(path: &Path) -> OpenFileRecentState {
    match fs::read_to_string(path) {
        Ok(contents) => serde_json::from_str(&contents).unwrap_or_default(),
        Err(_) => OpenFileRecentState::default(),
    }
}

fn save_recent_state(path: PathBuf, state: &OpenFileRecentState) -> Result<()> {
    let payload = serde_json::to_string_pretty(state).context("serialize recent files")?;
    let tmp_path = path.with_extension("json.tmp");
    fs::write(&tmp_path, payload)?;
    if let Err(err) = fs::rename(&tmp_path, &path) {
        if path.exists() {
            fs::remove_file(&path)?;
        }
        fs::rename(&tmp_path, &path)
            .with_context(|| format!("replace recent file after error: {err}"))?;
    }
    Ok(())
}

fn record_recent(
    state: &mut OpenFileRecentState,
    scope: OpenFileMenuScope,
    path: &Path,
    global_limit: usize,
    per_scope_limit: usize,
) {
    let path_string = path.to_string_lossy().to_string();
    push_recent(&mut state.global, &path_string, global_limit);
    match scope {
        OpenFileMenuScope::Config => {
            push_recent(&mut state.config, &path_string, per_scope_limit);
        }
        OpenFileMenuScope::Matches => {
            push_recent(&mut state.matches, &path_string, per_scope_limit);
        }
        OpenFileMenuScope::Packages => {
            push_recent(&mut state.packages, &path_string, per_scope_limit);
        }
    }
}

fn push_recent(target: &mut Vec<String>, value: &str, limit: usize) {
    target.retain(|entry| entry != value);
    target.insert(0, value.to_string());
    if target.len() > limit {
        target.truncate(limit);
    }
}

fn is_yaml_file(path: &Path) -> bool {
    path.extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("yml") || ext.eq_ignore_ascii_case("yaml"))
}

fn open_with_editor(editor: &str, file_path: &Path) -> Result<()> {
    if cfg!(target_os = "windows") {
        std::process::Command::new(editor)
            .arg(file_path)
            .spawn()
            .context("spawn editor")?;
    } else {
        std::process::Command::new("/bin/bash")
            .arg("-c")
            .arg(format!("{} '{}'", editor, file_path.to_string_lossy()))
            .spawn()
            .context("spawn editor")?;
    }

    Ok(())
}

fn open_with_default_app(file_path: &Path) -> Result<()> {
    if cfg!(target_os = "macos") {
        std::process::Command::new("open")
            .arg(file_path)
            .spawn()
            .context("spawn open")?;
    } else if cfg!(target_os = "windows") {
        let path_string = file_path.to_string_lossy();
        std::process::Command::new("cmd")
            .args(["/C", "start", "", path_string.as_ref()])
            .spawn()
            .context("spawn start")?;
    } else {
        std::process::Command::new("xdg-open")
            .arg(file_path)
            .spawn()
            .context("spawn xdg-open")?;
    }

    Ok(())
}
