//! Frame-rate independent motion primitives.

/// Damped harmonic spring. `stiffness`/`damping` per unit mass.
#[derive(Clone, Copy, Debug)]
pub struct Spring {
    pub value: f32,
    pub velocity: f32,
    pub target: f32,
    pub stiffness: f32,
    pub damping: f32,
}
impl Spring {
    pub const fn new(value: f32, stiffness: f32, damping: f32) -> Self {
        Self {
            value,
            velocity: 0.,
            target: value,
            stiffness,
            damping,
        }
    }
    pub fn set(&mut self, target: f32) {
        self.target = target;
    }
    pub fn snap(&mut self, value: f32) {
        self.value = value;
        self.target = value;
        self.velocity = 0.;
    }
    /// Advances the simulation; returns true while still moving.
    pub fn step(&mut self, dt: f32) -> bool {
        if self.settled() {
            self.value = self.target;
            self.velocity = 0.;
            return false;
        }
        // Sub-stepped semi-implicit Euler keeps stiff springs stable at low fps.
        let steps = (dt / 0.004).ceil().clamp(1., 64.) as usize;
        let h = dt / steps as f32;
        for _ in 0..steps {
            let force = (self.target - self.value) * self.stiffness - self.velocity * self.damping;
            self.velocity += force * h;
            self.value += self.velocity * h;
        }
        if self.settled() {
            self.value = self.target;
            self.velocity = 0.;
        }
        true
    }
    pub fn settled(&self) -> bool {
        let scale = self.target.abs().max(1.);
        (self.target - self.value).abs() < 0.0015 * scale && self.velocity.abs() < 0.01 * scale
    }
}

/// Exponential approach, frame-rate independent.
pub fn approach(value: &mut f32, target: f32, rate: f32, dt: f32) -> bool {
    if (*value - target).abs() < 0.002 {
        let changed = *value != target;
        *value = target;
        return changed;
    }
    *value += (target - *value) * (1. - (-rate * dt).exp());
    true
}
pub fn ease_out(t: f32) -> f32 {
    1. - (1. - t.clamp(0., 1.)).powi(3)
}
pub fn smoothstep(a: f32, b: f32, x: f32) -> f32 {
    let t = ((x - a) / (b - a)).clamp(0., 1.);
    t * t * (3. - 2. * t)
}
pub fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn spring_settles_exactly_on_target_at_any_frame_rate() {
        for dt in [1. / 240., 1. / 60., 1. / 20.] {
            let mut s = Spring::new(0., 300., 26.);
            s.set(1.);
            let mut frames = 0;
            while s.step(dt) {
                frames += 1;
                assert!(frames < 2000);
            }
            assert_eq!(s.value, 1.);
        }
    }
    #[test]
    fn approach_is_monotonic_and_lands() {
        let mut v = 0.;
        let mut last = 0.;
        for _ in 0..400 {
            approach(&mut v, 1., 12., 1. / 60.);
            assert!(v >= last);
            last = v;
        }
        assert_eq!(v, 1.);
    }
}
