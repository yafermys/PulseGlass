use egui::{Color32, FontId, Pos2, Rect, Rounding, Sense, Stroke, Ui, Vec2};
use std::f32::consts::PI;

pub fn draw_circular_gauge(
    ui: &mut Ui,
    size: f32,
    value_pct: f32,
    center_label: &str,
    sub_text: &str,
    normal_color: Color32,
    warn_color: Color32,
    warn_threshold: f32,
) {
    let (rect, _resp) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
    let painter = ui.painter_at(rect);
    let center = rect.center();
    let radius = (size * 0.5) - 5.0;
    let stroke_w = (size * 0.085).clamp(4.0, 7.0);

    // Background track ring
    painter.circle_stroke(
        center,
        radius,
        Stroke::new(stroke_w, Color32::from_rgba_unmultiplied(255, 255, 255, 28)),
    );

    // Active arc
    let pct = value_pct.clamp(0.0, 100.0);
    let arc_color = if pct >= warn_threshold {
        warn_color
    } else {
        normal_color
    };

    if pct > 0.5 {
        let steps = ((pct / 100.0) * 48.0).ceil().max(4.0) as usize;
        let start_angle = -PI / 2.0;
        let sweep = (pct / 100.0) * 2.0 * PI;
        let mut pts = Vec::with_capacity(steps + 1);
        for i in 0..=steps {
            let a = start_angle + sweep * (i as f32 / steps as f32);
            pts.push(Pos2::new(
                center.x + radius * a.cos(),
                center.y + radius * a.sin(),
            ));
        }
        painter.add(egui::Shape::line(pts, Stroke::new(stroke_w, arc_color)));
    }

    // Percentage value
    painter.text(
        Pos2::new(center.x, center.y - size * 0.10),
        egui::Align2::CENTER_CENTER,
        format!("{:.0}%", pct),
        FontId::proportional((size * 0.21).clamp(11.0, 18.0)),
        Color32::from_rgb(255, 241, 235),
    );

    // Sub-text (temp / GB)
    painter.text(
        Pos2::new(center.x, center.y + size * 0.16),
        egui::Align2::CENTER_CENTER,
        sub_text,
        FontId::proportional((size * 0.145).clamp(9.0, 12.0)),
        normal_color,
    );

    let _ = center_label;
}

pub fn glass_card<R>(
    ui: &mut Ui,
    border_accent: Color32,
    add_contents: impl FnOnce(&mut Ui) -> R,
) -> egui::InnerResponse<R> {
    egui::Frame::none()
        .fill(Color32::from_rgba_unmultiplied(255, 255, 255, 16))
        .stroke(Stroke::new(
            1.0_f32,
            Color32::from_rgba_unmultiplied(
                border_accent.r(),
                border_accent.g(),
                border_accent.b(),
                52,
            ),
        ))
        .rounding(Rounding::same(11.0))
        .inner_margin(egui::Margin::same(10.0))
        .show(ui, add_contents)
}

pub fn draw_shadowed_osd_text(ui: &mut Ui, text: &str, color: Color32, font_size: f32) -> Rect {
    let font = FontId::proportional(font_size);
    let galley_shadow =
        ui.painter()
            .layout_no_wrap(text.to_string(), font.clone(), Color32::from_rgba_unmultiplied(0, 0, 0, 230));
    let size = galley_shadow.size() + Vec2::new(10.0, 6.0);
    let (rect, _) = ui.allocate_exact_size(size, Sense::click_and_drag());
    let pos = rect.min + Vec2::new(4.0, 2.0);
    let painter = ui.painter();

    // Crisp multi-offset black outline shadow so pure transparent OSD stays readable on any background
    for (dx, dy) in [(-1.0, 0.0), (1.0, 0.0), (0.0, -1.0), (0.0, 1.0), (1.2, 1.2)] {
        painter.text(
            pos + Vec2::new(dx, dy),
            egui::Align2::LEFT_TOP,
            text,
            font.clone(),
            Color32::from_rgba_unmultiplied(0, 0, 0, 220),
        );
    }
    painter.text(pos, egui::Align2::LEFT_TOP, text, font, color);
    rect
}
