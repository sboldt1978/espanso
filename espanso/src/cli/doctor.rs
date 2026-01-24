/*
 * This file is part of espanso.
 *
 * Copyright (C) 2019-2021 Federico Terzi
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

//! Sample output for a broken config:
//!
//! ERROR: match file '/path/to/base.yml': failed to parse YAML match group
//! WARNING: duplicate trigger ':hi' found in: /path/to/base.yml, /path/to/extra.yml
//! ERROR: match ':email' references missing variables: name
//! ERROR: script variable 'run' in match ':deploy' points to missing file: /path/to/script.sh

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::PathBuf;

use super::{CliModule, CliModuleArgs};
use crate::path::Paths;
use espanso_config::{
    error::ErrorLevel,
    matches::{read_match_group_triggers, Match, MatchEffect, Value},
};

pub fn new() -> CliModule {
    CliModule {
        requires_paths: true,
        requires_config: true,
        subcommand: "doctor".to_string(),
        entry: doctor_main,
        ..Default::default()
    }
}

fn doctor_main(args: CliModuleArgs) -> i32 {
    let match_store = args.match_store.expect("missing match_store argument");
    let config_store = args.config_store.expect("missing config_store argument");
    let paths = args.paths.expect("missing paths argument");

    let mut has_errors = false;

    has_errors |= report_yaml_errors(&args.non_fatal_errors);

    let loaded_paths = match_store.loaded_paths();
    has_errors |= report_duplicate_triggers(&loaded_paths);

    let match_paths: Vec<String> = config_store.get_all_match_paths().into_iter().collect();
    let match_set = match_store.query(&match_paths);

    has_errors |= report_missing_variables(&match_set.matches, &match_set.global_vars);
    has_errors |= report_missing_script_files(&match_set.matches, &paths);

    if has_errors {
        1
    } else {
        println!("OK: configuration looks good");
        0
    }
}

fn report_yaml_errors(non_fatal_errors: &[espanso_config::error::NonFatalErrorSet]) -> bool {
    let mut has_errors = false;
    let mut has_warnings = false;

    for error_set in non_fatal_errors {
        for record in &error_set.errors {
            match record.level {
                ErrorLevel::Error => {
                    has_errors = true;
                    eprintln!(
                        "ERROR: match file '{}': {:#}",
                        error_set.file.display(),
                        record.error
                    );
                }
                ErrorLevel::Warning => {
                    has_warnings = true;
                    eprintln!(
                        "WARNING: match file '{}': {:#}",
                        error_set.file.display(),
                        record.error
                    );
                }
            }
        }
    }

    if !has_errors && !has_warnings {
        println!("OK: match files parsed without YAML errors");
    }

    has_errors
}

fn report_duplicate_triggers(paths: &[String]) -> bool {
    let duplicates = find_duplicate_triggers(paths);

    if duplicates.is_empty() {
        println!("OK: no duplicate triggers detected");
        return false;
    }

    for (trigger, paths) in duplicates {
        eprintln!(
            "WARNING: duplicate trigger '{}' found in: {}",
            trigger,
            paths.join(", ")
        );
    }

    false
}

fn find_duplicate_triggers(paths: &[String]) -> BTreeMap<String, Vec<String>> {
    let mut triggers: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();

    for path in paths {
        if let Ok(triggers_for_file) = read_match_group_triggers(PathBuf::from(path).as_path()) {
            for trigger in triggers_for_file {
                triggers.entry(trigger).or_default().insert(path.clone());
            }
        }
    }

    triggers
        .into_iter()
        .filter_map(|(trigger, paths)| {
            if paths.len() > 1 {
                Some((trigger, paths.into_iter().collect()))
            } else {
                None
            }
        })
        .collect()
}

fn report_missing_variables(
    matches: &[&Match],
    global_vars: &[&espanso_config::matches::Variable],
) -> bool {
    let global_names: HashSet<&str> = global_vars.iter().map(|var| var.name.as_str()).collect();
    let mut has_errors = false;

    for m in matches {
        let match_vars = match &m.effect {
            MatchEffect::Text(effect) => &effect.vars,
            _ => continue,
        };

        let local_names: HashSet<&str> = match_vars.iter().map(|var| var.name.as_str()).collect();
        let available: HashSet<&str> = global_names.union(&local_names).copied().collect();
        let mut missing: BTreeSet<String> = BTreeSet::new();

        if let MatchEffect::Text(effect) = &m.effect {
            for var in extract_variable_names(&effect.replace) {
                if !available.contains(var.as_str()) {
                    missing.insert(var);
                }
            }
        }

        for var in match_vars {
            for name in extract_variable_names_from_params(&var.params) {
                if !available.contains(name.as_str()) {
                    missing.insert(name);
                }
            }
        }

        if !missing.is_empty() {
            has_errors = true;
            let description = m
                .cause_description()
                .or(m.label.as_deref())
                .unwrap_or("(unnamed match)");
            eprintln!(
                "ERROR: match '{}' references missing variables: {}",
                description,
                missing.into_iter().collect::<Vec<_>>().join(", ")
            );
        }
    }

    if !has_errors {
        println!("OK: no missing variable references detected");
    }

    has_errors
}

fn extract_variable_names(body: &str) -> HashSet<String> {
    let mut variables = HashSet::new();
    let mut remaining = body;

    while let Some(start) = remaining.find("{{") {
        let after_start = &remaining[start + 2..];
        let Some(end) = after_start.find("}}") else {
            break;
        };

        let candidate = after_start[..end].trim();
        let candidate = candidate.trim_start_matches('\u{feff}');
        let candidate = candidate.strip_prefix('\\').unwrap_or(candidate);
        let name = candidate
            .split_whitespace()
            .next()
            .unwrap_or("")
            .split('.')
            .next()
            .unwrap_or("");

        if !name.is_empty() {
            variables.insert(name.to_string());
        }

        remaining = &after_start[end + 2..];
    }

    variables
}

fn extract_variable_names_from_params(params: &espanso_config::matches::Params) -> HashSet<String> {
    params
        .values()
        .flat_map(extract_variable_names_from_value)
        .collect()
}

fn extract_variable_names_from_value(value: &Value) -> HashSet<String> {
    match value {
        Value::String(body) => extract_variable_names(body),
        Value::Array(values) => values
            .iter()
            .flat_map(extract_variable_names_from_value)
            .collect(),
        Value::Object(values) => values
            .values()
            .flat_map(extract_variable_names_from_value)
            .collect(),
        _ => HashSet::new(),
    }
}

fn report_missing_script_files(matches: &[&Match], paths: &Paths) -> bool {
    let mut has_errors = false;

    for m in matches {
        let MatchEffect::Text(effect) = &m.effect else {
            continue;
        };

        for var in &effect.vars {
            if var.var_type != "script" {
                continue;
            }

            let Some(Value::Array(args)) = var.params.get("args") else {
                continue;
            };
            let Some(Value::String(raw_path)) = args.first() else {
                continue;
            };

            if let Some((path, resolved)) = resolve_script_path(raw_path, paths) {
                if !path.exists() {
                    has_errors = true;
                    let description = m
                        .cause_description()
                        .or(m.label.as_deref())
                        .unwrap_or("(unnamed match)");
                    eprintln!(
                        "ERROR: script variable '{}' in match '{}' points to missing file: {}",
                        var.name,
                        description,
                        resolved.display()
                    );
                }
            }
        }
    }

    if !has_errors {
        println!("OK: no missing script files detected");
    }

    has_errors
}

fn resolve_script_path(raw: &str, paths: &Paths) -> Option<(PathBuf, PathBuf)> {
    let path_like = raw.contains('/')
        || raw.contains('\\')
        || raw.starts_with('.')
        || raw.starts_with('~')
        || raw.contains("%CONFIG%")
        || raw.contains("%PACKAGES%")
        || raw.contains("%HOME%");

    if !path_like {
        return None;
    }

    let mut resolved = raw.to_string();
    resolved = resolved.replace("%CONFIG%", &paths.config.to_string_lossy());
    resolved = resolved.replace("%PACKAGES%", &paths.packages.to_string_lossy());
    if let Some(home_dir) = dirs::home_dir() {
        resolved = resolved.replace("%HOME%", &home_dir.to_string_lossy());
        if let Some(stripped) = resolved.strip_prefix("~") {
            resolved = format!("{}{}", home_dir.to_string_lossy(), stripped);
        }
    }

    let candidate = PathBuf::from(&resolved);
    if candidate.is_absolute() {
        return Some((candidate.clone(), candidate));
    }

    let config_candidate = paths.config.join(&candidate);
    if config_candidate.exists() {
        return Some((config_candidate.clone(), config_candidate));
    }

    Some((candidate.clone(), candidate))
}
