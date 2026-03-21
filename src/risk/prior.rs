use crate::types::Prob;

#[derive(Debug, Clone, Copy)]
pub enum PriorMode {
    ModelOnly,
    Fixed(Prob),
}

#[derive(Debug, Clone, Copy)]
pub struct PriorEngine {
    mode: PriorMode,
}

impl PriorEngine {
    pub fn new(configured_prior: Option<f64>) -> Self {
        let mode = configured_prior
            .map(|prior| PriorMode::Fixed(Prob(prior.clamp(0.0, 1.0))))
            .unwrap_or(PriorMode::ModelOnly);
        Self { mode }
    }

    pub fn mode(&self) -> PriorMode {
        self.mode
    }

    pub fn current_prior(&self) -> Option<Prob> {
        match self.mode {
            PriorMode::ModelOnly => None,
            PriorMode::Fixed(prob) => Some(prob),
        }
    }
}
