//! Internal file dragging; destinations consume a release once, after all panels render.
use crate::playlist::Item;
use eframe::egui::{self, Color32, Context, Id, LayerId, Order, Pos2, Rect, Response, Stroke};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Origin {
    Browser,
    Session,
    Playlist(usize),
}

#[derive(Clone)]
pub struct Music {
    pub item: Item,
    pub origin: Origin,
}

pub fn source(response: &Response, item: Item, origin: Origin) {
    response.dnd_set_drag_payload(Music { item, origin });
    response.clone().on_hover_cursor(egui::CursorIcon::Grab);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    Deck(usize),
    Session,
    /// None appends; Some inserts before the given item.
    Playlist(Option<usize>),
}

struct Zone {
    target: Target,
    rect: Rect,
    layer: LayerId,
    line: Option<f32>,
}

#[derive(Default)]
pub struct Zones {
    zones: Vec<Zone>,
}

impl Zones {
    pub fn add(&mut self, target: Target, rect: Rect, layer: LayerId) {
        self.zones.push(Zone {
            target,
            rect,
            layer,
            line: None,
        });
    }

    pub fn playlist_row(&mut self, ui: &egui::Ui, rect: Rect, index: usize) {
        let below = ui.input(|i| {
            i.pointer
                .latest_pos()
                .is_some_and(|p| p.y >= rect.center().y)
        });
        let clipped = rect.intersect(ui.clip_rect());
        self.zones.push(Zone {
            target: Target::Playlist(Some(index + usize::from(below))),
            rect: clipped,
            layer: ui.layer_id(),
            line: Some(if below {
                clipped.bottom()
            } else {
                clipped.top()
            }),
        });
    }

    fn zone(&self, ctx: &Context) -> Option<&Zone> {
        let pointer = ctx.input(|i| i.pointer.latest_pos())?;
        let layer = ctx.layer_id_at(pointer)?;
        self.zones
            .iter()
            .rev()
            .find(|zone| zone.rect.contains(pointer) && zone.layer == layer)
    }

    pub fn target(&self, ctx: &Context) -> Option<Target> {
        self.zone(ctx).map(|z| z.target)
    }

    pub fn finish(&self, ctx: &Context) -> Option<(Target, Music)> {
        let payload = egui::DragAndDrop::payload::<Music>(ctx)?;
        let zone = self.zone(ctx);
        ctx.set_cursor_icon(if zone.is_some() {
            egui::CursorIcon::Copy
        } else {
            egui::CursorIcon::NoDrop
        });
        if let Some(zone) = zone {
            let color = Color32::from_rgb(174, 119, 255);
            let painter = ctx.layer_painter(zone.layer);
            if let Some(y) = zone.line {
                painter.line_segment(
                    [
                        Pos2::new(zone.rect.left(), y),
                        Pos2::new(zone.rect.right(), y),
                    ],
                    Stroke::new(3.0_f32, color),
                );
            } else {
                painter.rect_filled(zone.rect.shrink(2.0), 3.0, color.gamma_multiply(0.10));
                painter.rect_stroke(
                    zone.rect.shrink(2.0),
                    3.0,
                    Stroke::new(2.0_f32, color),
                    egui::StrokeKind::Inside,
                );
            }
        }
        if let Some(pointer) = ctx.input(|i| i.pointer.latest_pos()) {
            let painter =
                ctx.layer_painter(LayerId::new(Order::Tooltip, Id::new("music-drag-preview")));
            let title: String = payload.item.title.chars().take(55).collect();
            let galley =
                painter.layout_no_wrap(title, egui::FontId::proportional(13.0), Color32::WHITE);
            let size = galley.size() + egui::vec2(16.0, 12.0);
            let screen = ctx.content_rect();
            let pos = (pointer + egui::vec2(14.0, 18.0))
                .min(screen.max - size)
                .max(screen.min);
            painter.rect_filled(
                Rect::from_min_size(pos, size),
                4.0,
                Color32::from_rgb(43, 31, 60),
            );
            painter.galley(pos + egui::vec2(8.0, 6.0), galley, Color32::WHITE);
        }
        if ctx.input(|i| i.pointer.button_released(egui::PointerButton::Primary))
            && let Some(zone) = zone
        {
            let music = egui::DragAndDrop::take_payload::<Music>(ctx)?;
            return Some((zone.target, (*music).clone()));
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{Event, Modifiers, PointerButton, RawInput, Sense};
    use std::path::PathBuf;

    fn frame(ctx: &Context, events: Vec<Event>) -> Option<(Target, Music)> {
        let mut dropped = None;
        let _ = ctx.run(
            RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(700.0, 400.0))),
                events,
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let response = ui.allocate_rect(
                        Rect::from_min_max(egui::pos2(15.0, 15.0), egui::pos2(180.0, 60.0)),
                        Sense::click_and_drag(),
                    );
                    source(
                        &response,
                        Item::new(PathBuf::from("test.mp3"), None),
                        Origin::Browser,
                    );
                    let mut zones = Zones::default();
                    zones.add(
                        Target::Deck(0),
                        Rect::from_min_max(egui::pos2(200.0, 10.0), egui::pos2(400.0, 160.0)),
                        ui.layer_id(),
                    );
                    zones.add(
                        Target::Deck(1),
                        Rect::from_min_max(egui::pos2(420.0, 10.0), egui::pos2(680.0, 160.0)),
                        ui.layer_id(),
                    );
                    zones.add(
                        Target::Session,
                        Rect::from_min_max(egui::pos2(200.0, 200.0), egui::pos2(400.0, 380.0)),
                        ui.layer_id(),
                    );
                    zones.add(
                        Target::Playlist(None),
                        Rect::from_min_max(egui::pos2(420.0, 200.0), egui::pos2(680.0, 380.0)),
                        ui.layer_id(),
                    );
                    dropped = zones.finish(ctx);
                });
            },
        );
        dropped
    }

    fn button(pos: Pos2, pressed: bool) -> Event {
        Event::PointerButton {
            pos,
            button: PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        }
    }

    fn start(ctx: &Context) {
        frame(ctx, vec![]);
        frame(
            ctx,
            vec![
                Event::PointerMoved(egui::pos2(40.0, 35.0)),
                button(egui::pos2(40.0, 35.0), true),
            ],
        );
        assert!(frame(ctx, vec![Event::PointerMoved(egui::pos2(75.0, 35.0))]).is_none());
        assert!(egui::DragAndDrop::has_payload_of_type::<Music>(ctx));
    }

    #[test]
    fn pointer_drag_releases_once_on_the_correct_deck_or_list() {
        for (pos, target) in [
            (egui::pos2(300.0, 100.0), Target::Deck(0)),
            (egui::pos2(500.0, 100.0), Target::Deck(1)),
            (egui::pos2(300.0, 300.0), Target::Session),
            (egui::pos2(500.0, 300.0), Target::Playlist(None)),
        ] {
            let ctx = Context::default();
            start(&ctx);
            assert!(frame(&ctx, vec![Event::PointerMoved(pos)]).is_none());
            let (actual, music) = frame(&ctx, vec![button(pos, false)]).unwrap();
            assert_eq!(actual, target);
            assert_eq!(music.item.path, PathBuf::from("test.mp3"));
            assert_eq!(music.origin, Origin::Browser);
            assert!(frame(&ctx, vec![]).is_none());
            assert!(!egui::DragAndDrop::has_any_payload(&ctx));
        }
    }

    #[test]
    fn release_outside_a_destination_or_escape_performs_no_drop() {
        let ctx = Context::default();
        start(&ctx);
        let outside = egui::pos2(100.0, 300.0);
        assert!(
            frame(
                &ctx,
                vec![Event::PointerMoved(outside), button(outside, false)]
            )
            .is_none()
        );
        start(&ctx);
        frame(
            &ctx,
            vec![Event::Key {
                key: egui::Key::Escape,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: Modifiers::NONE,
            }],
        );
        let deck = egui::pos2(300.0, 100.0);
        assert!(frame(&ctx, vec![Event::PointerMoved(deck), button(deck, false)]).is_none());
    }
}
