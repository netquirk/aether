The root of an Aether settings file (`.aether/settings.json`). It selects the
default agent and defines the agents, prompt sources, MCP servers, and provider
overrides available to a project.

## Basic Examples
A minimal project with a single user-invocable agent:

```json
{
  "agent": "Build",
  "agents": [
    {
      "name": "Build",
      "description": "Builds features and fixes bugs",
      "model": "anthropic:claude-sonnet-4-5-20250929",
      "userInvocable": true
    }
  ]
}
```

A fuller setup with shared prompts, an MCP source, and a provider override:

```json
{
  "agent": "Build",
  "prompts": ["AGENTS.md"],
  "mcps": [".aether/mcp.json"],
  "providers": {
    "anthropic": { "auth": "default" }
  },
  "agents": [
    {
      "name": "Build",
      "description": "Builds features and fixes bugs",
      "model": "anthropic:claude-sonnet-4-5-20250929",
      "reasoningEffort": "high",
      "userInvocable": true,
      "prompts": [".aether/BUILD.md", "AGENTS.md"]
    }
  ]
}
```

An encrypted file credential store using a passphrase from the environment:

```json
{
  "credentialsStore": {
    "type": "encryptedFile",
    "passwordEnv": "PASSWORD_ENV_VAR_NAME"
  },
  "agents": [
    {
      "name": "Build",
      "description": "Builds features and fixes bugs",
      "model": "anthropic:claude-sonnet-4-5-20250929",
      "userInvocable": true
    }
  ]
}
```

## OpenTelemetry

OpenTelemetry is enabled by adding a `telemetry` section to
settings. User-level telemetry settings merge with project-level, with project-level taking priority.
Prompt, response, reasoning, and tool content is not exported unless the matching `telemetry.content`
flag is explicitly set to `true`.

```json
{
  "telemetry": {
    "serviceName": "aether",
    "sampleRatio": 1.0,
    "content": {
      "systemInstructions": false,
      "inputMessages": false,
      "outputMessages": false,
      "toolDefinitions": false,
      "toolCalls": false
    },
    "traces": { "enabled": true },
    "metrics": { "enabled": true },
    "otlp": {
      "endpoint": "http://localhost:4318"
    }
  },
  "agents": [
    {
      "name": "Build",
      "description": "Builds features and fixes bugs",
      "model": "anthropic:claude-sonnet-4-5-20250929",
      "userInvocable": true
    }
  ]
}
```

All `content` flags default to `false`; enable only the content attributes appropriate for
your collector and its access controls. See the website telemetry reference for the complete
attribute mapping.

For an OTLP backend with exact signal URLs, set `otlp.tracesEndpoint` and `otlp.metricsEndpoint`. Aether sends each configured signal to its matching URL unchanged; an unconfigured signal uses the `/v1/traces` or `/v1/metrics` URL derived from `otlp.endpoint`.

## Run-time warnings

The top-level `run` block tunes the headless CLI's behaviour during a single
run. Every field is optional; an unset field disables the corresponding
behaviour so an absent `run` block leaves the existing run shape unchanged.

`providerStallWarnSeconds` sets how long a provider call may wait before the
CLI prints a one-line warning naming the elapsed wait. The warning fires
once per stalled call while the turn is still live so a hung turn is
visible instead of silent; `0` or omitted disables the warning.

```json
{
  "run": { "providerStallWarnSeconds": 30 },
  "agents": [
    {
      "name": "Build",
      "description": "Builds features and fixes bugs",
      "model": "anthropic:claude-sonnet-4-5-20250929",
      "userInvocable": true
    }
  ]
}
```

## Tool output cap

Aether caps the byte length of every tool result before it reaches the model. The
cap keeps the head and tail of the original text, writes the full output to a
content-hashed file on disk, and embeds a marker in the model-visible text
naming the file so the model can read it back in ranges. The on-disk filename
is a hex content hash, so the same `(tool, cap)` pair produces byte-identical
text on every call — including across runs — which keeps prompt caches warm.

The cap is 16 KiB by default. Configure it with the top-level `toolOutput`
block; agents can override it per-agent with their own `toolOutput` field.
Two environment variables always win over the settings file:

- `AETHER_TOOL_OUTPUT_MAX_BYTES` overrides `maxBytes`. `0` disables the cap.
- `PRAIRIE_TOOL_OUTPUT_DIR` overrides `outputDir`.

A project that caps tool results at 32 KiB and writes full outputs to
`/var/log/aether`:

```json
{
  "toolOutput": {
    "maxBytes": 32768,
    "outputDir": "/var/log/aether"
  },
  "agents": [
    {
      "name": "Build",
      "description": "Builds features and fixes bugs",
      "model": "anthropic:claude-sonnet-4-5-20250929",
      "userInvocable": true
    }
  ]
}
```

An agent that disables the cap while leaving the top-level default in place:

```json
{
  "toolOutput": { "maxBytes": 32768 },
  "agents": [
    {
      "name": "Audit",
      "description": "Inspects verbose CI logs that must reach the model verbatim",
      "model": "anthropic:claude-sonnet-4-5-20250929",
      "userInvocable": true,
      "toolOutput": { "maxBytes": 0 }
    }
  ]
}
```

## Shell environment for run commands

Every shell command a run starts (the `bash` tool of the built-in `coding`
MCP server) inherits the variables from this list, merged over the process
environment of the running Aether process. A configured key shadows whatever
the OS process would otherwise have seen — useful for pinning `PATH`,
injecting a project-local SDK directory, or tagging runs with a stable
identifier. The internal `AETHER_MCP_IPC_SOCKET` gateway variable is always
written by the MCP runtime after this map, so config-supplied values cannot
spoof the gateway socket.

```json
{
  "shellEnvironment": {
    "PATH": "/opt/build-tools/bin:${PATH}",
    "BUILD_TAG": "staging"
  },
  "agents": [
    {
      "name": "Build",
      "description": "Builds features and fixes bugs",
      "model": "anthropic:claude-sonnet-4-5-20250929",
      "userInvocable": true
    }
  ]
}
```

