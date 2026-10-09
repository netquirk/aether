import type {
  AgentEvent,
  ContextUsage,
  LlmCallOutcome,
  TokenUsage,
  TurnOutcome,
} from "@aether-agent/sdk";

export interface TurnUsage {
  outcome: TurnOutcome;
  usage: TokenUsage;
}

export class ToolCall {
  readonly arguments: string;

  constructor(
    readonly name: string,
    argumentsValue: string,
  ) {
    this.arguments = argumentsValue;
  }

  argumentsJson(): unknown {
    return JSON.parse(this.arguments);
  }
}

export class TranscriptError extends Error {
  readonly transcript: Transcript;

  constructor(transcript: Transcript, cause: unknown) {
    super("transcript stream failed", { cause });
    this.name = "TranscriptError";
    this.transcript = transcript;
  }
}

export class Transcript {
  readonly events: AgentEvent[];

  constructor(events: AgentEvent[] = []) {
    this.events = [...events];
  }

  static async fromStream(
    stream: AsyncIterable<AgentEvent>,
  ): Promise<Transcript> {
    const transcript = new Transcript();
    try {
      for await (const event of stream) {
        transcript.add(event);
      }
    } catch (err) {
      throw new TranscriptError(transcript, err);
    }
    return transcript;
  }

  add(event: AgentEvent): void {
    this.events.push(event);
  }

  allToolCalls(): ToolCall[] {
    return extractToolCalls(this.events);
  }

  toolCalls(name: string): ToolCall[] {
    return this.allToolCalls().filter((call) => call.name === name);
  }

  toolCalled(name: string): boolean {
    return this.toolCalls(name).length > 0;
  }

  toolCallCount(name: string): number {
    return this.toolCalls(name).length;
  }

  usage(): ContextUsage {
    return summarizeUsage(this.events);
  }

  /** Token usage attributed to each completed turn, in order. */
  turnUsage(): TurnUsage[] {
    const turnUsages: TurnUsage[] = [];
    let pending = zeroTokenUsage();
    for (const event of this.events) {
      if (event.category !== "turn") continue;
      const inner = event.event;
      if (inner.type === "llm_call_ended") {
        const outcome = inner.outcome as LlmCallOutcome;
        if (outcome.status === "completed" && outcome.usage) {
          pending = addUsage(pending, outcome.usage);
        }
      } else if (inner.type === "ended") {
        turnUsages.push({ outcome: inner.outcome, usage: pending });
        pending = zeroTokenUsage();
      }
    }
    return turnUsages;
  }
}

export const ZERO_TOKEN_USAGE: TokenUsage = {
  input_tokens: 0,
  output_tokens: 0,
  cache_read_tokens: null,
  cache_creation_tokens: null,
  input_audio_tokens: null,
  input_video_tokens: null,
  reasoning_tokens: null,
  output_audio_tokens: null,
  accepted_prediction_tokens: null,
  rejected_prediction_tokens: null,
};

function zeroTokenUsage(): TokenUsage {
  return { ...ZERO_TOKEN_USAGE };
}

function addUsage(acc: TokenUsage, usage: TokenUsage): TokenUsage {
  return {
    input_tokens: acc.input_tokens + usage.input_tokens,
    output_tokens: acc.output_tokens + usage.output_tokens,
    cache_read_tokens: addReported(
      acc.cache_read_tokens,
      usage.cache_read_tokens,
    ),
    cache_creation_tokens: addReported(
      acc.cache_creation_tokens,
      usage.cache_creation_tokens,
    ),
    input_audio_tokens: addReported(
      acc.input_audio_tokens,
      usage.input_audio_tokens,
    ),
    input_video_tokens: addReported(
      acc.input_video_tokens,
      usage.input_video_tokens,
    ),
    reasoning_tokens: addReported(acc.reasoning_tokens, usage.reasoning_tokens),
    output_audio_tokens: addReported(
      acc.output_audio_tokens,
      usage.output_audio_tokens,
    ),
    accepted_prediction_tokens: addReported(
      acc.accepted_prediction_tokens,
      usage.accepted_prediction_tokens,
    ),
    rejected_prediction_tokens: addReported(
      acc.rejected_prediction_tokens,
      usage.rejected_prediction_tokens,
    ),
  };
}

function addReported(
  lhs: number | null | undefined,
  rhs: number | null | undefined,
): number | null {
  if ((lhs ?? null) === null && (rhs ?? null) === null) return null;
  return (lhs ?? 0) + (rhs ?? 0);
}

const ZERO_USAGE: ContextUsage = {
  input_tokens: 0,
  output_tokens: 0,
  cache_read_tokens: null,
  cache_creation_tokens: null,
  reasoning_tokens: null,
  usage_ratio: null,
  context_limit: null,
  total_input_tokens: 0,
  total_output_tokens: 0,
  total_cache_read_tokens: 0,
  total_cache_creation_tokens: 0,
  total_reasoning_tokens: 0,
};

export function isTerminalEvent(event: AgentEvent): boolean {
  return event.category === "turn" && event.event.type === "ended";
}

/** Build the terminal turn event, defaulting to a completed turn. */
export function turnEnded(
  outcome: TurnOutcome = { status: "completed" },
): AgentEvent {
  return { category: "turn", event: { type: "ended", outcome } };
}

function extractToolCalls(events: AgentEvent[]): ToolCall[] {
  const calls: ToolCall[] = [];
  for (const event of events) {
    if (event.category !== "tool") continue;
    const tool = event.event;
    if (tool.type === "result") {
      calls.push(new ToolCall(tool.result.name, tool.result.arguments));
    } else if (tool.type === "error") {
      calls.push(new ToolCall(tool.error.name, tool.error.arguments ?? ""));
    }
  }
  return calls;
}

function summarizeUsage(events: AgentEvent[]): ContextUsage {
  for (let i = events.length - 1; i >= 0; i--) {
    const event = events[i];
    if (event?.category === "context" && event.event.type === "usage_updated")
      return event.event.usage;
  }
  return { ...ZERO_USAGE };
}
