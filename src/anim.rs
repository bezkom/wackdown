//! Frame-rate independent animation helpers. Every step takes the real elapsed time, so motion
//! looks identical at 60, 120 or 144 Hz.

/// Critically damped spring: reaches the target as fast as possible without overshooting.
#[derive(Clone, Copy, Debug)]
pub struct Spring {
    pub value: f32,
    pub velocity: f32,
    pub target: f32,
    /// Stiffness in rad/s; higher settles faster.
    pub omega: f32,
}

impl Spring {
    pub fn new(value: f32, omega: f32) -> Self {
        Self { value, velocity: 0.0, target: value, omega }
    }

    /// Jumps straight to `value` (drag, programmatic jumps).
    pub fn set(&mut self, value: f32) {
        self.value = value;
        self.target = value;
        self.velocity = 0.0;
    }

    pub fn settled(&self) -> bool {
        (self.value - self.target).abs() < 0.25 && self.velocity.abs() < 4.0
    }

    /// Advances by `dt` seconds using the exact solution of the critically damped spring.
    pub fn step(&mut self, dt: f32) {
        if self.settled() {
            self.value = self.target;
            self.velocity = 0.0;
            return;
        }
        let w = self.omega;
        let x = self.value - self.target;
        let decay = (-w * dt).exp();
        let k = self.velocity + w * x;
        self.value = self.target + (x + k * dt) * decay;
        self.velocity = (self.velocity - w * k * dt) * decay;
    }
}

/// Eases a 0..1 value toward on/off with an exponential approach.
#[derive(Clone, Copy, Debug, Default)]
pub struct Fade {
    pub value: f32,
    pub on: bool,
}

impl Fade {
    /// `rate`: how many e-foldings per second (12 ≈ 120 ms to settle).
    pub fn step(&mut self, dt: f32, rate: f32) {
        let target = if self.on { 1.0 } else { 0.0 };
        let t = 1.0 - (-rate * dt).exp();
        self.value += (target - self.value) * t;
        if (self.value - target).abs() < 0.01 {
            self.value = target;
        }
    }

    pub fn moving(&self) -> bool {
        self.value != if self.on { 1.0 } else { 0.0 }
    }
}

/// Smoothstep-style ease-out for one-shot effects (0..1 in, 0..1 out).
pub fn ease_out(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    1.0 - (1.0 - t).powi(3)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spring_settles_without_overshoot_at_any_frame_rate() {
        for hz in [30.0, 60.0, 144.0, 240.0] {
            let mut s = Spring::new(0.0, 20.0);
            s.target = 1000.0;
            let mut max = 0.0f32;
            for _ in 0..(hz as usize * 2) {
                s.step(1.0 / hz);
                max = max.max(s.value);
            }
            assert!(s.settled(), "{hz} Hz: did not settle ({})", s.value);
            assert!(max <= 1000.5, "{hz} Hz: overshot to {max}");
        }
    }

    #[test]
    fn fade_reaches_targets() {
        let mut f = Fade { value: 0.0, on: true };
        for _ in 0..60 {
            f.step(1.0 / 60.0, 12.0);
        }
        assert_eq!(f.value, 1.0);
        f.on = false;
        for _ in 0..60 {
            f.step(1.0 / 60.0, 12.0);
        }
        assert_eq!(f.value, 0.0);
        assert!(!f.moving());
    }
}
