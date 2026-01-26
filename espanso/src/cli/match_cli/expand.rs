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

use std::io::{self, IsTerminal, Read};

use anyhow::{bail, Context, Result};
use espanso_config::matches::{Match, MatchCause, MatchEffect, UpperCasingStyle};
use espanso_render::Renderer;
use espanso_render::{CasingStyle, Context as RenderContext, RenderOptions, Template, Variable};

use super::ExpandArgs;

pub fn expand_main(args: ExpandArgs) -> i32 {
    match expand_main_result(args) {
        Ok(()) => 0,
        Err(err) => {
            eprintln!("ERROR: {err:#}");
            1
        }
    }
}

fn expand_main_result(args: ExpandArgs) -> Result<()> {
    let cli_args = args.cli_args;
    let config_store = args.config_store;
    let match_store = args.match_store;
    let paths = args.paths;

    // Get input text with priority: stdin > file > argument
    let input_text = get_input_text(cli_args)?;

    if input_text.is_empty() {
        bail!("no input text provided. Use: stdin pipe, -f/--file, or text argument");
    }

    let config = config_store.default();
    let match_set = match_store.query(config.match_paths());

    // Build trigger map for efficient lookup
    let trigger_map: Vec<(&str, &Match)> = match_set
        .matches
        .iter()
        .flat_map(|m| {
            if let MatchCause::Trigger(cause) = &m.cause {
                cause
                    .triggers
                    .iter()
                    .map(|t| (t.as_str(), *m))
                    .collect::<Vec<_>>()
            } else {
                vec![]
            }
        })
        .collect();

    // Sort by trigger length (longest first) to match longer triggers before shorter ones
    let mut sorted_triggers: Vec<(&str, &Match)> = trigger_map;
    sorted_triggers.sort_by(|a, b| b.0.len().cmp(&a.0.len()));

    // Build templates for rendering
    let templates: Vec<Template> = match_set
        .matches
        .iter()
        .filter_map(|m| convert_to_template(m))
        .collect();
    let template_refs: Vec<&Template> = templates.iter().collect();

    let global_vars: Vec<Variable> = match_set
        .global_vars
        .iter()
        .copied()
        .map(convert_var)
        .collect();
    let global_var_refs: Vec<&Variable> = global_vars.iter().collect();

    let context = RenderContext {
        global_vars: global_var_refs,
        templates: template_refs,
    };

    // Create renderer with all extensions
    let locale_provider = espanso_render::extension::date::DefaultLocaleProvider::new();
    let date_extension = espanso_render::extension::date::DateExtension::new(&locale_provider);
    let echo_extension = espanso_render::extension::echo::EchoExtension::new();
    let random_extension = espanso_render::extension::random::RandomExtension::new();
    let home_path = dirs::home_dir().context("unable to obtain home dir path")?;
    let script_extension = espanso_render::extension::script::ScriptExtension::new(
        &paths.config,
        &home_path,
        &paths.packages,
    );
    let shell_extension = espanso_render::extension::shell::ShellExtension::new(&paths.config);
    let renderer = espanso_render::create(vec![
        &date_extension,
        &echo_extension,
        &random_extension,
        &script_extension,
        &shell_extension,
    ]);

    // Process input text and replace triggers
    let output = expand_triggers(&input_text, &sorted_triggers, &renderer, &context)?;

    print!("{output}");

    Ok(())
}

fn get_input_text(cli_args: &clap::ArgMatches) -> Result<String> {
    // Priority 1: stdin (if not a terminal)
    if !io::stdin().is_terminal() {
        let mut buffer = String::new();
        io::stdin()
            .read_to_string(&mut buffer)
            .context("failed to read from stdin")?;
        return Ok(buffer);
    }

    // Priority 2: file
    if let Some(file_path) = cli_args.value_of("file") {
        let content = std::fs::read_to_string(file_path)
            .context(format!("failed to read file: {file_path}"))?;
        return Ok(content);
    }

    // Priority 3: text argument
    if let Some(values) = cli_args.values_of("text") {
        let text: Vec<&str> = values.collect();
        // Process escape sequences like \n
        let joined = text.join(" ");
        let processed = process_escape_sequences(&joined);
        return Ok(processed);
    }

    Ok(String::new())
}

fn process_escape_sequences(input: &str) -> String {
    let mut result = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();

    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.peek() {
                Some('n') => {
                    result.push('\n');
                    chars.next();
                }
                Some('t') => {
                    result.push('\t');
                    chars.next();
                }
                Some('r') => {
                    result.push('\r');
                    chars.next();
                }
                Some('\\') => {
                    result.push('\\');
                    chars.next();
                }
                _ => result.push(c),
            }
        } else {
            result.push(c);
        }
    }

    result
}

fn expand_triggers(
    input: &str,
    triggers: &[(&str, &Match)],
    renderer: &impl Renderer,
    context: &RenderContext,
) -> Result<String> {
    let mut result = input.to_string();

    // Keep expanding until no more triggers are found (handles nested triggers)
    let mut changed = true;
    let mut iterations = 0;
    const MAX_ITERATIONS: usize = 100; // Prevent infinite loops

    while changed && iterations < MAX_ITERATIONS {
        changed = false;
        iterations += 1;

        for (trigger, match_ref) in triggers {
            if result.contains(*trigger) {
                if let Some(template) = convert_to_template(match_ref) {
                    let options = RenderOptions {
                        casing_style: calculate_casing_style(match_ref, trigger),
                    };

                    let rendered = match renderer.render(&template, context, &options) {
                        espanso_render::RenderResult::Success(body) => body,
                        espanso_render::RenderResult::Aborted => continue,
                        espanso_render::RenderResult::Error(err) => {
                            return Err(err.context(format!("failed to render trigger '{trigger}'")))
                        }
                    };

                    result = result.replace(*trigger, &rendered);
                    changed = true;
                }
            }
        }
    }

    if iterations >= MAX_ITERATIONS {
        eprintln!("WARNING: max iterations reached, possible circular trigger reference");
    }

    Ok(result)
}

fn convert_to_template(m: &Match) -> Option<Template> {
    if let MatchEffect::Text(text_effect) = &m.effect {
        let ids = if let MatchCause::Trigger(cause) = &m.cause {
            cause.triggers.clone()
        } else {
            Vec::new()
        };

        Some(Template {
            ids,
            body: text_effect.replace.clone(),
            vars: text_effect.vars.iter().map(convert_var).collect(),
        })
    } else {
        None
    }
}

fn convert_var(var: &espanso_config::matches::Variable) -> espanso_render::Variable {
    Variable {
        name: var.name.clone(),
        var_type: var.var_type.clone(),
        params: convert_params(var.params.clone()),
        inject_vars: var.inject_vars,
        depends_on: var.depends_on.clone(),
    }
}

fn convert_params(params: espanso_config::matches::Params) -> espanso_render::Params {
    let mut new_params = espanso_render::Params::new();
    for (key, value) in params {
        new_params.insert(key, convert_value(value));
    }
    new_params
}

fn convert_value(value: espanso_config::matches::Value) -> espanso_render::Value {
    match value {
        espanso_config::matches::Value::Null => espanso_render::Value::Null,
        espanso_config::matches::Value::Bool(v) => espanso_render::Value::Bool(v),
        espanso_config::matches::Value::Number(n) => match n {
            espanso_config::matches::Number::Integer(i) => {
                espanso_render::Value::Number(espanso_render::Number::Integer(i))
            }
            espanso_config::matches::Number::Float(f) => {
                espanso_render::Value::Number(espanso_render::Number::Float(f.into_inner()))
            }
        },
        espanso_config::matches::Value::String(s) => espanso_render::Value::String(s),
        espanso_config::matches::Value::Array(v) => {
            espanso_render::Value::Array(v.into_iter().map(convert_value).collect())
        }
        espanso_config::matches::Value::Object(params) => {
            espanso_render::Value::Object(convert_params(params))
        }
    }
}

fn calculate_casing_style(m: &Match, trigger: &str) -> CasingStyle {
    let MatchCause::Trigger(cause) = &m.cause else {
        return CasingStyle::None;
    };

    if !cause.propagate_case {
        return CasingStyle::None;
    }

    let mut first_alphabetic = None;
    let mut second_alphabetic = None;

    for c in trigger.chars() {
        if c.is_alphabetic() {
            if first_alphabetic.is_none() {
                first_alphabetic = Some(c);
            } else if second_alphabetic.is_none() {
                second_alphabetic = Some(c);
            } else {
                break;
            }
        }
    }

    match (first_alphabetic, second_alphabetic) {
        (Some(first), Some(second)) => {
            if first.is_uppercase() {
                if second.is_uppercase() {
                    CasingStyle::Uppercase
                } else {
                    match cause.uppercase_style {
                        UpperCasingStyle::CapitalizeWords => CasingStyle::CapitalizeWords,
                        _ => CasingStyle::Capitalize,
                    }
                }
            } else {
                CasingStyle::None
            }
        }
        (Some(first), None) => {
            if first.is_uppercase() {
                match cause.uppercase_style {
                    UpperCasingStyle::Capitalize => CasingStyle::Capitalize,
                    UpperCasingStyle::CapitalizeWords => CasingStyle::CapitalizeWords,
                    _ => CasingStyle::Uppercase,
                }
            } else {
                CasingStyle::None
            }
        }
        _ => CasingStyle::None,
    }
}
