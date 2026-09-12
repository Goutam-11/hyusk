use eframe::egui;

pub struct Butterfly;

impl Butterfly {
    pub fn draw(painter: &egui::Painter, center: egui::Pos2, time: f32, intensity: f32) {
        let breathing = 1.0 + (time * 1.8).sin() * 0.025 * intensity;

        let wing_motion = (time * 3.5).sin() * 0.08 * intensity;

        let scale = 1.0 + wing_motion;

        let size = 38.0 * breathing;

        // ----------------------------------------
        // Ambient glow
        // ----------------------------------------

        Self::draw_glow(painter, center, size, intensity, time);

        // ----------------------------------------
        // Outer energy rings
        // ----------------------------------------

        Self::draw_rings(painter, center, size, intensity, time);

        // ----------------------------------------
        // Butterfly
        // ----------------------------------------

        Self::draw_wing(painter, center, size, scale, false, false, time);

        Self::draw_wing(painter, center, size, scale, true, false, time);

        Self::draw_wing(painter, center, size, scale * 0.9, false, true, time);

        Self::draw_wing(painter, center, size, scale * 0.9, true, true, time);

        // ----------------------------------------
        // Butterfly body
        // ----------------------------------------

        Self::draw_body(painter, center, size);

        // ----------------------------------------
        // Antennae
        // ----------------------------------------

        Self::draw_antennae(painter, center, size, time);

        // ----------------------------------------
        // Floating particles
        // ----------------------------------------

        Self::draw_particles(painter, center, size, intensity, time);
    }

    // ============================================================
    // Glow
    // ============================================================

    fn draw_glow(
        painter: &egui::Painter,
        center: egui::Pos2,
        size: f32,
        intensity: f32,
        time: f32,
    ) {
        let pulse = 1.0 + (time * 2.0).sin() * 0.04;

        for layer in (1..=12).rev() {
            let radius = size * pulse + layer as f32 * 6.0;

            let alpha = ((14.0 / layer as f32) * intensity).min(18.0) as u8;

            let color = if layer % 2 == 0 {
                egui::Color32::from_rgba_unmultiplied(70, 170, 255, alpha)
            } else {
                egui::Color32::from_rgba_unmultiplied(180, 80, 255, alpha)
            };

            painter.circle_filled(center, radius, color);
        }
    }

    // ============================================================
    // Rings
    // ============================================================

    fn draw_rings(
        painter: &egui::Painter,
        center: egui::Pos2,
        size: f32,
        intensity: f32,
        time: f32,
    ) {
        let rotation = time * 0.35;

        for ring in 0..3 {
            let radius = size + 15.0 + ring as f32 * 10.0;

            let alpha = (35.0 * intensity / (ring as f32 + 1.0)) as u8;

            painter.circle_stroke(
                center,
                radius,
                egui::Stroke::new(
                    0.7_f32,
                    egui::Color32::from_rgba_unmultiplied(100, 180, 255, alpha),
                ),
            );
        }

        // Rotating energy arc.
        let arc_radius = size + 24.0;

        let segments = 24;

        let start = rotation;

        let end = rotation + 1.5;

        let mut points = Vec::with_capacity(segments + 1);

        for i in 0..=segments {
            let t = i as f32 / segments as f32;

            let angle = start + (end - start) * t;

            points.push(egui::pos2(
                center.x + angle.cos() * arc_radius,
                center.y + angle.sin() * arc_radius,
            ));
        }

        painter.add(egui::Shape::line(
            points,
            egui::Stroke::new(
                1.5_f32,
                egui::Color32::from_rgba_unmultiplied(130, 210, 255, 170),
            ),
        ));
    }

    // ============================================================
    // Wing
    // ============================================================

    fn draw_wing(
        painter: &egui::Painter,
        center: egui::Pos2,
        size: f32,
        scale: f32,
        right: bool,
        lower: bool,
        time: f32,
    ) {
        let direction = if right { 1.0 } else { -1.0 };

        let wing_scale = if lower { 0.72 } else { 1.0 };

        let flap = 1.0 + (time * 4.0).sin() * 0.025;

        let sx = size * scale * wing_scale * direction;

        let sy = size * scale * wing_scale * flap;

        let points = if lower {
            Self::lower_wing_points(center, sx, sy)
        } else {
            Self::upper_wing_points(center, sx, sy)
        };

        Self::gradient_shape(painter, &points, time, lower, right);

        Self::wing_outline(painter, &points, right, lower);
    }

    // ============================================================
    // Upper wing geometry
    // ============================================================

    fn upper_wing_points(center: egui::Pos2, sx: f32, sy: f32) -> Vec<egui::Pos2> {
        vec![
            egui::pos2(center.x, center.y - sy * 0.05),
            egui::pos2(center.x + sx * 0.30, center.y - sy * 0.72),
            egui::pos2(center.x + sx * 0.82, center.y - sy * 0.82),
            egui::pos2(center.x + sx * 1.05, center.y - sy * 0.40),
            egui::pos2(center.x + sx * 0.88, center.y + sy * 0.05),
            egui::pos2(center.x + sx * 0.50, center.y + sy * 0.28),
            egui::pos2(center.x + sx * 0.22, center.y + sy * 0.12),
        ]
    }

    // ============================================================
    // Lower wing geometry
    // ============================================================

    fn lower_wing_points(center: egui::Pos2, sx: f32, sy: f32) -> Vec<egui::Pos2> {
        vec![
            egui::pos2(center.x, center.y + sy * 0.04),
            egui::pos2(center.x + sx * 0.48, center.y + sy * 0.18),
            egui::pos2(center.x + sx * 0.65, center.y + sy * 0.58),
            egui::pos2(center.x + sx * 0.28, center.y + sy * 0.82),
            egui::pos2(center.x + sx * 0.04, center.y + sy * 0.40),
        ]
    }

    // ============================================================
    // Gradient wing mesh
    // ============================================================

    fn gradient_shape(
        painter: &egui::Painter,
        points: &[egui::Pos2],
        time: f32,
        lower: bool,
        right: bool,
    ) {
        if points.len() < 3 {
            return;
        }

        let center = points[0];

        let mut mesh = egui::Mesh::default();

        let center_color = if lower {
            egui::Color32::from_rgba_unmultiplied(80, 220, 255, 150)
        } else {
            egui::Color32::from_rgba_unmultiplied(120, 210, 255, 175)
        };

        let center_index = mesh.vertices.len() as u32;

        mesh.colored_vertex(center, center_color);

        for i in 1..points.len() {
            let p = points[i];

            let phase = (time * 0.8 + i as f32 * 0.8).sin() * 0.5 + 0.5;

            let color = if phase < 0.33 {
                egui::Color32::from_rgba_unmultiplied(40, 210, 255, 135)
            } else if phase < 0.66 {
                egui::Color32::from_rgba_unmultiplied(150, 100, 255, 145)
            } else {
                egui::Color32::from_rgba_unmultiplied(255, 105, 205, 150)
            };

            let index = mesh.vertices.len() as u32;

            mesh.colored_vertex(p, color);

            let next = if i + 1 < points.len() { i + 1 } else { 1 };

            mesh.add_triangle(center_index, index, next as u32);
        }

        // Slightly different tint for opposite wings.
        if right {
            let _ = time;
        }

        painter.add(egui::Shape::mesh(mesh));
    }

    // ============================================================
    // Wing outline
    // ============================================================

    fn wing_outline(painter: &egui::Painter, points: &[egui::Pos2], right: bool, lower: bool) {
        let color = if lower {
            egui::Color32::from_rgba_unmultiplied(90, 220, 255, 110)
        } else if right {
            egui::Color32::from_rgba_unmultiplied(255, 135, 225, 150)
        } else {
            egui::Color32::from_rgba_unmultiplied(90, 210, 255, 150)
        };

        let mut outline = points.to_vec();

        outline.push(points[0]);

        painter.add(egui::Shape::line(
            outline,
            egui::Stroke::new(1.0_f32, color),
        ));
    }

    // ============================================================
    // Body
    // ============================================================

    fn draw_body(painter: &egui::Painter, center: egui::Pos2, size: f32) {
        // Outer glow.
        painter.circle_filled(
            egui::pos2(center.x, center.y),
            size * 0.15,
            egui::Color32::from_rgba_unmultiplied(200, 230, 255, 80),
        );

        // Head.
        painter.circle_filled(
            egui::pos2(center.x, center.y - size * 0.16),
            size * 0.09,
            egui::Color32::from_rgb(12, 18, 30),
        );

        let body_center = egui::pos2(center.x, center.y + size * 0.08);

        let body_points = vec![
            egui::pos2(body_center.x, body_center.y - size * 0.30),
            egui::pos2(body_center.x + size * 0.055, body_center.y - size * 0.20),
            egui::pos2(body_center.x + size * 0.065, body_center.y + size * 0.18),
            egui::pos2(body_center.x, body_center.y + size * 0.30),
            egui::pos2(body_center.x - size * 0.065, body_center.y + size * 0.18),
            egui::pos2(body_center.x - size * 0.055, body_center.y - size * 0.20),
        ];

        painter.add(egui::Shape::convex_polygon(
            body_points,
            egui::Color32::from_rgb(8, 13, 24),
            egui::Stroke::NONE,
        ));

        // Central highlight.
        painter.circle_filled(
            egui::pos2(center.x, center.y + size * 0.01),
            size * 0.035,
            egui::Color32::from_rgb(225, 245, 255),
        );
    }

    // ============================================================
    // Antennae
    // ============================================================

    fn draw_antennae(painter: &egui::Painter, center: egui::Pos2, size: f32, time: f32) {
        let sway = (time * 2.0).sin() * size * 0.025;

        let left_start = egui::pos2(center.x - size * 0.025, center.y - size * 0.22);

        let right_start = egui::pos2(center.x + size * 0.025, center.y - size * 0.22);

        let left_end = egui::pos2(center.x - size * 0.22 + sway, center.y - size * 0.52);

        let right_end = egui::pos2(center.x + size * 0.22 + sway, center.y - size * 0.52);

        painter.line_segment(
            [left_start, left_end],
            egui::Stroke::new(
                0.8_f32,
                egui::Color32::from_rgba_unmultiplied(190, 225, 255, 180),
            ),
        );

        painter.line_segment(
            [right_start, right_end],
            egui::Stroke::new(
                0.8_f32,
                egui::Color32::from_rgba_unmultiplied(255, 175, 235, 180),
            ),
        );

        painter.circle_filled(left_end, 1.4, egui::Color32::from_rgb(180, 230, 255));

        painter.circle_filled(right_end, 1.4, egui::Color32::from_rgb(255, 170, 230));
    }

    // ============================================================
    // Particles
    // ============================================================

    fn draw_particles(
        painter: &egui::Painter,
        center: egui::Pos2,
        size: f32,
        intensity: f32,
        time: f32,
    ) {
        let count = (14.0 * intensity) as usize;

        for i in 0..count {
            let seed = i as f32 * 12.9898;

            let angle = time * (0.15 + i as f32 * 0.008) + seed;

            let radius = size * (1.35 + (seed.sin() * 0.25));

            let x = center.x + angle.cos() * radius;

            let y = center.y + angle.sin() * radius * 0.75;

            let flicker = 0.5 + 0.5 * (time * 3.0 + seed).sin();

            let alpha = (80.0 + 150.0 * flicker) as u8;

            painter.circle_filled(
                egui::pos2(x, y),
                0.8 + flicker * 1.2,
                egui::Color32::from_rgba_unmultiplied(130, 210, 255, alpha),
            );
        }
    }
}
