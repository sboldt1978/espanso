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

//! Espanso Doctor - Configuration validation and diagnostics
//!
//! Sample output for a broken config:
//!
//! ```text
//! ERROR [E001]: match file '/path/to/base.yml': failed to parse YAML match group
//! WARNING [W001]: duplicate trigger ':hi' found in: /path/to/base.yml, /path/to/extra.yml
//! ERROR [E002]: match ':email' references missing variables: name
//! ERROR [E003]: script variable 'run' in match ':deploy' points to missing file: /path/to/script.sh
//!
//! Summary: 3 errors, 1 warning
//! ```

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::PathBuf;

use log::warn;
use regex::Regex;
use serde::Serialize;

use super::{CliModule, CliModuleArgs};
use crate::cli::launcher::accessibility::is_accessibility_enabled;
use crate::path::Paths;
use espanso_config::{
    error::ErrorLevel,
    matches::{read_match_group_triggers, Match, MatchCause, MatchEffect, RegexCause, Value},
};

/// Diagnostic severity level
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum DiagnosticLevel {
    Error,
    Warning,
}

/// Diagnostic error/warning codes
#[derive(Debug, Clone, Copy, Serialize)]
pub enum DiagnosticCode {
    // Errors (E0xx)
    #[serde(rename = "E001")]
    YamlParseError,
    #[serde(rename = "E002")]
    MissingVariable,
    #[serde(rename = "E003")]
    MissingScriptFile,
    #[serde(rename = "E004")]
    InvalidRegex,
    #[serde(rename = "E005")]
    AccessibilityNotEnabled,
    #[serde(rename = "E006")]
    FileReadError,

    // Warnings (W0xx)
    #[serde(rename = "W001")]
    DuplicateTrigger,
    #[serde(rename = "W002")]
    EmptyTrigger,
    #[serde(rename = "W003")]
    YamlParseWarning,
}

impl DiagnosticCode {
    fn as_str(&self) -> &'static str {
        match self {
            DiagnosticCode::YamlParseError => "E001",
            DiagnosticCode::MissingVariable => "E002",
            DiagnosticCode::MissingScriptFile => "E003",
            DiagnosticCode::InvalidRegex => "E004",
            DiagnosticCode::AccessibilityNotEnabled => "E005",
            DiagnosticCode::FileReadError => "E006",
            DiagnosticCode::DuplicateTrigger => "W001",
            DiagnosticCode::EmptyTrigger => "W002",
            DiagnosticCode::YamlParseWarning => "W003",
        }
    }
}

/// A single diagnostic message
#[derive(Debug, Clone, Serialize)]
pub struct Diagnostic {
    pub level: DiagnosticLevel,
    pub code: DiagnosticCode,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub suggestion: Option<String>,
}

impl Diagnostic {
    fn error(code: DiagnosticCode, message: impl Into<String>) -> Self {
        Self {
            level: DiagnosticLevel::Error,
            code,
            message: message.into(),
            file: None,
            suggestion: None,
        }
    }

    fn warning(code: DiagnosticCode, message: impl Into<String>) -> Self {
        Self {
            level: DiagnosticLevel::Warning,
            code,
            message: message.into(),
            file: None,
            suggestion: None,
        }
    }

    fn with_file(mut self, file: impl Into<PathBuf>) -> Self {
        self.file = Some(file.into());
        self
    }

    fn with_suggestion(mut self, suggestion: impl Into<String>) -> Self {
        self.suggestion = Some(suggestion.into());
        self
    }

    fn format_text(&self) -> String {
        let level_str = match self.level {
            DiagnosticLevel::Error => "ERROR",
            DiagnosticLevel::Warning => "WARNING",
        };

        let mut output = format!("{} [{}]: {}", level_str, self.code.as_str(), self.message);

        if let Some(file) = &self.file {
            output = format!("{}\n  --> {}", output, file.display());
        }

        if let Some(suggestion) = &self.suggestion {
            output = format!("{}\n  hint: {}", output, suggestion);
        }

        output
    }
}

/// Collection of diagnostics with summary
#[derive(Debug, Default, Serialize)]
pub struct DiagnosticReport {
    pub diagnostics: Vec<Diagnostic>,
}

impl DiagnosticReport {
    fn new() -> Self {
        Self {
            diagnostics: Vec::new(),
        }
    }

    fn add(&mut self, diagnostic: Diagnostic) {
        self.diagnostics.push(diagnostic);
    }

    fn error_count(&self) -> usize {
        self.diagnostics
            .iter()
            .filter(|d| d.level == DiagnosticLevel::Error)
            .count()
    }

    fn warning_count(&self) -> usize {
        self.diagnostics
            .iter()
            .filter(|d| d.level == DiagnosticLevel::Warning)
            .count()
    }

    fn has_errors(&self) -> bool {
        self.error_count() > 0
    }

    fn print_text(&self) {
        for diagnostic in &self.diagnostics {
            eprintln!("{}", diagnostic.format_text());
        }

        let errors = self.error_count();
        let warnings = self.warning_count();

        if errors == 0 && warnings == 0 {
            println!("OK: configuration looks good");
        } else {
            eprintln!();
            eprintln!("Summary: {} error(s), {} warning(s)", errors, warnings);
        }
    }

    fn print_json(&self) {
        #[derive(Serialize)]
        struct JsonOutput<'a> {
            diagnostics: &'a [Diagnostic],
            summary: Summary,
        }

        #[derive(Serialize)]
        struct Summary {
            errors: usize,
            warnings: usize,
            success: bool,
        }

        let output = JsonOutput {
            diagnostics: &self.diagnostics,
            summary: Summary {
                errors: self.error_count(),
                warnings: self.warning_count(),
                success: !self.has_errors(),
            },
        };

        if let Ok(json) = serde_json::to_string_pretty(&output) {
            println!("{}", json);
        }
    }
}

/// Output format for doctor command
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputFormat {
    Text,
    Json,
}

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

    // Parse CLI arguments
    let cli_args = args.cli_args.expect("missing cli_args");
    let verbose = cli_args.is_present("verbose");
    let format = match cli_args.value_of("format") {
        Some("json") => OutputFormat::Json,
        _ => OutputFormat::Text,
    };

    let mut report = DiagnosticReport::new();

    // Run all diagnostic checks
    if verbose && format == OutputFormat::Text {
        println!("Running diagnostic checks...\n");
    }

    check_yaml_errors(&mut report, &args.non_fatal_errors, verbose, format);
    check_accessibility(&mut report, verbose, format);

    let loaded_paths = match_store.loaded_paths();
    check_duplicate_triggers(&mut report, &loaded_paths, verbose, format);
    check_empty_triggers(&mut report, &loaded_paths, verbose, format);

    let match_paths: Vec<String> = config_store.get_all_match_paths().into_iter().collect();
    let match_set = match_store.query(&match_paths);

    check_missing_variables(
        &mut report,
        &match_set.matches,
        &match_set.global_vars,
        verbose,
        format,
    );
    check_invalid_regex(&mut report, &match_set.matches, verbose, format);
    check_missing_script_files(&mut report, &match_set.matches, &paths, verbose, format);

    // Output results
    match format {
        OutputFormat::Text => report.print_text(),
        OutputFormat::Json => report.print_json(),
    }

    if report.has_errors() {
        1
    } else {
        0
    }
}

fn check_yaml_errors(
    report: &mut DiagnosticReport,
    non_fatal_errors: &[espanso_config::error::NonFatalErrorSet],
    verbose: bool,
    format: OutputFormat,
) {
    let mut found_issues = false;

    for error_set in non_fatal_errors {
        for record in &error_set.errors {
            found_issues = true;
            let (code, level) = match record.level {
                ErrorLevel::Error => (DiagnosticCode::YamlParseError, DiagnosticLevel::Error),
                ErrorLevel::Warning => (DiagnosticCode::YamlParseWarning, DiagnosticLevel::Warning),
            };

            let diagnostic = Diagnostic {
                level,
                code,
                message: format!("{:#}", record.error),
                file: Some(error_set.file.clone()),
                suggestion: Some(
                    "Check YAML syntax and ensure all required fields are present".to_string(),
                ),
            };
            report.add(diagnostic);
        }
    }

    if !found_issues && verbose && format == OutputFormat::Text {
        println!("OK: match files parsed without YAML errors");
    }
}

fn check_accessibility(report: &mut DiagnosticReport, verbose: bool, format: OutputFormat) {
    if !is_accessibility_enabled() {
        let diagnostic = Diagnostic::error(
            DiagnosticCode::AccessibilityNotEnabled,
            "Accessibility permissions not granted",
        )
        .with_suggestion(
            "On macOS, grant Accessibility permissions in System Preferences > Security & Privacy > Privacy > Accessibility",
        );
        report.add(diagnostic);
    } else if verbose && format == OutputFormat::Text {
        println!("OK: accessibility permissions granted");
    }
}

fn check_duplicate_triggers(
    report: &mut DiagnosticReport,
    paths: &[String],
    verbose: bool,
    format: OutputFormat,
) {
    let (duplicates, read_errors) = find_duplicate_triggers(paths);

    let has_errors = !read_errors.is_empty();
    let has_duplicates = !duplicates.is_empty();

    // Report file read errors
    for (path, error) in read_errors {
        let diagnostic = Diagnostic::error(
            DiagnosticCode::FileReadError,
            format!("Failed to read match file: {}", error),
        )
        .with_file(path);
        report.add(diagnostic);
    }

    // Report duplicate triggers
    for (trigger, trigger_paths) in &duplicates {
        let diagnostic = Diagnostic::warning(
            DiagnosticCode::DuplicateTrigger,
            format!(
                "duplicate trigger '{}' found in: {}",
                trigger,
                trigger_paths.join(", ")
            ),
        )
        .with_suggestion("Consider using unique triggers or consolidating matches into one file");
        report.add(diagnostic);
    }

    if !has_duplicates && !has_errors && verbose && format == OutputFormat::Text {
        println!("OK: no duplicate triggers detected");
    }
}

fn find_duplicate_triggers(
    paths: &[String],
) -> (BTreeMap<String, Vec<String>>, Vec<(String, String)>) {
    let mut triggers: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut errors: Vec<(String, String)> = Vec::new();

    for path in paths {
        match read_match_group_triggers(PathBuf::from(path).as_path()) {
            Ok(triggers_for_file) => {
                for trigger in triggers_for_file {
                    triggers.entry(trigger).or_default().insert(path.clone());
                }
            }
            Err(e) => {
                warn!("Failed to read triggers from {}: {}", path, e);
                errors.push((path.clone(), e.to_string()));
            }
        }
    }

    let duplicates = triggers
        .into_iter()
        .filter_map(|(trigger, paths)| {
            if paths.len() > 1 {
                Some((trigger, paths.into_iter().collect()))
            } else {
                None
            }
        })
        .collect();

    (duplicates, errors)
}

fn check_empty_triggers(
    report: &mut DiagnosticReport,
    paths: &[String],
    verbose: bool,
    format: OutputFormat,
) {
    let mut found_empty = false;

    for path in paths {
        if let Ok(triggers) = read_match_group_triggers(PathBuf::from(path).as_path()) {
            for trigger in &triggers {
                if trigger.trim().is_empty() || trigger.chars().all(char::is_whitespace) {
                    found_empty = true;
                    let diagnostic = Diagnostic::warning(
                        DiagnosticCode::EmptyTrigger,
                        "Match has an empty or whitespace-only trigger",
                    )
                    .with_file(path)
                    .with_suggestion("Remove the empty trigger or provide a valid trigger string");
                    report.add(diagnostic);
                }
            }
        }
    }

    if !found_empty && verbose && format == OutputFormat::Text {
        println!("OK: no empty triggers detected");
    }
}

fn check_missing_variables(
    report: &mut DiagnosticReport,
    matches: &[&Match],
    global_vars: &[&espanso_config::matches::Variable],
    verbose: bool,
    format: OutputFormat,
) {
    let global_names: HashSet<&str> = global_vars.iter().map(|var| var.name.as_str()).collect();
    let mut found_missing = false;

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
            found_missing = true;
            let description = m
                .cause_description()
                .or(m.label.as_deref())
                .unwrap_or("(unnamed match)");

            // Try to suggest similar variable names
            let suggestion = find_similar_variable(&missing, &available);

            let mut diagnostic = Diagnostic::error(
                DiagnosticCode::MissingVariable,
                format!(
                    "match '{}' references missing variables: {}",
                    description,
                    missing.into_iter().collect::<Vec<_>>().join(", ")
                ),
            );

            if let Some(hint) = suggestion {
                diagnostic = diagnostic.with_suggestion(hint);
            } else {
                diagnostic = diagnostic.with_suggestion(
                    "Define the variable in a global_vars section or in the match's vars",
                );
            }

            report.add(diagnostic);
        }
    }

    if !found_missing && verbose && format == OutputFormat::Text {
        println!("OK: no missing variable references detected");
    }
}

/// Find similar variable names for suggestions
fn find_similar_variable(missing: &BTreeSet<String>, available: &HashSet<&str>) -> Option<String> {
    for m in missing {
        for a in available {
            // Simple similarity: case-insensitive match or prefix/suffix match
            if m.to_lowercase() == a.to_lowercase() {
                return Some(format!("Did you mean '{}'?", a));
            }
            if a.contains(m.as_str()) || m.contains(*a) {
                return Some(format!("Did you mean '{}'?", a));
            }
        }
    }
    None
}

fn check_invalid_regex(
    report: &mut DiagnosticReport,
    matches: &[&Match],
    verbose: bool,
    format: OutputFormat,
) {
    let mut found_invalid = false;

    for m in matches {
        if let MatchCause::Regex(RegexCause { regex }) = &m.cause {
            if let Err(e) = Regex::new(regex) {
                found_invalid = true;
                let description = m.label.as_deref().unwrap_or("(unnamed match)");
                let diagnostic = Diagnostic::error(
                    DiagnosticCode::InvalidRegex,
                    format!(
                        "match '{}' has invalid regex '{}': {}",
                        description, regex, e
                    ),
                )
                .with_suggestion(
                    "Check regex syntax at https://docs.rs/regex/latest/regex/#syntax",
                );
                report.add(diagnostic);
            }
        }
    }

    if !found_invalid && verbose && format == OutputFormat::Text {
        println!("OK: all regex patterns are valid");
    }
}

fn check_missing_script_files(
    report: &mut DiagnosticReport,
    matches: &[&Match],
    paths: &Paths,
    verbose: bool,
    format: OutputFormat,
) {
    let mut found_missing = false;

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
                    found_missing = true;
                    let description = m
                        .cause_description()
                        .or(m.label.as_deref())
                        .unwrap_or("(unnamed match)");
                    let diagnostic = Diagnostic::error(
                        DiagnosticCode::MissingScriptFile,
                        format!(
                            "script variable '{}' in match '{}' points to missing file",
                            var.name, description
                        ),
                    )
                    .with_file(resolved)
                    .with_suggestion("Create the script file or update the path");
                    report.add(diagnostic);
                }
            }
        }
    }

    if !found_missing && verbose && format == OutputFormat::Text {
        println!("OK: no missing script files detected");
    }
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
        // Handle BOM markers
        let candidate = candidate.trim_start_matches('\u{feff}');
        // Handle escaped variables
        let candidate = candidate.strip_prefix('\\').unwrap_or(candidate);
        // Extract variable name (first word, before any dot for nested access)
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
        if let Some(stripped) = resolved.strip_prefix('~') {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_variable_names_simple() {
        let vars = extract_variable_names("Hello {{name}}!");
        assert!(vars.contains("name"));
        assert_eq!(vars.len(), 1);
    }

    #[test]
    fn test_extract_variable_names_multiple() {
        let vars = extract_variable_names("{{greeting}} {{name}}, welcome to {{place}}!");
        assert!(vars.contains("greeting"));
        assert!(vars.contains("name"));
        assert!(vars.contains("place"));
        assert_eq!(vars.len(), 3);
    }

    #[test]
    fn test_extract_variable_names_nested_property() {
        let vars = extract_variable_names("Hello {{user.name}}!");
        assert!(vars.contains("user"));
        assert_eq!(vars.len(), 1);
    }

    #[test]
    fn test_extract_variable_names_with_whitespace() {
        let vars = extract_variable_names("Hello {{  name  }}!");
        assert!(vars.contains("name"));
        assert_eq!(vars.len(), 1);
    }

    #[test]
    fn test_extract_variable_names_escaped() {
        let vars = extract_variable_names("Hello {{\\escaped}}!");
        assert!(vars.contains("escaped"));
        assert_eq!(vars.len(), 1);
    }

    #[test]
    fn test_extract_variable_names_empty() {
        let vars = extract_variable_names("Hello world!");
        assert!(vars.is_empty());
    }

    #[test]
    fn test_extract_variable_names_unclosed() {
        let vars = extract_variable_names("Hello {{name without closing");
        assert!(vars.is_empty());
    }

    #[test]
    fn test_extract_variable_names_with_bom() {
        let vars = extract_variable_names("Hello {{\u{feff}name}}!");
        assert!(vars.contains("name"));
        assert_eq!(vars.len(), 1);
    }

    #[test]
    fn test_diagnostic_format_error() {
        let diag = Diagnostic::error(DiagnosticCode::MissingVariable, "test error");
        let formatted = diag.format_text();
        assert!(formatted.contains("ERROR"));
        assert!(formatted.contains("[E002]"));
        assert!(formatted.contains("test error"));
    }

    #[test]
    fn test_diagnostic_format_warning() {
        let diag = Diagnostic::warning(DiagnosticCode::DuplicateTrigger, "test warning");
        let formatted = diag.format_text();
        assert!(formatted.contains("WARNING"));
        assert!(formatted.contains("[W001]"));
        assert!(formatted.contains("test warning"));
    }

    #[test]
    fn test_diagnostic_with_file_and_suggestion() {
        let diag = Diagnostic::error(DiagnosticCode::MissingScriptFile, "missing file")
            .with_file("/path/to/file.sh")
            .with_suggestion("Create the file");
        let formatted = diag.format_text();
        assert!(formatted.contains("/path/to/file.sh"));
        assert!(formatted.contains("hint: Create the file"));
    }

    #[test]
    fn test_diagnostic_report_counts() {
        let mut report = DiagnosticReport::new();
        report.add(Diagnostic::error(
            DiagnosticCode::MissingVariable,
            "error 1",
        ));
        report.add(Diagnostic::error(
            DiagnosticCode::MissingVariable,
            "error 2",
        ));
        report.add(Diagnostic::warning(
            DiagnosticCode::DuplicateTrigger,
            "warning 1",
        ));

        assert_eq!(report.error_count(), 2);
        assert_eq!(report.warning_count(), 1);
        assert!(report.has_errors());
    }

    #[test]
    fn test_find_similar_variable() {
        let mut missing = BTreeSet::new();
        missing.insert("Name".to_string());

        let mut available = HashSet::new();
        available.insert("name");

        let suggestion = find_similar_variable(&missing, &available);
        assert!(suggestion.is_some());
        assert!(suggestion.unwrap().contains("name"));
    }

    #[test]
    fn test_extract_variable_names_from_value_string() {
        let value = Value::String("Hello {{var}}".to_string());
        let vars = extract_variable_names_from_value(&value);
        assert!(vars.contains("var"));
    }

    #[test]
    fn test_extract_variable_names_from_value_array() {
        let value = Value::Array(vec![
            Value::String("{{var1}}".to_string()),
            Value::String("{{var2}}".to_string()),
        ]);
        let vars = extract_variable_names_from_value(&value);
        assert!(vars.contains("var1"));
        assert!(vars.contains("var2"));
    }

    #[test]
    fn test_extract_variable_names_from_value_null() {
        let value = Value::Null;
        let vars = extract_variable_names_from_value(&value);
        assert!(vars.is_empty());
    }
}
