//! Temporal interpolation / springs via **lilt** (Phase B0 foundation).
//!
//! Live LED/button animation is Phase C; this wrapper keeps the dependency
//! compile-proof and ready for the canvas render loop (`Instant` + springs).

use lilt::{Animated, Easing};
use std::time::Instant;

/// Opacity (or similar 0..=1 scalar) driven by lilt for future Camada B previews.
#[derive(Debug, Clone)]
pub struct ScalarAnim {
    inner: Animated<f32, Instant>,
}

impl ScalarAnim {
    pub fn new(value: f32) -> Self {
        Self {
            inner: Animated::new(value).duration(180.0).easing(Easing::EaseInOut),
        }
    }

    pub fn transition(&mut self, target: f32, at: Instant) {
        self.inner.transition(target, at);
    }

    pub fn animate(&self, at: Instant) -> f32 {
        self.inner.animate(|v| v, at)
    }

    pub fn value(&self) -> f32 {
        self.inner.value
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lilt_scalar_transitions() {
        let mut anim = ScalarAnim::new(0.0);
        let t0 = Instant::now();
        anim.transition(1.0, t0);
        assert_eq!(anim.value(), 1.0);
        let mid = anim.animate(t0);
        assert!((0.0..=1.0).contains(&mid));
    }
}
