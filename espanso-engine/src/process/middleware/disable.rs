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

use std::{
    cell::RefCell,
    time::{Duration, Instant},
};

use log::info;

use super::super::Middleware;
use crate::event::{
    input::{Key, KeyboardEvent, Status, Variant},
    Event, EventType,
};

pub struct DisableOptions {
    pub toggle_key: Option<Key>,
    pub toggle_key_variant: Option<Variant>,
    pub toggle_key_maximum_window: Duration,
    pub toggle_key_press_count: u32,
    // TODO: toggle shortcut?
}

pub struct DisableMiddleware {
    enabled: RefCell<bool>,
    last_toggle_press: RefCell<Option<Instant>>,
    toggle_press_count: RefCell<u32>,
    options: DisableOptions,
}

impl DisableMiddleware {
    pub fn new(options: DisableOptions) -> Self {
        Self {
            enabled: RefCell::new(true),
            last_toggle_press: RefCell::new(None),
            toggle_press_count: RefCell::new(0),
            options,
        }
    }
}

impl Middleware for DisableMiddleware {
    fn name(&self) -> &'static str {
        "disable"
    }

    fn next(&self, event: Event, dispatch: &mut dyn FnMut(Event)) -> Event {
        let mut has_status_changed = false;
        let mut enabled = self.enabled.borrow_mut();

        match &event.etype {
            EventType::Keyboard(m_event) => {
                if m_event.status == Status::Released {
                    let mut last_toggle_press = self.last_toggle_press.borrow_mut();
                    let mut toggle_press_count = self.toggle_press_count.borrow_mut();
                    if is_toggle_key(m_event, &self.options) {
                        let within_window = last_toggle_press
                            .map(|t| t.elapsed() < self.options.toggle_key_maximum_window)
                            .unwrap_or(false);

                        if within_window {
                            *toggle_press_count += 1;
                        } else {
                            *toggle_press_count = 1;
                        }
                        *last_toggle_press = Some(Instant::now());

                        if *toggle_press_count >= self.options.toggle_key_press_count {
                            *enabled = !*enabled;
                            *last_toggle_press = None;
                            *toggle_press_count = 0;
                            has_status_changed = true;
                        }
                    } else {
                        // If another key is pressed (not the toggle key), we should reset the window
                        // For more information, see: https://github.com/espanso/espanso/issues/815
                        *last_toggle_press = None;
                        *toggle_press_count = 0;
                    }
                }
            }
            EventType::EnableRequest => {
                *enabled = true;
                has_status_changed = true;
            }
            EventType::DisableRequest => {
                *enabled = false;
                has_status_changed = true;
            }
            EventType::ToggleRequest => {
                *enabled = !*enabled;
                has_status_changed = true;
            }
            _ => {}
        }

        if has_status_changed {
            info!("toggled enabled state, is_enabled = {}", *enabled);
            dispatch(Event::caused_by(
                event.source_id,
                if *enabled {
                    EventType::Enabled
                } else {
                    EventType::Disabled
                },
            ));
        }

        // Block keyboard events when disabled
        if let EventType::Keyboard(_) = &event.etype {
            if !*enabled {
                return Event::caused_by(event.source_id, EventType::NOOP);
            }
        }
        // TODO: also ignore hotkey and mouse events

        event
    }
}

fn is_toggle_key(event: &KeyboardEvent, options: &DisableOptions) -> bool {
    if options
        .toggle_key
        .as_ref()
        .is_some_and(|key| key == &event.key)
    {
        if let (Some(variant), Some(e_variant)) = (&options.toggle_key_variant, &event.variant) {
            variant == e_variant
        } else {
            true
        }
    } else {
        false
    }
}
