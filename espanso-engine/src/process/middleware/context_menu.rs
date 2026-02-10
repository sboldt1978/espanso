/*
 * This file is part of espanso.
 *
 * Copyright  id: (), label: () id: (), label: () id: (), label: ()(C) 2019-2021 Federico Terzi
 *
 * espanso is free software: you can redistribute it and/or modify
 * it under the terms of the GNU General Public License as published by
 * the Free Software Foundation, either version 3 of the License, or
 * (at your option) any later version.
 *
 * espanso is distributed in the hope that it will be useful,
 * but WITHOUT ANY WARRANTY; without even the implied warranty of
 * MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
 * GNU General Public License for more details.
 *
 * You should have received a copy of the GNU General Public License
 * along with espanso.  If not, see <https://www.gnu.org/licenses/>.
 */

use std::{cell::RefCell, collections::HashMap, path::PathBuf};

use super::super::Middleware;
use crate::event::{
    ui::{MenuItem, ShowContextMenuEvent, SimpleMenuItem, SubMenuItem},
    Event, EventType, ExitMode,
};
use anyhow::Result;
use log::error;

const CONTEXT_ITEM_EXIT: u32 = 0;
const CONTEXT_ITEM_RELOAD: u32 = 1;
const CONTEXT_ITEM_ENABLE: u32 = 2;
const CONTEXT_ITEM_DISABLE: u32 = 3;
const CONTEXT_ITEM_SECURE_INPUT_EXPLAIN: u32 = 4;
const CONTEXT_ITEM_SECURE_INPUT_TRIGGER_WORKAROUND: u32 = 5;
const CONTEXT_ITEM_OPEN_SEARCH: u32 = 6;
const CONTEXT_ITEM_SHOW_LOGS: u32 = 7;
const CONTEXT_ITEM_OPEN_CONFIG_FOLDER: u32 = 8;
const CONTEXT_ITEM_EXPORT_CONFIG: u32 = 9;
const CONTEXT_ITEM_IMPORT_CONFIG: u32 = 10;
const CONTEXT_ITEM_EXPLAIN_MATCH: u32 = 11;
const CONTEXT_ITEM_OPEN_FILE_START: u32 = 1000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OpenFileMenuScope {
    Config,
    Matches,
    Packages,
}

#[derive(Debug, Clone)]
pub struct OpenFileMenuItem {
    pub path: PathBuf,
    pub scope: OpenFileMenuScope,
}

pub struct OpenFileMenuBuildResult {
    pub items: Vec<MenuItem>,
    pub item_map: HashMap<u32, OpenFileMenuItem>,
}

pub trait OpenFileMenuProvider {
    fn build_open_file_menu(&self, start_id: u32) -> OpenFileMenuBuildResult;
    fn open_file(&self, item: &OpenFileMenuItem) -> Result<()>;
}

pub struct ContextMenuMiddleware<'a> {
    is_enabled: RefCell<bool>,
    is_secure_input_enabled: RefCell<bool>,
    open_file_menu_provider: &'a dyn OpenFileMenuProvider,
    open_file_item_map: RefCell<HashMap<u32, OpenFileMenuItem>>,
}

impl<'a> ContextMenuMiddleware<'a> {
    pub fn new(open_file_menu_provider: &'a dyn OpenFileMenuProvider) -> Self {
        Self {
            is_enabled: RefCell::new(true),
            is_secure_input_enabled: RefCell::new(false),
            open_file_menu_provider,
            open_file_item_map: RefCell::new(HashMap::new()),
        }
    }
}

impl Middleware for ContextMenuMiddleware<'_> {
    fn name(&self) -> &'static str {
        "context_menu"
    }

    fn next(&self, event: Event, dispatch: &mut dyn FnMut(Event)) -> Event {
        let mut is_enabled = self.is_enabled.borrow_mut();
        let mut is_secure_input_enabled = self.is_secure_input_enabled.borrow_mut();

        match &event.etype {
            EventType::TrayIconClicked => {
                // TODO: fetch top matches for the active config to be added

                let open_file_menu = self
                    .open_file_menu_provider
                    .build_open_file_menu(CONTEXT_ITEM_OPEN_FILE_START);
                {
                    let mut map = self.open_file_item_map.borrow_mut();
                    *map = open_file_menu.item_map;
                }

                let mut items = vec![
                    MenuItem::Simple(if *is_enabled {
                        SimpleMenuItem {
                            id: CONTEXT_ITEM_DISABLE,
                            label: "Disable".to_string(),
                            enabled: true,
                        }
                    } else {
                        SimpleMenuItem {
                            id: CONTEXT_ITEM_ENABLE,
                            label: "Enable".to_string(),
                            enabled: true,
                        }
                    }),
                    MenuItem::Simple(SimpleMenuItem {
                        id: CONTEXT_ITEM_OPEN_SEARCH,
                        label: "Open search bar".to_string(),
                        enabled: true,
                    }),
                    MenuItem::Separator,
                    MenuItem::Simple(SimpleMenuItem {
                        id: CONTEXT_ITEM_RELOAD,
                        label: "Reload config".to_string(),
                        enabled: true,
                    }),
                    MenuItem::Simple(SimpleMenuItem {
                        id: CONTEXT_ITEM_OPEN_CONFIG_FOLDER,
                        label: "Open config folder".to_string(),
                        enabled: true,
                    }),
                    MenuItem::Sub(SubMenuItem {
                        label: "Edit config file".to_string(),
                        items: open_file_menu.items,
                    }),
                    MenuItem::Simple(SimpleMenuItem {
                        id: CONTEXT_ITEM_EXPLAIN_MATCH,
                        label: "Explain match".to_string(),
                        enabled: true,
                    }),
                    MenuItem::Simple(SimpleMenuItem {
                        id: CONTEXT_ITEM_SHOW_LOGS,
                        label: "Show logs".to_string(),
                        enabled: true,
                    }),
                    MenuItem::Separator,
                    MenuItem::Simple(SimpleMenuItem {
                        id: CONTEXT_ITEM_EXPORT_CONFIG,
                        label: "Export config".to_string(),
                        enabled: true,
                    }),
                    MenuItem::Simple(SimpleMenuItem {
                        id: CONTEXT_ITEM_IMPORT_CONFIG,
                        label: "Import config".to_string(),
                        enabled: true,
                    }),
                    MenuItem::Separator,
                    MenuItem::Simple(SimpleMenuItem {
                        id: CONTEXT_ITEM_EXIT,
                        label: "Exit espanso".to_string(),
                        enabled: true,
                    }),
                ];

                if *is_secure_input_enabled {
                    items.insert(
                        0,
                        MenuItem::Simple(SimpleMenuItem {
                            id: CONTEXT_ITEM_SECURE_INPUT_EXPLAIN,
                            label: "Why is Espanso not working?".to_string(),
                            enabled: true,
                        }),
                    );
                    items.insert(
                        1,
                        MenuItem::Simple(SimpleMenuItem {
                            id: CONTEXT_ITEM_SECURE_INPUT_TRIGGER_WORKAROUND,
                            label: "Launch SecureInput auto-fix".to_string(),
                            enabled: true,
                        }),
                    );
                    items.insert(2, MenuItem::Separator);
                }

                // TODO: my idea is to use a set of reserved u32 ids for built-in
                // actions such as Exit, Open Editor etc
                // then we need some u32 for the matches, so we need to create
                // a mapping structure match_id <-> context-menu-id
                Event::caused_by(
                    event.source_id,
                    EventType::ShowContextMenu(ShowContextMenuEvent {
                        // TODO: add actual entries
                        items,
                    }),
                )
            }
            EventType::ContextMenuClicked(context_click_event) => {
                if let Some(item) = self
                    .open_file_item_map
                    .borrow()
                    .get(&context_click_event.context_item_id)
                    .cloned()
                {
                    if let Err(err) = self.open_file_menu_provider.open_file(&item) {
                        error!("unable to open file from menu: {err}");
                    }
                    return Event::caused_by(event.source_id, EventType::NOOP);
                }

                match context_click_event.context_item_id {
                    CONTEXT_ITEM_EXIT => Event::caused_by(
                        event.source_id,
                        EventType::ExitRequested(ExitMode::ExitAllProcesses),
                    ),
                    CONTEXT_ITEM_RELOAD => Event::caused_by(
                        event.source_id,
                        EventType::ExitRequested(ExitMode::RestartWorker),
                    ),
                    CONTEXT_ITEM_ENABLE => {
                        dispatch(Event::caused_by(event.source_id, EventType::EnableRequest));
                        Event::caused_by(event.source_id, EventType::NOOP)
                    }
                    CONTEXT_ITEM_DISABLE => {
                        dispatch(Event::caused_by(event.source_id, EventType::DisableRequest));
                        Event::caused_by(event.source_id, EventType::NOOP)
                    }
                    CONTEXT_ITEM_SECURE_INPUT_EXPLAIN => {
                        dispatch(Event::caused_by(
                            event.source_id,
                            EventType::DisplaySecureInputTroubleshoot,
                        ));
                        Event::caused_by(event.source_id, EventType::NOOP)
                    }
                    CONTEXT_ITEM_SECURE_INPUT_TRIGGER_WORKAROUND => {
                        dispatch(Event::caused_by(
                            event.source_id,
                            EventType::LaunchSecureInputAutoFix,
                        ));
                        Event::caused_by(event.source_id, EventType::NOOP)
                    }
                    CONTEXT_ITEM_OPEN_SEARCH => {
                        dispatch(Event::caused_by(event.source_id, EventType::ShowSearchBar));
                        Event::caused_by(event.source_id, EventType::NOOP)
                    }
                    CONTEXT_ITEM_SHOW_LOGS => {
                        dispatch(Event::caused_by(event.source_id, EventType::ShowLogs));
                        Event::caused_by(event.source_id, EventType::NOOP)
                    }
                    CONTEXT_ITEM_OPEN_CONFIG_FOLDER => {
                        dispatch(Event::caused_by(
                            event.source_id,
                            EventType::ShowConfigFolder,
                        ));
                        Event::caused_by(event.source_id, EventType::NOOP)
                    }
                    CONTEXT_ITEM_EXPORT_CONFIG => {
                        dispatch(Event::caused_by(event.source_id, EventType::ExportConfig));
                        Event::caused_by(event.source_id, EventType::NOOP)
                    }
                    CONTEXT_ITEM_IMPORT_CONFIG => {
                        dispatch(Event::caused_by(event.source_id, EventType::ImportConfig));
                        Event::caused_by(event.source_id, EventType::NOOP)
                    }
<<<<<<< HEAD
                    CONTEXT_ITEM_EXPLAIN_MATCH => {
                        dispatch(Event::caused_by(
                            event.source_id,
                            EventType::ShowMatchExplainDialog,
                        ));
                        Event::caused_by(event.source_id, EventType::NOOP)
                    }
                    _ => Event::caused_by(event.source_id, EventType::NOOP),
                }
            }
            EventType::Disabled => {
                *is_enabled = false;
                event
            }
            EventType::Enabled => {
                *is_enabled = true;
                event
            }
            EventType::SecureInputEnabled(_) => {
                *is_secure_input_enabled = true;
                event
            }
            EventType::SecureInputDisabled => {
                *is_secure_input_enabled = false;
                event
            }
            _ => event,
        }
    }
}
