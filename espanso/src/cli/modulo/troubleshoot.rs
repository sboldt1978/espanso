/*
 * This file is part of espanso.
 *
 * Copyright (C) 2019-2 file: (), errors: ()  file: (), errors: () 021 Federico Terzi
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

use std::path::Path;
use std::process::Command;

use crate::icon::IconPaths;
use crate::path::Paths;
use crate::preferences::Preferences;
use espanso_modulo::troubleshooting::{TroubleshootingHandlers, TroubleshootingOptions};

fn is_yaml_file(path: &Path) -> bool {
    path.extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("yml") || ext.eq_ignore_ascii_case("yaml"))
}

fn open_with_editor(editor: &str, file_path: &Path) -> anyhow::Result<()> {
    if cfg!(target_os = "windows") {
        Command::new(editor).arg(file_path).spawn()?;
    } else {
        Command::new("/bin/bash")
            .arg("-c")
            .arg(format!("{} '{}'", editor, file_path.to_string_lossy()))
            .spawn()?;
    }
    Ok(())
}

fn open_file_with_yaml_editor(
    file_path: &Path,
    yaml_editor_path: Option<&str>,
) -> anyhow::Result<()> {
    if is_yaml_file(file_path) {
        if let Some(editor) = yaml_editor_path {
            open_with_editor(editor, file_path)?;
            return Ok(());
        }
    }

    opener::open(file_path)?;
    Ok(())
}

pub fn troubleshoot_main(paths: &Paths, icon_paths: &IconPaths) -> i32 {
    let preferences =
        crate::preferences::get_default(&paths.runtime).expect("unable to initialize preferences");

    let dont_show_again_handler = Box::new(move |dont_show: bool| {
        preferences.set_should_display_troubleshoot_for_non_fatal_errors(!dont_show);
    });

    let mut yaml_editor_path: Option<String> = None;

    let (is_fatal_error, error_sets) = match crate::config::load_config(&paths.config) {
        Ok(config_result) => {
            yaml_editor_path = config_result.config_store.default().yaml_editor_path();
            let error_sets = config_result
                .non_fatal_errors
                .into_iter()
                .map(|error_set| espanso_modulo::troubleshooting::ErrorSet {
                    file: Some(error_set.file),
                    errors: error_set
                        .errors
                        .into_iter()
                        .map(|error| espanso_modulo::troubleshooting::ErrorRecord {
                            level: match error.level {
                                espanso_config::error::ErrorLevel::Error => {
                                    espanso_modulo::troubleshooting::ErrorLevel::Error
                                }
                                espanso_config::error::ErrorLevel::Warning => {
                                    espanso_modulo::troubleshooting::ErrorLevel::Warning
                                }
                            },
                            message: format!("{:?}", error.error),
                        })
                        .collect(),
                })
                .collect();

            (false, error_sets)
        }
        Err(err) => {
            let message = format!("{err:?}");
            let file_path = if message.contains("missing default config") {
                let default_file_path = paths.config.join("config").join("default.yml");
                Some(default_file_path)
            } else {
                None
            };

            (
                true,
                vec![espanso_modulo::troubleshooting::ErrorSet {
                    file: file_path,
                    errors: vec![espanso_modulo::troubleshooting::ErrorRecord {
                        level: espanso_modulo::troubleshooting::ErrorLevel::Error,
                        message: format!("{err:?}"),
                    }],
                }],
            )
        }
    };

    let yaml_editor_path = yaml_editor_path.and_then(|path| {
        if path.trim().is_empty() {
            None
        } else {
            Some(path)
        }
    });
    let open_file_handler = Box::new(move |file_path: &Path| {
        if let Err(err) = open_file_with_yaml_editor(file_path, yaml_editor_path.as_deref()) {
            eprintln!("error opening file: {err}");
        }
    });

    espanso_modulo::troubleshooting::show(TroubleshootingOptions {
        window_icon_path: icon_paths
            .wizard_icon
            .as_ref()
            .map(|path| path.to_string_lossy().to_string()),
        error_sets,
        is_fatal_error,
        handlers: TroubleshootingHandlers {
            dont_show_again_changed: Some(dont_show_again_handler),
            open_file: Some(open_file_handler),
        },
    })
    .expect("troubleshoot GUI returned error");

    0
}
