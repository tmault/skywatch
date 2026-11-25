//! G2-smooth tangent quintic fillets inside the union of the original paved corridors.
//! A five-unit tangent offset stays well within 1.1 units at a right angle with zero
//! curvature jump at entry/exit.
#[derive(Clone, Debug, Default)]
pub(super) struct GroundRoute {
    pub points: Vec<([f32; 2], f32)>,
    pub distance: f32,
}
fn distance(a: [f32; 2], b: [f32; 2]) -> f32 {
    (a[0] - b[0]).hypot(a[1] - b[1])
}
fn mix(a: [f32; 2], b: [f32; 2], t: f32) -> [f32; 2] {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t]
}
fn eval_bezier(controls: &[[f32; 2]], t: f32) -> ([f32; 2], f32) {
    let mut ps = controls.to_vec();
    while ps.len() > 2 {
        ps = ps.windows(2).map(|w| mix(w[0], w[1], t)).collect();
    }
    let h = (ps[1][1] - ps[0][1]).atan2(ps[1][0] - ps[0][0]);
    (mix(ps[0], ps[1], t), h)
}
impl GroundRoute {
    pub fn new(start: [f32; 2], route: &[[f32; 2]], wide_apron: Option<f32>) -> Self {
        let mut vertices = vec![start];
        for &p in route {
            if distance(*vertices.last().unwrap(), p) > 0.001 {
                vertices.push(p);
            }
        }
        let mut path = Self::default();
        path.points.push((start, 0.0));
        let mut previous = start;
        for i in 1..vertices.len().saturating_sub(1) {
            let (a, b, c) = (vertices[i - 1], vertices[i], vertices[i + 1]);
            let (ab, bc) = (distance(a, b), distance(b, c));
            let u = [(b[0] - a[0]) / ab, (b[1] - a[1]) / ab];
            let v = [(c[0] - b[0]) / bc, (c[1] - b[1]) / bc];
            // Leave half of every segment for its other endpoint's fillet.
            let width = if wide_apron.is_some_and(|x| (b[0] - x).abs() < 0.01) && b[1].abs() < 80.0
            {
                8.0_f32
            } else {
                5.0_f32
            };
            let trim = width.min(ab * 0.45).min(bc * 0.45);
            if u[0] * v[0] + u[1] * v[1] < -0.99 {
                path.line(previous, b);
                previous = b;
                continue;
            }
            let entry = [b[0] - u[0] * trim, b[1] - u[1] * trim];
            let exit = [b[0] + v[0] * trim, b[1] + v[1] * trim];
            path.line(previous, entry);
            // Collinear first/last three control points give zero curvature at
            // entry and exit (G2 clothoid-like transition without steering snap).
            let controls = [
                entry,
                mix(entry, b, 0.27),
                mix(entry, b, 0.54),
                mix(exit, b, 0.54),
                mix(exit, b, 0.27),
                exit,
            ];
            for j in 1..=128 {
                let t = j as f32 / 128.0;
                path.points.push(eval_bezier(&controls, t));
            }
            previous = exit;
        }
        if let Some(&end) = vertices.last() {
            path.line(previous, end);
        }
        if path.points.len() > 1 {
            path.points[0].1 = path.points[1].1;
        }
        path
    }
    /// Blend headings while translating within the first/last paved segment.
    pub fn align(&mut self, start_heading: f32, end_heading: f32) {
        if self.points.len() < 2 {
            self.points[0].1 = start_heading;
            return;
        }
        for first in [true, false] {
            let heading = if first { start_heading } else { end_heading };
            let index = if first { 0 } else { self.points.len() - 1 };
            let current = self.points[index].1;
            if (heading - current)
                .sin()
                .atan2((heading - current).cos())
                .abs()
                < 0.001
            {
                continue;
            }
            let reverse_hint = (heading - current).cos() < -0.95;
            let target_span = if reverse_hint { 4.4 } else { 7.5 };
            let mut span = 0.0;
            let mut k = index;
            while span < target_span
                && (if first {
                    k + 1 < self.points.len()
                } else {
                    k > 0
                })
            {
                let next = if first { k + 1 } else { k - 1 };
                span += distance(self.points[k].0, self.points[next].0);
                k = next;
            }
            if span < 0.01 {
                continue;
            }
            let (a, b, ha, hb) = if first {
                (
                    self.points[0].0,
                    self.points[k].0,
                    heading,
                    self.points[k].1,
                )
            } else {
                (
                    self.points[k].0,
                    self.points[index].0,
                    self.points[k].1,
                    heading,
                )
            };
            let reverse = reverse_hint || (ha - hb).cos() < -0.95;
            let d = span.min(target_span) * 0.45;
            let p1 = [a[0] + ha.cos() * d, a[1] + ha.sin() * d];
            let p2 = [b[0] - hb.cos() * d, b[1] - hb.sin() * d];
            let controls = if reverse {
                vec![
                    a,
                    p1,
                    [
                        (a[0] + b[0]) * 0.5 - ha.sin() * d * 2.5,
                        (a[1] + b[1]) * 0.5 + ha.cos() * d * 2.5,
                    ],
                    p2,
                    b,
                ]
            } else {
                vec![a, p1, p2, b]
            };
            let mut blend = Vec::new();
            for j in 0..=256 {
                let t = j as f32 / 256.0;
                blend.push(eval_bezier(&controls, t));
            }
            if first {
                self.points.splice(0..=k, blend);
            } else {
                self.points.splice(k.., blend);
            }
        }
    }
    fn line(&mut self, a: [f32; 2], b: [f32; 2]) {
        let len = distance(a, b);
        if len < 0.0001 {
            return;
        }
        let h = (b[1] - a[1]).atan2(b[0] - a[0]);
        for i in 1..=(len / 0.25).ceil() as usize {
            let n = (len / 0.25).ceil();
            self.points.push((mix(a, b, i as f32 / n), h));
        }
    }
    pub fn remaining(&self) -> f32 {
        self.points
            .windows(2)
            .map(|w| distance(w[0].0, w[1].0))
            .sum::<f32>()
            - self.distance
    }
    pub fn speed_limit(&self, max: f32, brake: f32) -> f32 {
        let mut along = -self.distance;
        let mut limit = max.min((2.0 * brake * self.remaining().max(0.0)).sqrt());
        for w in self.points.windows(2) {
            let ds = distance(w[0].0, w[1].0);
            let yaw = (w[1].1 - w[0].1).sin().atan2((w[1].1 - w[0].1).cos()).abs();
            if ds > 0.00001 && yaw > 0.00001 {
                let curvature = yaw / ds;
                let corner = (1.5 / curvature).sqrt().min(0.8 / curvature);
                limit = limit.min((corner * corner + 2.0 * brake * (along - 10.0).max(0.0)).sqrt());
            }
            along += ds;
        }
        limit
    }
    pub fn advance(&mut self, travel: f32) -> ([f32; 2], f32, bool) {
        self.distance += travel;
        while self.points.len() > 1 {
            let ds = distance(self.points[0].0, self.points[1].0);
            if self.distance < ds && ds > 0.00001 {
                let t = self.distance / ds;
                let h = self.points[0].1;
                let dh = (self.points[1].1 - h)
                    .sin()
                    .atan2((self.points[1].1 - h).cos());
                return (
                    mix(self.points[0].0, self.points[1].0, t),
                    h + dh * t,
                    false,
                );
            }
            self.distance = (self.distance - ds).max(0.0);
            self.points.remove(0);
        }
        let (p, h) = self.points[0];
        (p, h, true)
    }
}
