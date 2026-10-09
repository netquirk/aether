import { describe, expect, it } from "vitest";

import {
  Transcript,
  TranscriptError,
  turnEnded,
  type AgentEvent,
  type LlmCallOutcome,
  type SessionUsageEvent,
  type SessionUsageTotals,
} from "../src/index.js";
import { ZERO_TOKEN_USAGE } from "../src/transcript.js";

describe("cumulative session usage", () => {
  it("preserves the complete snapshot in a failed transcript", async () => {
    const totals: SessionUsageTotals = {
      tokens: { input_tokens: 100, output_tokens: 20 },
      estimated_usd: 1,
      estimated_input_usd: 0.25,
      estimated_output_usd: 0.5,
      estimated_cache_read_usd: 0.125,
      estimated_cache_creation_usd: 0.125,
      unpriced_calls: 1,
    };

    const usage: SessionUsageEvent = {
      sequence: 3,
      source: {
        agent_id: "child",
        parent_agent_id: "root",
        agent_name: "Explore",
      },
      purpose: "chat",
      model: {},
      tokens: { input_tokens: 10, output_tokens: 2 },
      estimated_cost: null,
      totals,
    };

    const failure = new Error("provider failed");
    async function* stream(): AsyncGenerator<AgentEvent> {
      yield { category: "session_usage", event: usage };
      throw failure;
    }

    const error = await Transcript.fromStream(stream()).catch(
      (cause: unknown) => cause,
    );
    expect(error).toBeInstanceOf(TranscriptError);
    if (!(error instanceof TranscriptError)) throw error;
    expect(error.cause).toBe(failure);
    expect(error.transcript.events).toEqual([
      { category: "session_usage", event: usage },
    ]);
  });
});

describe("per-turn token usage", () => {
  it("returns one entry per terminal turn with summed token usage", () => {
    const trace = new Transcript([
      turnStarted(),
      llmCallEnded({
        status: "completed",
        usage: { input_tokens: 10, output_tokens: 2 },
      }),
      turnEnded(),
      turnStarted(),
      llmCallEnded({
        status: "completed",
        usage: { input_tokens: 30, output_tokens: 5 },
      }),
      turnEnded(),
    ]);

    expect(trace.turnUsage()).toEqual([
      {
        outcome: { status: "completed" },
        usage: { ...ZERO_TOKEN_USAGE, input_tokens: 10, output_tokens: 2 },
      },
      {
        outcome: { status: "completed" },
        usage: { ...ZERO_TOKEN_USAGE, input_tokens: 30, output_tokens: 5 },
      },
    ]);
  });

  it("sums completed calls within a single turn", () => {
    const trace = new Transcript([
      turnStarted(),
      llmCallEnded({
        status: "completed",
        usage: { input_tokens: 10, output_tokens: 2 },
      }),
      llmCallEnded({
        status: "completed",
        usage: { input_tokens: 20, output_tokens: 3 },
      }),
      turnEnded(),
    ]);

    expect(trace.turnUsage()).toEqual([
      {
        outcome: { status: "completed" },
        usage: { ...ZERO_TOKEN_USAGE, input_tokens: 30, output_tokens: 5 },
      },
    ]);
  });

  it("records zeroed usage for a cancelled turn without leakage from a prior turn", () => {
    const trace = new Transcript([
      turnStarted(),
      llmCallEnded({
        status: "completed",
        usage: { input_tokens: 10, output_tokens: 2 },
      }),
      turnEnded(),
      turnStarted(),
      llmCallEnded({ status: "cancelled" }) as AgentEvent,
      turnEnded({ status: "cancelled" }),
    ]);

    expect(trace.turnUsage()).toEqual([
      {
        outcome: { status: "completed" },
        usage: { ...ZERO_TOKEN_USAGE, input_tokens: 10, output_tokens: 2 },
      },
      { outcome: { status: "cancelled" }, usage: { ...ZERO_TOKEN_USAGE } },
    ]);
  });

  it("ignores session_usage events when attributing turn tokens", () => {
    const trace = new Transcript([
      {
        category: "session_usage",
        event: {
          sequence: 1,
          source: {
            agent_id: "root",
            parent_agent_id: null,
            agent_name: "root",
          },
          purpose: "chat",
          model: {},
          tokens: { input_tokens: 999, output_tokens: 999 },
          estimated_cost: null,
          totals: {
            tokens: { input_tokens: 999, output_tokens: 999 },
            estimated_usd: 0,
            estimated_input_usd: 0,
            estimated_output_usd: 0,
            estimated_cache_read_usd: 0,
            estimated_cache_creation_usd: 0,
            unpriced_calls: 0,
          },
        },
      },
      turnStarted(),
      llmCallEnded({
        status: "completed",
        usage: { input_tokens: 10, output_tokens: 2 },
      }),
      turnEnded(),
    ]);

    expect(trace.turnUsage()).toEqual([
      {
        outcome: { status: "completed" },
        usage: { ...ZERO_TOKEN_USAGE, input_tokens: 10, output_tokens: 2 },
      },
    ]);
  });

  it("returns an empty list when no turn terminated", () => {
    const trace = new Transcript([]);
    expect(trace.turnUsage()).toEqual([]);
  });

  it("preserves reported cache token dimensions across calls", () => {
    const trace = new Transcript([
      turnStarted(),
      llmCallEnded({
        status: "completed",
        usage: {
          input_tokens: 10,
          output_tokens: 2,
          cache_read_tokens: 3,
          cache_creation_tokens: null,
          reasoning_tokens: 5,
        },
      }),
      llmCallEnded({
        status: "completed",
        usage: {
          input_tokens: 0,
          output_tokens: 0,
          cache_read_tokens: 7,
          reasoning_tokens: 4,
        },
      }),
      turnEnded(),
    ]);

    expect(trace.turnUsage()[0]?.usage).toEqual({
      ...ZERO_TOKEN_USAGE,
      input_tokens: 10,
      output_tokens: 2,
      cache_read_tokens: 10,
      cache_creation_tokens: null,
      reasoning_tokens: 9,
    });
  });
});

function llmCallEnded(outcome: LlmCallOutcome): AgentEvent {
  return {
    category: "turn",
    event: { type: "llm_call_ended", purpose: "chat", outcome },
  };
}

function turnStarted(): AgentEvent {
  return { category: "turn", event: { type: "started", content: [] } };
}
