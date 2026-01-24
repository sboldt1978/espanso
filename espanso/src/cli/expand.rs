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

use anyhow::{bail, Context as AnyhowContext, Result};
use espanso_config::{
    config::AppProperties,
    matches::{Match, MatchCause, MatchEffect, UpperCasingStyle},
};
use espanso_render::Renderer;
use espanso_render::{CasingStyle, Context as RenderContext, RenderOptions, Template, Variable};

use super::{CliModule, CliModuleArgs};

pub fn new() -> CliModule {
    CliModule {
        requires_paths: true,
        requires_config: true,
        subcommand: "expand".to_string(),
        entry: expand_main,
        ..Default::default()
    }
}

fn expand_main(args: CliModuleArgs) -> i32 {
    match expand_main_result(args) {
        Ok(()) => 0,
        Err(err) => {
            eprintln!("ERROR: {err:#}");
            1
        }
    }
}

fn expand_main_result(args: CliModuleArgs) -> Result<()> {
    let cli_args = args.cli_args.expect("missing cli_args");
    let config_store = args.config_store.expect("missing config_store");
    let match_store = args.match_store.expect("missing match_store");
    let paths = args.paths.expect("missing paths");

    let trigger = cli_args
        .value_of("trigger")
        .filter(|value| !value.is_empty())
        .context("trigger cannot be empty")?;

    if !cli_args.is_present("dry-run") {
        bail!("use --dry-run to avoid injecting text into the OS");
    }

    let config = config_store.active(&AppProperties {
        title: None,
        class: None,
        exec: None,
    });
    let match_set = match_store.query(config.match_paths());

    let candidates = matches_for_trigger(&match_set.matches, trigger);
    let match_ref = match candidates.as_slice() {
        [] => bail!("no match found for trigger '{trigger}'"),
        [single] => *single,
        _ => bail!(
            "multiple matches found for trigger '{trigger}', unable to choose deterministically"
        ),
    };

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

    let template =
        convert_to_template(match_ref).context("match does not have a text effect to expand")?;

    let options = RenderOptions {
        casing_style: calculate_casing_style(match_ref, trigger),
    };

    let rendered = match renderer.render(&template, &context, &options) {
        espanso_render::RenderResult::Success(body) => body,
        espanso_render::RenderResult::Aborted => bail!("rendering aborted"),
        espanso_render::RenderResult::Error(err) => return Err(err),
    };

    println!("{rendered}");

    Ok(())
}

fn matches_for_trigger<'a>(matches: &[&'a Match], trigger: &str) -> Vec<&'a Match> {
    matches
        .iter()
        .copied()
        .filter(|m| match &m.cause {
            MatchCause::Trigger(cause) => cause.triggers.iter().any(|t| t == trigger),
            _ => false,
        })
        .collect()
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
