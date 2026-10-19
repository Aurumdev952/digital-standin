//! Y2K look: an indigo-to-violet backdrop, chrome bevels, glossy aqua buttons, neon accents,
//! an LCD status display and LED bar meters.

use eframe::egui::{
    self, Color32, CornerRadius, FontId, Frame, LayerId, Margin, Mesh, Pos2, Rect, Response, RichText, Sense, Shape,
    Stroke, TextStyle, Ui, Vec2, pos2, vec2,
};

pub const CYAN: Color32 = Color32::from_rgb(0, 229, 255);
pub const MAGENTA: Color32 = Color32::from_rgb(255, 62, 200);
pub const LIME: Color32 = Color32::from_rgb(182, 255, 0);
pub const AMBER: Color32 = Color32::from_rgb(255, 196, 40);
pub const CORAL: Color32 = Color32::from_rgb(255, 110, 90);
pub const ICE: Color32 = Color32::from_rgb(160, 200, 255);
pub const LCD_GREEN: Color32 = Color32::from_rgb(120, 255, 140);
pub const CHROME: Color32 = Color32::from_rgb(214, 222, 238);
const DEEP: Color32 = Color32::from_rgb(14, 10, 46);
const VIOLET: Color32 = Color32::from_rgb(52, 18, 92);
const PANEL: Color32 = Color32::from_rgba_premultiplied(10, 8, 30, 205);

pub const PALETTE: [Color32; 8] =
    [CYAN, MAGENTA, LIME, AMBER, ICE, CORAL, Color32::from_rgb(170, 120, 255), Color32::from_rgb(90, 255, 210)];

pub fn install(ctx: &egui::Context) {
    ctx.all_styles_mut(|s| {
        let v = &mut s.visuals;
        v.dark_mode = true;
        v.override_text_color = Some(Color32::from_rgb(222, 228, 255));
        v.panel_fill = Color32::TRANSPARENT;
        v.window_fill = DEEP;
        v.extreme_bg_color = Color32::from_rgb(6, 4, 22);
        v.faint_bg_color = Color32::from_rgb(28, 20, 70);
        v.hyperlink_color = CYAN;
        v.window_stroke = Stroke::new(1.0, CYAN);
        v.window_corner_radius = CornerRadius::same(10);
        v.selection.bg_fill = MAGENTA.gamma_multiply(0.6);
        v.selection.stroke = Stroke::new(1.0, Color32::WHITE);
        let w = &mut v.widgets;
        w.noninteractive.bg_stroke = Stroke::new(1.0, Color32::from_rgb(70, 60, 140));
        w.noninteractive.fg_stroke = Stroke::new(1.0, Color32::from_rgb(222, 228, 255));
        for (state, fill, stroke) in [
            (&mut w.inactive, Color32::from_rgb(34, 40, 110), Color32::from_rgb(110, 140, 255)),
            (&mut w.hovered, Color32::from_rgb(40, 70, 160), CYAN),
            (&mut w.active, Color32::from_rgb(120, 30, 120), MAGENTA),
            (&mut w.open, Color32::from_rgb(40, 50, 130), CYAN),
        ] {
            state.bg_fill = fill;
            state.weak_bg_fill = fill;
            state.bg_stroke = Stroke::new(1.0, stroke);
            state.fg_stroke = Stroke::new(1.5, Color32::WHITE);
            state.corner_radius = CornerRadius::same(9);
        }
        s.text_styles.insert(TextStyle::Heading, FontId::monospace(15.0));
        s.text_styles.insert(TextStyle::Small, FontId::monospace(10.5));
        s.text_styles.insert(TextStyle::Monospace, FontId::monospace(12.0));
        s.spacing.item_spacing = vec2(7.0, 5.0);
        s.spacing.slider_width = 150.0;
    });
}

/// Paints the gradient sky behind every panel.
pub fn backdrop(ctx: &egui::Context) {
    let rect = ctx.content_rect();
    let painter = ctx.layer_painter(LayerId::background());
    let mut mesh = Mesh::default();
    let corners = [(rect.left_top(), DEEP), (rect.right_top(), VIOLET), (rect.right_bottom(), DEEP), (rect.left_bottom(), VIOLET)];
    for (p, c) in corners {
        mesh.colored_vertex(p, c);
    }
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(0, 2, 3);
    painter.add(Shape::mesh(mesh));
    let mut y = rect.top();
    while y < rect.bottom() {
        painter.line_segment([pos2(rect.left(), y), pos2(rect.right(), y)], Stroke::new(1.0, Color32::from_white_alpha(5)));
        y += 4.0;
    }
}

/// A translucent panel with a chrome bevel: light top-left edge, dark bottom-right edge.
pub fn bevel() -> Frame {
    Frame::new()
        .fill(PANEL)
        .stroke(Stroke::new(1.0, Color32::from_rgb(120, 110, 220)))
        .corner_radius(CornerRadius::same(10))
        .inner_margin(Margin::same(10))
        .outer_margin(Margin::same(4))
}

pub fn panel_frame() -> Frame {
    Frame::new().inner_margin(Margin::same(6))
}

/// Chrome heading text with a magenta drop shadow.
pub fn heading(ui: &mut Ui, text: &str) {
    let font = FontId::monospace(14.0);
    let galley = ui.painter().layout_no_wrap(text.to_uppercase(), font.clone(), CHROME);
    let (rect, _) = ui.allocate_exact_size(galley.size() + vec2(2.0, 2.0), Sense::hover());
    let p = ui.painter();
    p.text(rect.min + vec2(1.5, 1.5), egui::Align2::LEFT_TOP, text.to_uppercase(), font.clone(), MAGENTA.gamma_multiply(0.8));
    p.text(rect.min, egui::Align2::LEFT_TOP, text.to_uppercase(), font, CHROME);
}

/// The big title with a cyan-to-magenta gradient, drawn letter by letter.
pub fn title(ui: &mut Ui, text: &str) {
    let font = FontId::monospace(20.0);
    let chars: Vec<char> = text.chars().collect();
    let width = ui.painter().layout_no_wrap(text.to_string(), font.clone(), Color32::WHITE).size();
    let (rect, _) = ui.allocate_exact_size(width + vec2(2.0, 2.0), Sense::hover());
    let p = ui.painter();
    let mut x = rect.left();
    for (i, c) in chars.iter().enumerate() {
        let t = i as f32 / chars.len().max(1) as f32;
        let color = lerp(CYAN, MAGENTA, t);
        let g = p.layout_no_wrap(c.to_string(), font.clone(), color);
        p.galley(pos2(x + 2.0, rect.top() + 2.0), p.layout_no_wrap(c.to_string(), font.clone(), Color32::from_black_alpha(160)), Color32::BLACK);
        let w = g.size().x;
        p.galley(pos2(x, rect.top()), g, color);
        x += w;
    }
}

/// Green-on-black liquid-crystal readout.
pub fn lcd(ui: &mut Ui, text: &str) {
    Frame::new()
        .fill(Color32::from_rgb(4, 16, 8))
        .stroke(Stroke::new(1.0, Color32::from_rgb(40, 110, 60)))
        .corner_radius(CornerRadius::same(4))
        .inner_margin(Margin::symmetric(8, 3))
        .show(ui, |ui| ui.label(RichText::new(text).monospace().color(LCD_GREEN)));
}

/// A glossy pill button: saturated fill, a white sheen on the top half, and a bright rim.
pub fn aqua_button(ui: &mut Ui, text: &str, base: Color32, min_width: f32) -> Response {
    let font = FontId::monospace(12.5);
    let galley = ui.painter().layout_no_wrap(text.to_string(), font, Color32::WHITE);
    let size = vec2((galley.size().x + 24.0).max(min_width), 26.0);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    let p = ui.painter();
    let fill = if response.is_pointer_button_down_on() {
        base.gamma_multiply(0.7)
    } else if response.hovered() {
        lerp(base, Color32::WHITE, 0.2)
    } else {
        base
    };
    let r = CornerRadius::same(13);
    p.rect_filled(rect, r, fill);
    let sheen = Rect::from_min_max(rect.min + vec2(4.0, 2.0), pos2(rect.max.x - 4.0, rect.center().y));
    p.rect_filled(sheen, CornerRadius::same(10), Color32::from_white_alpha(70));
    p.rect_stroke(rect, r, Stroke::new(1.0, lerp(base, Color32::WHITE, 0.6)), egui::StrokeKind::Inside);
    let pos = rect.center() - galley.size() / 2.0;
    p.galley(pos + vec2(1.0, 1.0), ui.painter().layout_no_wrap(text.to_string(), FontId::monospace(12.5), Color32::from_black_alpha(140)), Color32::BLACK);
    p.galley(pos, galley, Color32::WHITE);
    response
}

/// A segmented LED bar, like a hi-fi level meter.
pub fn led_meter(ui: &mut Ui, label: &str, v: f32, color: Color32) {
    ui.horizontal(|ui| {
        ui.add_sized(vec2(96.0, 16.0), egui::Label::new(RichText::new(label).monospace().size(11.0)));
        let width = (ui.available_width() - 52.0).max(60.0);
        let (rect, _) = ui.allocate_exact_size(vec2(width, 14.0), Sense::hover());
        let p = ui.painter();
        p.rect_filled(rect, CornerRadius::same(2), Color32::from_rgb(6, 4, 22));
        let n = 28;
        let gap = 2.0;
        let seg = (rect.width() - gap * (n as f32 + 1.0)) / n as f32;
        let lit = (v.clamp(0.0, 1.0) * n as f32).round() as usize;
        for i in 0..n {
            let x = rect.left() + gap + i as f32 * (seg + gap);
            let r = Rect::from_min_size(pos2(x, rect.top() + 2.0), vec2(seg, rect.height() - 4.0));
            let c = if i < lit { if i >= n * 9 / 10 { CORAL } else { color } } else { color.gamma_multiply(0.12) };
            p.rect_filled(r, CornerRadius::same(1), c);
        }
        ui.label(RichText::new(format!("{v:.2}")).monospace().size(11.0).color(color));
    });
}

/// Draws a camera frame: chrome bevel, scanlines and a blinking LIVE tag.
pub fn crt(ui: &mut Ui, rect: Rect, time: f64) {
    let p = ui.painter();
    let mut y = rect.top();
    while y < rect.bottom() {
        p.line_segment([pos2(rect.left(), y), pos2(rect.right(), y)], Stroke::new(1.0, Color32::from_black_alpha(22)));
        y += 3.0;
    }
    p.rect_stroke(rect.expand(3.0), CornerRadius::same(8), Stroke::new(2.0, CHROME), egui::StrokeKind::Outside);
    p.rect_stroke(rect.expand(6.0), CornerRadius::same(10), Stroke::new(1.0, CYAN.gamma_multiply(0.7)), egui::StrokeKind::Outside);
    if (time * 1.6).fract() < 0.65 {
        let tag = Rect::from_min_size(rect.left_top() + vec2(10.0, 10.0), vec2(64.0, 20.0));
        p.rect_filled(tag, CornerRadius::same(4), Color32::from_black_alpha(150));
        p.circle_filled(tag.left_center() + vec2(11.0, 0.0), 5.0, Color32::from_rgb(255, 40, 60));
        p.text(tag.left_center() + vec2(21.0, 0.0), egui::Align2::LEFT_CENTER, "LIVE", FontId::monospace(12.0), Color32::WHITE);
    }
}

/// A neon line with a soft glow under it.
pub fn neon_line(painter: &egui::Painter, pts: Vec<Pos2>, color: Color32) {
    if pts.len() < 2 {
        return;
    }
    painter.add(Shape::line(pts.clone(), Stroke::new(5.0, color.gamma_multiply(0.18))));
    painter.add(Shape::line(pts, Stroke::new(1.6, color)));
}

/// Dark plot area with a faint grid every 15 s and every quarter of the range.
pub fn plot_area(painter: &egui::Painter, rect: Rect, secs: f64) {
    painter.rect_filled(rect, CornerRadius::same(4), Color32::from_rgba_premultiplied(4, 2, 18, 230));
    let grid = Stroke::new(1.0, Color32::from_rgba_premultiplied(60, 50, 140, 60));
    for i in 1..4 {
        let y = rect.top() + rect.height() * i as f32 / 4.0;
        painter.line_segment([pos2(rect.left(), y), pos2(rect.right(), y)], grid);
    }
    let mut t = 15.0;
    while t < secs {
        let x = rect.right() - (t / secs) as f32 * rect.width();
        painter.line_segment([pos2(x, rect.top()), pos2(x, rect.bottom())], grid);
        t += 15.0;
    }
    painter.rect_stroke(rect, CornerRadius::same(4), Stroke::new(1.0, Color32::from_rgb(70, 60, 150)), egui::StrokeKind::Inside);
}

fn lerp(a: Color32, b: Color32, t: f32) -> Color32 {
    let m = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgb(m(a.r(), b.r()), m(a.g(), b.g()), m(a.b(), b.b()))
}

pub fn spacer(ui: &mut Ui, h: f32) {
    ui.allocate_exact_size(Vec2::new(1.0, h), Sense::hover());
}
