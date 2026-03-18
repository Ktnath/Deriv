use ort::{session::builder::GraphOptimizationLevel, session::Session};
use std::sync::{Arc, Mutex};
use crate::types::Prob;

#[derive(Clone)]
pub struct OnnxRunner {
    session: Arc<Mutex<Session>>,
}

impl OnnxRunner {
    pub fn new(model_path: &str) -> Result<Self, String> {
        let session = Session::builder()
            .map_err(|e| e.to_string())?
            .with_optimization_level(GraphOptimizationLevel::Level3)
            .map_err(|e| e.to_string())?
            .with_intra_threads(1)
            .map_err(|e| e.to_string())?
            .commit_from_file(model_path)
            .map_err(|e| e.to_string())?;

        Ok(Self {
            session: Arc::new(Mutex::new(session)),
        })
    }

    pub fn predict(&self, features: &[f32; 10]) -> Result<Prob, String> {
        let array = ndarray::Array2::from_shape_vec((1, 10), features.to_vec())
            .map_err(|e| e.to_string())?
            .into_dyn();

        let input_value = ort::value::Value::from_array(array)
            .map_err(|e| e.to_string())?;

        let inputs = vec![("float_input", input_value)];
        
        // Lock the session for mutable run
        let mut session = self.session.lock().map_err(|e| e.to_string())?;
        let outputs = session.run(inputs)
            .map_err(|e| e.to_string())?;
        
        let mut prob_val = 0.50;

        if let Some(output) = outputs.get("output_probability") {
             if let Ok(tensor) = output.try_extract_tensor::<f32>() {
                 // Output probability tensor shape for binary classification without ZipMap is (1, 2)
                 // index 0: Down (class 0), index 1: Up (class 1)
                 if let Some(&p_up) = tensor.1.get(1) {
                     prob_val = p_up as f64;
                 }
             }
        } else if let Some(output) = outputs.get("output_label") {
             // Fallback to label if probability extraction fails
             if let Ok(tensor) = output.try_extract_tensor::<i64>() {
                 if let Some(&label) = tensor.1.first() {
                     prob_val = if label == 1 { 0.60 } else { 0.40 };
                 }
             }
        }
        
        Ok(Prob(prob_val))
    }
}
