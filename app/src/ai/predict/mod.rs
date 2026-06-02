//! This module contains all code relevant to Agent Predict within Waz.
//!
//! Agent Predict attempts to predict the next action the user will take in Waz.

pub(crate) mod generate_ai_input_suggestions;
pub(crate) mod generate_am_query_suggestions;
pub mod next_command_model;
// Waz(Wave 3-2):`predict_am_queries` API module has been physically deleted - the original `ServerApi::predict_am_queries`
// 0 External consumption has been deleted simultaneously; FeatureFlag::PredictAMQueries/terminal/input.rs
// `predict_am_queries_future_handle` is reserved only as a control switch/handle code, this module is no longer required.
pub mod prompt_suggestions;
