//! Per-model token usage accumulator and end-of-run printer.
//!
//! The headless event stream emits one `SessionUsageEvent` per LLM call,
//! including calls from compaction and folded sub-agent samples. `RunUsage`
//! collapses those events into a per-model running total of input and output
//! tokens so a finished run can print a single "Token usage by model:" block
//! alongside the turn summary. When the catalog lists per-token prices for the
//! model that emitted an event, the same block also names a dollar amount and
//! the run ends with a single "Total cost:" line; with no pricing configured
//! the block stays in tokens only.

use llm::{ModelIdentity, SessionUsageEvent, TokenUsage, UsageCost, Usd};
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
    /// Cumulative estimated USD cost for every priced call that landed on this
    /// model. `None` means the catalog has no per-token price for any of this
    /// model's calls yet; `Some(zero)` would mean prices were configured but
    /// all calls were free, which still renders as `$0.000000`.
    cost: Option<UsageCost>,
}

impl ModelUsage {
    /// Fold one priced call into this entry. Adds to `tokens`; if a `cost` is
    /// supplied and the entry had none, the entry becomes priced.
    fn add_cost(&mut self, tokens: TokenUsage, cost: Option<UsageCost>) {
        self.tokens += tokens;
        match (self.cost.as_mut(), cost) {
            (Some(existing), Some(cost)) => {
                existing.input_usd += cost.input_usd;
                existing.output_usd += cost.output_usd;
                existing.cache_read_usd += cost.cache_read_usd;
                existing.cache_creation_usd += cost.cache_creation_usd;
                existing.total_usd += cost.total_usd;
            }
            (None, Some(cost)) => self.cost = Some(cost),
            // No pricing on this call keeps any previously accumulated cost,
            // which mirrors how `SessionUsageTotals::add` records unpriced
            // calls in `unpriced_calls` rather than zeroing the dollars.
            (existing, None) => {
                let _ = existing;
            }
        }
    }
}

impl RunUsage {
    pub(crate) fn record(&mut self, usage: &SessionUsageEvent) {
        let label = model_label(&usage.model);
        let call_cost = usage.model.pricing.map(|pricing| pricing.estimate_cost(usage.tokens));
        match self.models.iter_mut().find(|entry| entry.model == label) {
            Some(entry) => entry.add_cost(usage.tokens, call_cost),
            None => self.models.push(ModelUsage { model: label, tokens: usage.tokens, cost: call_cost }),
        }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.models.is_empty()
    }

    /// Sum of every model's reported input + output tokens across the whole
    /// run (TASK-25-45). This is the same total the end-of-run usage block
    /// renders, so the `--max-tokens` cap and the report can read the same
    /// figure without re-summing. Cache / audio / reasoning dimensions are
    /// not counted: the cap tracks the same input/output pair the existing
    /// `Token usage by model:` block already prints.
    #[allow(dead_code)] // consumed by a forthcoming `--max-tokens` flag (TASK-25-45); not yet wired in.
    pub(crate) fn total_tokens(&self) -> u64 {
        let mut total: u64 = 0;
        for entry in &self.models {
            total =
                total.saturating_add(entry.tokens.input_tokens.get()).saturating_add(entry.tokens.output_tokens.get());
        }
        total
    }

    /// `true` when at least one model has an accumulated price — the run as a
    /// whole is "priced" when any model is, and that is when dollars render.
    fn any_priced(&self) -> bool {
        self.models.iter().any(|entry| entry.cost.is_some())
    }

    pub(crate) fn format_text(&self) -> Option<String> {
        if self.models.is_empty() {
            return None;
        }
        let mut out = String::from("Token usage by model:");
        for entry in &self.models {
            match entry.cost {
                Some(cost) => {
                    let _ = write!(
                        out,
                        "\n  {}: {} in, {} out (${:.6})",
                        entry.model,
                        entry.tokens.input_tokens,
                        entry.tokens.output_tokens,
                        cost.total_usd.get()
                    );
                }
                None => {
                    let _ = write!(
                        out,
                        "\n  {}: {} in, {} out",
                        entry.model, entry.tokens.input_tokens, entry.tokens.output_tokens
                    );
                }
            }
        }
        if self.any_priced() {
            let total = self
                .models
                .iter()
                .filter_map(|entry| entry.cost)
                .fold(Usd::ZERO, |running, cost| running + cost.total_usd);
            let _ = write!(out, "\nTotal cost: ${:.6}", total.get());
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
    #[serde(skip_serializing_if = "Option::is_none")]
    estimated_cost_usd: Option<f64>,
}

pub(crate) fn print_run_usage(format: OutputFormat, usage: &RunUsage) {
    if let Some(rendered) = render(format, usage) {
        println!("{rendered}");
    }
}

/// Pure renderer for the end-of-run usage block. Returns the bytes the CLI
/// would print for `format`, or `None` when the provider reported no usage
/// (so the caller prints nothing extra). Text and JSON both share the same
/// emptiness check: an accumulator that recorded no `SessionUsageEvent`
/// yields no summary at all.
pub(crate) fn render(format: OutputFormat, usage: &RunUsage) -> Option<String> {
    if usage.is_empty() {
        return None;
    }
    match format {
        OutputFormat::Text | OutputFormat::Pretty => usage.format_text(),
        OutputFormat::Json => render_json(usage),
    }
}

fn render_json(usage: &RunUsage) -> Option<String> {
    if usage.is_empty() {
        return None;
    }
    let models: Vec<ModelUsageJson<'_>> = usage
        .models
        .iter()
        .map(|entry| ModelUsageJson {
            model: &entry.model,
            input_tokens: entry.tokens.input_tokens.get(),
            output_tokens: entry.tokens.output_tokens.get(),
            estimated_cost_usd: entry.cost.map(|cost| cost.total_usd.get()),
        })
        .collect();
    let total_cost: Option<f64> = if usage.any_priced() {
        Some(
            usage
                .models
                .iter()
                .filter_map(|entry| entry.cost)
                .fold(Usd::ZERO, |running, cost| running + cost.total_usd)
                .get(),
        )
    } else {
        None
    };
    let payload = if let Some(total) = total_cost {
        serde_json::json!({ "type": "run_usage", "models": models, "total_cost_usd": total })
    } else {
        serde_json::json!({ "type": "run_usage", "models": models })
    };
    Some(payload.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use llm::ModelPricing;

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

    #[test]
    fn run_usage_total_tokens_sums_input_and_output_per_model() {
        // Acceptance for `--max-tokens` (TASK-25-45): the cap reads the same
        // input+output total the end-of-run usage block renders, so the
        // cap and the report agree. Two models, each fed two samples, must
        // sum to the per-model running totals — the figure the format_text
        // path also prints.
        let mut usage = RunUsage::default();
        usage.record(&sample("m1", 10, 2));
        usage.record(&sample("m1", 5, 3));
        usage.record(&sample("m2", 7, 1));

        // 15+5 (m1) + 7+1 (m2) = 28
        assert_eq!(usage.total_tokens(), 28);

        // An empty accumulator must report zero so a no-usage run can't
        // accidentally trip the cap.
        assert_eq!(RunUsage::default().total_tokens(), 0);
    }

    #[test]
    fn run_usage_prices_tokens_at_the_model_rates() {
        // 1,000,000 in @ $3/M = $3.00; 1,000,000 out @ $15/M = $15.00; total $18.
        let pricing = ModelPricing {
            input_per_million: 3.0,
            output_per_million: 15.0,
            cache_read_per_million: None,
            cache_write_per_million: None,
        };
        let mut usage = RunUsage::default();
        usage.record(&priced_sample("m1", &pricing, 1_000_000, 0));
        usage.record(&priced_sample("m1", &pricing, 0, 1_000_000));

        assert_eq!(
            usage.format_text(),
            Some(
                "Token usage by model:\n  m1: 1000000 in, 1000000 out ($18.000000)\nTotal cost: $18.000000".to_string()
            )
        );
    }

    #[test]
    fn run_usage_prices_accumulate_across_turns() {
        // Two halves at the same rate must sum to the same total one full call gives.
        let pricing = ModelPricing {
            input_per_million: 3.0,
            output_per_million: 15.0,
            cache_read_per_million: None,
            cache_write_per_million: None,
        };
        let mut usage = RunUsage::default();
        usage.record(&priced_sample("m1", &pricing, 500_000, 0));
        usage.record(&priced_sample("m1", &pricing, 0, 500_000));

        assert_eq!(
            usage.format_text(),
            Some("Token usage by model:\n  m1: 500000 in, 500000 out ($9.000000)\nTotal cost: $9.000000".to_string())
        );
    }

    #[test]
    fn run_usage_mixed_pricing_prints_amounts_only_for_priced_models() {
        let pricing = ModelPricing {
            input_per_million: 2.0,
            output_per_million: 4.0,
            cache_read_per_million: None,
            cache_write_per_million: None,
        };
        let mut usage = RunUsage::default();
        usage.record(&sample("unpriced", 5, 7));
        usage.record(&priced_sample("priced", &pricing, 1_000_000, 0));

        assert_eq!(
            usage.format_text(),
            Some(
                "Token usage by model:\n  unpriced: 5 in, 7 out\n  priced: 1000000 in, 0 out ($2.000000)\nTotal cost: $2.000000"
                    .to_string()
            )
        );
    }

    fn sample(model: &str, input: u64, output: u64) -> SessionUsageEvent {
        let mut event = llm::testing::session_usage_event(1, TokenUsage::new(input, output));
        event.model = ModelIdentity { provider: Some("test".into()), model_id: Some(model.into()), pricing: None };
        event
    }

    fn priced_sample(model: &str, pricing: &ModelPricing, input: u64, output: u64) -> SessionUsageEvent {
        let mut event = llm::testing::session_usage_event(1, TokenUsage::new(input, output));
        event.model =
            ModelIdentity { provider: Some("test".into()), model_id: Some(model.into()), pricing: Some(*pricing) };
        event
    }

    /// Feeds a provider usage payload (the kind the headless stream carries in
    /// `AgentEvent::SessionUsage`) and asserts the rendered text names the
    /// exact input and output token counts. This is the acceptance test for
    /// "the CLI prints the input and output token counts the provider
    /// reported" — it bypasses the per-model grouping and the priced /
    /// unpriced branch by using a single, known model and a known payload.
    #[test]
    fn render_text_carries_provider_reported_token_counts() {
        let mut usage = RunUsage::default();
        // A realistic two-call sample for one model: 1,234 input / 56 output,
        // then 78 input / 90 output. The provider-reported sum is what the CLI
        // must print at end-of-run, not the per-call split.
        usage.record(&sample("gpt-test", 1_234, 56));
        usage.record(&sample("gpt-test", 78, 90));

        let rendered = render(OutputFormat::Text, &usage).expect("non-empty usage must render");
        assert!(
            rendered.contains("gpt-test: 1312 in, 146 out"),
            "rendered text must carry the summed input/output token counts, got: {rendered:?}"
        );
        assert!(
            rendered.starts_with("Token usage by model:"),
            "rendered text must open with the usage header, got: {rendered:?}"
        );
    }

    /// Same guarantee for the JSON output format the run can be configured to
    /// produce. `render` returns a JSON document that lists each model with
    /// the provider-reported `input_tokens` / `output_tokens` totals; an empty
    /// accumulator yields no document at all.
    #[test]
    fn render_json_carries_provider_reported_token_counts() {
        let mut usage = RunUsage::default();
        usage.record(&sample("gpt-test", 1_234, 56));
        usage.record(&sample("gpt-test", 78, 90));

        let rendered = render(OutputFormat::Json, &usage).expect("non-empty usage must render JSON");
        let value: serde_json::Value = serde_json::from_str(&rendered).expect("rendered output is valid JSON");
        assert_eq!(value["type"], "run_usage");
        let models = value["models"].as_array().expect("models is an array");
        assert_eq!(models.len(), 1);
        assert_eq!(models[0]["model"], "gpt-test");
        assert_eq!(models[0]["input_tokens"], 1_312);
        assert_eq!(models[0]["output_tokens"], 146);
        assert!(
            value.get("total_cost_usd").is_none(),
            "no pricing was supplied, so the JSON document must omit total_cost_usd: {rendered:?}"
        );

        // Empty usage must render nothing for either format.
        let empty = RunUsage::default();
        assert_eq!(render(OutputFormat::Text, &empty), None);
        assert_eq!(render(OutputFormat::Json, &empty), None);
    }
}
