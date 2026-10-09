# Release notes — Tool output cap (Aether)

This release makes tool-result capping the default behavior in Aether. Long
tool outputs (over 16 KiB) are now truncated to a head-and-tail preview, with a
marker line that names the on-disk file holding the full output. The same
`(tool, cap)` pair always produces byte-identical text — including across
runs — so prompt caches stay warm.

## What ships

- **Default cap: 16 KiB** of tool result text reaches the model. Anything
  larger is truncated to a head+tail preview with an explicit marker line.
- **Full output saved to disk**: every truncated result writes the complete
  tool output to `<sha256>.log` under the configured spillover directory
  (default `<project_root>/.prairie/out`). The marker tells the model where
  to read it back with `read`, `grep`, or `tail`.
- **Deterministic marker**: `elided <N>/<total> bytes and <N>/<total> lines.
  Full: <path>.` — identical bytes on every call so the prompt cache key
  doesn't churn.
- **Cap is fully off-able**: `maxBytes: 0` (or `AETHER_TOOL_OUTPUT_MAX_BYTES=0`)
  disables the cap and the on-disk spill for the whole session.

## Configuration

### `settings.json`

A new top-level block, `toolOutput`, sets the global defaults:

```jsonc
{
  "toolOutput": {
    "maxBytes": 16384,        // 0 to disable; unset inherits the 16 KiB default
    "outputDir": ".prairie/out" // directory the cap writes full output under
  }
}
```

Per-agent overrides live on `agentConfig.toolOutput`:

```jsonc
{
  "agentConfig": {
    "toolOutput": { "maxBytes": 65536 }
  }
}
```

### Environment variables (win over settings)

| Variable                     | Effect                                                       |
|------------------------------|--------------------------------------------------------------|
| `AETHER_TOOL_OUTPUT_MAX_BYTES` | Byte budget the model sees. `0` disables the cap entirely.   |
| `PRAIRIE_TOOL_OUTPUT_DIR`      | Directory the cap writes the full output to.                 |

## Schema and docs

- The full JSON Schema for the new block is in
  `packages/website/src/data/aether-settings.schema.json`.
- The user-facing docs are in
  `crates/aether-project/src/docs/aether_settings.md`.

## End-to-end test

`crates/mcp-servers/tests/integration/tool_output_cap_e2e.rs`
(`bash_tool_one_mib_output_gets_capped_and_full_bytes_recoverable`) drives a
real `bash` tool call that produces 1 MiB of stdout, runs the response
through the production bridge with a 4 KiB cap, and asserts:

- the model-visible result fits in the cap and carries the marker;
- the on-disk file parses back to the exact 1 MiB bash output;
- the marker names the saved file.

## Prairie followup

- Drop the `prairie-toolcap` wrapper mount in the Prairie release. Aether
  handles the cap directly; the wrapper is no longer needed.
- Keep `PRAIRIE_TOOL_OUTPUT_DIR` as the env provider — Aether reads it on
  every `run` start.
- Use Prairie's `tooloutreport` to measure token use per engineer call
  before/after release; expect a meaningful drop on tool-heavy workloads.