# Log Format Spike

Date: 2026-05-21

Source inputs:
- Spec: `.omc/specs/deep-interview-token-notifier.md`
- Plan: `.omc/plans/token-notifier-consensus-plan.md`

Scope: Plan Step 1 only. This document records local file inventory, observed formats, rate-limit metadata availability, and the autostart mechanism conclusion. No implementation step was started.

## Pass-Criteria Summary

| Criterion | Conclusion |
| --- | --- |
| CC log format | JSONL |
| CX log format | SQLite for aggregate thread token usage; JSONL history exists but does not contain token usage |
| CC window metadata | No. Not found in sampled `~/.claude/projects/*/*.jsonl` usage entries |
| CX window metadata | No. Not found in `~/.codex/state_5.sqlite` or `~/.codex/logs_2.sqlite` schema |
| CC quota source | Unknown from logs; must use plan fallback estimator until a local quota source is discovered |
| CX quota source | Unknown from logs; must use plan fallback estimator until a local quota source is discovered |
| Autostart mechanism | Preserve plan decision: SMAppService. Current Tauri autostart plugin docs/default examples use LaunchAgent, so Step 2 must not assume the plugin alone satisfies this decision |

## Claude Code Inventory

Observed candidate paths:
- `~/.claude/projects/*/*.jsonl`
- `~/.claude/settings.json`
- `~/.claude/stats-cache.json`
- `~/.claude/.session-stats.json`

Not observed in this environment:
- `~/.claude/usage.json`
- `~/.claude/history/`

Representative project-session path:
- `~/.claude/projects/-Users-ujeonghyeon-Desktop-dev-myDev-token-notifier/94105052-cd43-43d1-a7bb-3c003a964123.jsonl`

Observed JSONL shape:
- Initial records can contain only session linkage fields such as `leafUuid`, `sessionId`, and `type`.
- Assistant records with usage contain fields equivalent to:

```json
{
  "type": "assistant",
  "timestamp": "2026-05-21T01:01:24.096Z",
  "message": {
    "model": "claude-opus-4-7",
    "usage": {
      "input_tokens": 6,
      "cache_creation_input_tokens": 726,
      "cache_read_input_tokens": 154039,
      "output_tokens": 343,
      "service_tier": "standard"
    }
  }
}
```

Parser implication:
- `ClaudeCodeParser` can read `~/.claude/projects/**/*.jsonl`.
- It should treat each assistant message with `message.usage` as a usage event.
- Token total should include at least `input_tokens`, `output_tokens`, `cache_creation_input_tokens`, and `cache_read_input_tokens`, with the exact weighting kept in `WindowEstimator` or an explicit token normalization function.
- No rate-limit window start, reset time, duration, quota, or remaining-percent field was found in the sampled JSONL entries.

## Codex CLI Inventory

Observed candidate paths:
- `~/.codex/state_5.sqlite`
- `~/.codex/logs_2.sqlite`
- `~/.codex/history.jsonl`
- `~/.codex/log/codex-tui.log`
- `~/.codex/version.json`

Not observed in this environment:
- `~/.cache/codex/*`
- Codex JSONL usage logs with token usage records

`~/.codex/state_5.sqlite` has the useful aggregate signal:

```sql
CREATE TABLE threads (
    id TEXT PRIMARY KEY,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    cwd TEXT NOT NULL,
    tokens_used INTEGER NOT NULL DEFAULT 0,
    model TEXT,
    reasoning_effort TEXT
);
```

Representative row shape:

```text
thread_id | created_at | updated_at | tokens_used | model | reasoning_effort | cwd
019e480d... | 2026-05-21 01:02:02 | 2026-05-21 01:04:24 | 307706 | gpt-5.5 | medium | /Users/ujeonghyeon/Desktop/dev/myDev/token-notifier
```

`~/.codex/logs_2.sqlite` appears to be operational telemetry rather than the primary usage source:

```sql
CREATE TABLE logs (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    ts INTEGER NOT NULL,
    ts_nanos INTEGER NOT NULL,
    level TEXT NOT NULL,
    target TEXT NOT NULL,
    feedback_log_body TEXT,
    estimated_bytes INTEGER NOT NULL DEFAULT 0
);
```

Parser implication:
- `CodexParser` should prefer `~/.codex/state_5.sqlite` and read deltas from `threads.tokens_used` grouped by `thread id`.
- `logs_2.sqlite` may be useful for diagnostics but is too noisy for primary usage accounting.
- `history.jsonl` stores prompt history, not reliable token usage.
- No rate-limit window start, reset time, duration, quota, or remaining-percent field was found in the sampled Codex SQLite schemas.

## Window Metadata and Quota

CC:
- Window metadata explicit: no.
- Discovery location: sampled `~/.claude/projects/*/*.jsonl` assistant usage records.
- Quota source: unknown.

CX:
- Window metadata explicit: no.
- Discovery location: `~/.codex/state_5.sqlite` and `~/.codex/logs_2.sqlite` schemas plus sampled rows.
- Quota source: unknown.

Conclusion:
- Plan option B3 remains necessary: metadata-first if discovered later, fallback to `DEFAULT_WINDOW_SECS: u64 = 5 * 3600`.
- The fallback path should mark the display as estimated, because Step 1 did not find enough local metadata to prove AC11-grade reset accuracy from logs alone.

## Autostart Mechanism

Plan decision to preserve:
- macOS 13+ SMAppService automatic startup.

Evidence checked:
- Tauri v2 autostart documentation initializes `tauri_plugin_autostart` with `MacosLauncher::LaunchAgent`.
- The Tauri plugin source for the v2 branch exposes `MacosLauncher::{LaunchAgent, AppleScript}` and passes `set_use_launch_agent(...)` into `auto_launch`.
- The underlying `auto-launch` crate documentation now lists `MacOSLaunchMode::{LaunchAgent, AppleScript, SMAppService}`.

Conclusion:
- The app-level mechanism remains SMAppService to match the plan.
- Step 2 should not wire `tauri-plugin-autostart` in its documented default `LaunchAgent` mode and call that done.
- Implementation options for Step 2/8 need approval before changing the plan:
  - use `auto-launch` directly with `MacOSLaunchMode::SMAppService`, if compatible with Tauri packaging, or
  - keep `tauri-plugin-autostart` only if its version exposes SMAppService by implementation time, or
  - update the plan/spec to LaunchAgent with an explicit rationale.

References:
- Tauri Autostart docs: https://v2.tauri.app/plugin/autostart/
- Tauri plugin source: https://raw.githubusercontent.com/tauri-apps/plugins-workspace/v2/plugins/autostart/src/lib.rs
- `auto-launch` docs: https://docs.rs/auto-launch/latest/auto_launch/enum.MacOSLaunchMode.html

## Acceptance-Criteria Impact

Potentially affected criteria:
- AC1: CC token deltas are available within JSONL, but percent accuracy depends on quota/window estimation.
- AC2: CX token deltas are available from `state_5.sqlite`, so the plan's CX stub failure branch is not triggered in this environment. Percent accuracy still depends on quota/window estimation.
- AC11: Reset time cannot be proven from local CC/CX logs discovered in Step 1. The fallback estimator must surface estimated state and cannot claim exact reset accuracy without a future metadata source.
- AC12: SMAppService remains the plan target, but the current Tauri plugin default path does not satisfy it without an additional integration decision.

Suggested plan/spec update proposal, not applied in this step:
- Plan Step 3 parser outputs should explicitly support `CodexStateSqliteParser` over `~/.codex/state_5.sqlite`.
- Plan risk R1 should mention that both CC and CX currently lack discovered window/quota metadata.
- AC11 verification should be split into two cases: exact when metadata exists, estimated when only fallback is available. This would affect the spec and plan, so it needs approval before editing either file.
- Step 2/8 autostart implementation should explicitly choose the SMAppService integration route instead of relying on the documented Tauri plugin LaunchAgent default.

## Step 1 Status

Status: complete.

Stop condition reached:
- All Step 1 pass-criteria fields have an evidence-backed conclusion.
- Artifact saved at `docs/spike/LOG_FORMAT.md`.
- No Step 2 work has been started.
