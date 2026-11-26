use egui::{Color32, FontId, Pos2, Rect, Rounding, Sense, Stroke, Ui, Vec2};
use std::f32::consts::PI;

pub fn draw_circular_gauge(
    ui: &mut Ui,
    size: f32,
    value_pct: f32,
    _center_label: &str,
    sub_text: &str,
    normal_color: Color32,
    warn_color: Color32,
    warn_threshold: f32,
) {
    let (rect, _resp) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
    let painter = ui.painter_at(rect);
    let center = rect.center();
    let stroke_w = (size * 0.074).clamp(4.5, 6.2);
    let radius = (size * 0.5) - stroke_w * 0.75 - 1.5;

    // Deep subtle charcoal track ring matching original PulseGlass WPF aesthetic
    painter.circle_stroke(
        center,
        radius,
        Stroke::new(stroke_w, Color32::from_rgb(56, 48, 60)),
    );

    // Active arc with rounded caps
    let pct = value_pct.clamp(0.0, 100.0);
    let arc_color = if pct >= warn_threshold {
        warn_color
    } else {
        normal_color
    };

    if pct > 0.5 {
        let steps = ((pct / 100.0) * 64.0).ceil().max(6.0) as usize;
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
        if let (Some(&first), Some(&last)) = (pts.first(), pts.last()) {
            painter.circle_filled(first, stroke_w * 0.5, arc_color);
            painter.circle_filled(last, stroke_w * 0.5, arc_color);
        }
        painter.add(egui::Shape::line(pts, Stroke::new(stroke_w, arc_color)));
    }

    // Center percentage
    painter.text(
        Pos2::new(center.x, center.y - size * 0.11),
        egui::Align2::CENTER_CENTER,
        format!("{:.0}%", pct),
        FontId::proportional((size * 0.225).clamp(13.0, 19.0)),
        Color32::from_rgb(255, 245, 242),
    );

    // Sub-text (temp / GB)
    painter.text(
        Pos2::new(center.x, center.y + size * 0.15),
        egui::Align2::CENTER_CENTER,
        sub_text,
        FontId::proportional((size * 0.15).clamp(10.0, 12.5)),
        normal_color,
    );
}

pub fn glass_card<R>(
    ui: &mut Ui,
    border_accent: Color32,
    add_contents: impl FnOnce(&mut Ui) -> R,
) -> egui::InnerResponse<R> {
    let base = ui.visuals().extreme_bg_color;
    let fill = if base.r() > 0 || base.g() > 0 || base.b() > 0 {
        Color32::from_rgba_unmultiplied(base.r(), base.g(), base.b(), 215)
    } else {
        Color32::from_rgba_unmultiplied(34, 27, 38, 215)
    };
    egui::Frame::none()
        .fill(fill)
        .stroke(Stroke::new(
            1.0_f32,
            Color32::from_rgba_unmultiplied(
                border_accent.r(),
                border_accent.g(),
                border_accent.b(),
                68,
            ),
        ))
        .rounding(Rounding::same(11.0))
        .inner_margin(egui::Margin::symmetric(11.0, 9.0))
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            add_contents(ui)
        })
}

pub fn measure_osd_text_size(ui: &egui::Ui, template_text: &str, font_size: f32) -> Vec2 {
    let font = FontId::proportional(font_size);
    let galley = ui
        .painter()
        .layout_no_wrap(template_text.to_string(), font, Color32::WHITE);
    Vec2::new(
        (galley.size().x + 26.0).ceil().max(150.0),
        (galley.size().y + 14.0).ceil().max(26.0),
    )
}

pub fn draw_shadowed_osd_text(
    ui: &mut Ui,
    text: &str,
    color: Color32,
    font_size: f32,
    fixed_box: Option<Vec2>,
) -> (Rect, egui::Response) {
    let font = FontId::proportional(font_size);
    let galley = ui
        .painter()
        .layout_no_wrap(text.to_string(), font.clone(), color);
    let natural = galley.size() + Vec2::new(12.0, 8.0);
    let size = match fixed_box {
        Some(fb) => Vec2::new(fb.x.max(natural.x), fb.y.max(natural.y)),
        None => natural,
    };
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click_and_drag());
    let pos = rect.min + Vec2::new(6.0, ((rect.height() - galley.size().y) * 0.5).max(2.0));
    let painter = ui.painter();

    // Crisp multi-offset black outline shadow so transparent OSD is legible on any screen
    for (dx, dy) in [(-1.0, 0.0), (1.0, 0.0), (0.0, -1.0), (0.0, 1.0), (1.2, 1.2)] {
        painter.text(
            pos + Vec2::new(dx, dy),
            egui::Align2::LEFT_TOP,
            text,
            font.clone(),
            Color32::from_rgba_unmultiplied(0, 0, 0, 235),
        );
    }
    painter.text(pos, egui::Align2::LEFT_TOP, text, font, color);
    (rect, resp)
}
