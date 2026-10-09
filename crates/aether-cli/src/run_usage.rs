//! Per-model token usage accumulator and end-of-run printer.
//!
//! The headless event stream emits one `SessionUsageEvent` per LLM call,
//! including calls from compaction and folded sub-agent samples. `RunUsage`
//! collapses those events into a per-model running total of input and output
//! tokens so a finished run can print a single "Token usage by model:" block
//! alongside the turn summary.

use llm::{ModelIdentity, SessionUsageEvent, TokenUsage};
use serde::Serialize;
use std::fmt::Write as _;

use crate::output::OutputFormat;

/// Per-model running total of input and output tokens across every LLM call a
/// single headless run has seen. Samples are added in the order the agent
/// emits them, and the summary keeps that order.
#[derive(Default)]
pub(crate) struct RunUsage {
    models: Vec<ModelUsage>,
}

struct ModelUsage {
    model: String,
    tokens: TokenUsage,
}

impl RunUsage {
    pub(crate) fn record(&mut self, usage: &SessionUsageEvent) {
        let label = model_label(&usage.model);
        match self.models.iter_mut().find(|entry| entry.model == label) {
            Some(entry) => entry.tokens += usage.tokens,
            None => self.models.push(ModelUsage { model: label, tokens: usage.tokens }),
        }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.models.is_empty()
    }

    pub(crate) fn format_text(&self) -> Option<String> {
        if self.models.is_empty() {
            return None;
        }
        let mut out = String::from("Token usage by model:");
        for entry in &self.models {
            let _ = write!(
                out,
                "\n  {}: {} in, {} out",
                entry.model, entry.tokens.input_tokens, entry.tokens.output_tokens
            );
        }
        Some(out)
    }
}

fn model_label(identity: &ModelIdentity) -> String {
    identity.model_id.clone().or_else(|| identity.provider.clone()).unwrap_or_else(|| "unknown".to_string())
}

#[derive(Serialize)]
struct ModelUsageJson<'a> {
    model: &'a str,
    input_tokens: u64,
    output_tokens: u64,
}

pub(crate) fn print_run_usage(format: OutputFormat, usage: &RunUsage) {
    if usage.is_empty() {
        return;
    }
    match format {
        OutputFormat::Text | OutputFormat::Pretty => {
            if let Some(text) = usage.format_text() {
                println!("{text}");
            }
        }
        OutputFormat::Json => {
            let models: Vec<ModelUsageJson<'_>> = usage
                .models
                .iter()
                .map(|entry| ModelUsageJson {
                    model: &entry.model,
                    input_tokens: entry.tokens.input_tokens.get(),
                    output_tokens: entry.tokens.output_tokens.get(),
                })
                .collect();
            let payload = serde_json::json!({ "type": "run_usage", "models": models });
            println!("{payload}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_usage_sums_tokens_per_model_across_turns() {
        let mut usage = RunUsage::default();
        usage.record(&sample("m1", 10, 2));
        usage.record(&sample("m1", 5, 3));
        usage.record(&sample("m2", 7, 1));

        assert!(!usage.is_empty());
        assert_eq!(
            usage.format_text(),
            Some("Token usage by model:\n  m1: 15 in, 5 out\n  m2: 7 in, 1 out".to_string())
        );
    }

    #[test]
    fn run_usage_empty_has_no_summary() {
        let usage = RunUsage::default();
        assert!(usage.is_empty());
        assert_eq!(usage.format_text(), None);
    }

    fn sample(model: &str, input: u64, output: u64) -> SessionUsageEvent {
        let mut event = llm::testing::session_usage_event(1, TokenUsage::new(input, output));
        event.model = ModelIdentity { provider: Some("test".into()), model_id: Some(model.into()), pricing: None };
        event
    }
}
