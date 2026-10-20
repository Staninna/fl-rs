//! Morphine's effects: overdrive, chorus, delay, reverb and equaliser, in
//! the order `FUN_1800ff630` runs them on the voices' sum.

use crate::morphine_state::PARAMS;

/// The effects section.
#[derive(Debug, Clone, Default)]
pub struct Fx {
    rate: f64,
}

impl Fx {
    pub fn set_rate(&mut self, rate: f64) {
        self.rate = rate;
    }

    pub fn set_params(&mut self, _params: &[f32; PARAMS], _rate: f64) {}

    pub fn reset(&mut self) {}

    /// Seconds the effects ring on after the voices stop.
    pub fn tail(&self) -> f64 {
        0.0
    }

    pub fn process(&mut self, _left: &mut [f32], _right: &mut [f32]) {}
}
