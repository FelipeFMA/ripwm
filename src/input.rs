use smithay::{
    backend::input::{
        AbsolutePositionEvent, Axis, AxisSource, ButtonState, Event, InputBackend, InputEvent,
        KeyState, KeyboardKeyEvent, PointerAxisEvent, PointerButtonEvent, PointerMotionEvent,
    },
    backend::session::Session,
    input::{
        keyboard::{FilterResult, Keysym, keysyms as xkb},
        pointer::{AxisFrame, ButtonEvent, MotionEvent},
    },
    reexports::wayland_server::protocol::wl_surface::WlSurface,
    utils::{Rectangle, SERIAL_COUNTER},
};
use std::process::Command;

use crate::state::Smallvil;

enum KeyAction {
    Forward,
    Quit,
    VtSwitch(i32),
    RunFoot,
}

#[allow(clippy::cast_possible_truncation)]
fn f64_to_i32_saturating(value: f64) -> i32 {
    if value.is_nan() {
        0
    } else if value > f64::from(i32::MAX) {
        i32::MAX
    } else if value < f64::from(i32::MIN) {
        i32::MIN
    } else {
        value as i32
    }
}

impl Smallvil {
    #[allow(clippy::too_many_lines)]
    pub fn process_input_event<I: InputBackend>(&mut self, event: InputEvent<I>) {
        match event {
            InputEvent::Keyboard { event, .. } => {
                let serial = SERIAL_COUNTER.next_serial();
                let time = Event::time_msec(&event);

                let Some(keyboard) = self.seat.get_keyboard() else {
                    tracing::warn!("Keyboard event received without keyboard in seat");
                    return;
                };

                let action = keyboard
                    .input::<KeyAction, _>(
                        self,
                        event.key_code(),
                        event.state(),
                        serial,
                        time,
                        |_, modifiers, handle| {
                            if event.state() == KeyState::Pressed {
                                let keysym = handle.modified_sym();

                                if (modifiers.ctrl && modifiers.alt && keysym == Keysym::BackSpace)
                                    || keysym == Keysym::Escape
                                {
                                    return FilterResult::Intercept(KeyAction::Quit);
                                }

                                if (xkb::KEY_XF86Switch_VT_1..=xkb::KEY_XF86Switch_VT_12)
                                    .contains(&keysym.raw())
                                {
                                    let vt =
                                        i32::try_from(keysym.raw() - xkb::KEY_XF86Switch_VT_1 + 1)
                                            .unwrap_or(i32::MAX);
                                    return FilterResult::Intercept(KeyAction::VtSwitch(vt));
                                }

                                if modifiers.logo && keysym == Keysym::Return {
                                    return FilterResult::Intercept(KeyAction::RunFoot);
                                }
                            }

                            FilterResult::Forward
                        },
                    )
                    .unwrap_or(KeyAction::Forward);

                match action {
                    KeyAction::Quit => self.loop_signal.stop(),
                    KeyAction::VtSwitch(vt) => {
                        if let Some(udev) = self.udev.as_mut()
                            && let Err(err) = udev.session.change_vt(vt)
                        {
                            tracing::error!("Error switching VT to {vt}: {err}");
                        }
                    }
                    KeyAction::RunFoot => {
                        if let Err(err) = Command::new("foot").spawn() {
                            tracing::error!("Failed to start foot: {err}");
                        }
                    }
                    KeyAction::Forward => {}
                }
            }
            InputEvent::PointerMotion { event, .. } => {
                let Some(pointer) = self.seat.get_pointer() else {
                    tracing::warn!("Pointer motion received without pointer in seat");
                    return;
                };

                let mut pos = pointer.current_location() + event.delta();

                if let Some(output) = self.space.outputs().next()
                    && let Some(output_geo) = self.space.output_geometry(output)
                {
                    pos = pos.constrain(Rectangle::new(output_geo.loc, output_geo.size).to_f64());
                }

                let serial = SERIAL_COUNTER.next_serial();
                let under = self.surface_under(pos);

                pointer.motion(
                    self,
                    under,
                    &MotionEvent { location: pos, serial, time: event.time_msec() },
                );
                pointer.frame(self);
            }
            InputEvent::PointerMotionAbsolute { event, .. } => {
                let Some(output) = self.space.outputs().next() else {
                    return;
                };

                let Some(output_geo) = self.space.output_geometry(output) else {
                    return;
                };

                let pos = event.position_transformed(output_geo.size) + output_geo.loc.to_f64();

                let serial = SERIAL_COUNTER.next_serial();

                let Some(pointer) = self.seat.get_pointer() else {
                    tracing::warn!("Pointer absolute motion received without pointer in seat");
                    return;
                };

                let under = self.surface_under(pos);

                pointer.motion(
                    self,
                    under,
                    &MotionEvent { location: pos, serial, time: event.time_msec() },
                );
                pointer.frame(self);
            }
            InputEvent::PointerButton { event, .. } => {
                let Some(pointer) = self.seat.get_pointer() else {
                    tracing::warn!("Pointer button received without pointer in seat");
                    return;
                };
                let Some(keyboard) = self.seat.get_keyboard() else {
                    tracing::warn!("Pointer button received without keyboard in seat");
                    return;
                };

                let serial = SERIAL_COUNTER.next_serial();

                let button = event.button_code();

                let button_state = event.state();

                if ButtonState::Pressed == button_state && !pointer.is_grabbed() {
                    if let Some((window, _loc)) = self
                        .space
                        .element_under(pointer.current_location())
                        .map(|(w, l)| (w.clone(), l))
                    {
                        self.space.raise_element(&window, true);
                        let Some(toplevel) = window.toplevel() else {
                            tracing::warn!("Window without toplevel cannot receive focus");
                            pointer.button(
                                self,
                                &ButtonEvent {
                                    button,
                                    state: button_state,
                                    serial,
                                    time: event.time_msec(),
                                },
                            );
                            pointer.frame(self);
                            return;
                        };
                        keyboard.set_focus(self, Some(toplevel.wl_surface().clone()), serial);
                        self.space.elements().for_each(|window| {
                            if let Some(toplevel) = window.toplevel() {
                                toplevel.send_pending_configure();
                            }
                        });
                    } else {
                        self.space.elements().for_each(|window| {
                            window.set_activated(false);
                            if let Some(toplevel) = window.toplevel() {
                                toplevel.send_pending_configure();
                            }
                        });
                        keyboard.set_focus(self, Option::<WlSurface>::None, serial);
                    }
                }

                pointer.button(
                    self,
                    &ButtonEvent { button, state: button_state, serial, time: event.time_msec() },
                );
                pointer.frame(self);
            }
            InputEvent::PointerAxis { event, .. } => {
                let source = event.source();

                let horizontal_amount = event.amount(Axis::Horizontal).unwrap_or_else(|| {
                    event.amount_v120(Axis::Horizontal).unwrap_or(0.0) * 15.0 / 120.
                });
                let vertical_amount = event.amount(Axis::Vertical).unwrap_or_else(|| {
                    event.amount_v120(Axis::Vertical).unwrap_or(0.0) * 15.0 / 120.
                });
                let horizontal_amount_discrete = event.amount_v120(Axis::Horizontal);
                let vertical_amount_discrete = event.amount_v120(Axis::Vertical);

                let mut frame = AxisFrame::new(event.time_msec()).source(source);
                if horizontal_amount != 0.0 {
                    frame = frame.value(Axis::Horizontal, horizontal_amount);
                    if let Some(discrete) = horizontal_amount_discrete {
                        frame = frame.v120(Axis::Horizontal, f64_to_i32_saturating(discrete));
                    }
                }
                if vertical_amount != 0.0 {
                    frame = frame.value(Axis::Vertical, vertical_amount);
                    if let Some(discrete) = vertical_amount_discrete {
                        frame = frame.v120(Axis::Vertical, f64_to_i32_saturating(discrete));
                    }
                }

                if source == AxisSource::Finger {
                    if event.amount(Axis::Horizontal) == Some(0.0) {
                        frame = frame.stop(Axis::Horizontal);
                    }
                    if event.amount(Axis::Vertical) == Some(0.0) {
                        frame = frame.stop(Axis::Vertical);
                    }
                }

                let Some(pointer) = self.seat.get_pointer() else {
                    tracing::warn!("Pointer axis received without pointer in seat");
                    return;
                };
                pointer.axis(self, frame);
                pointer.frame(self);
            }
            _ => {}
        }

        if self.udev.is_some() {
            self.request_redraw_all();
        }
    }
}
