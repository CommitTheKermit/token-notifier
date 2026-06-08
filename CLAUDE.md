# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Overview

macOS 메뉴바 앱으로, Claude Code 와 Codex(ChatGPT) 의 5시간/주간 rate-limit 사용률을 메뉴바에 표시한다. Tauri 2 (Rust 백엔드 + WebView 팝오버), 대상은 macOS 13+.

표시값의 1차 출처는 **각 vendor 의 공식 usage API** 다. Claude Code 는 `api.anthropic.com/api/oauth/usage`, Codex 는 `chatgpt.com/backend-api/wham/usage` 를 호출하며, **인증 토큰은 CLI 가 이미 로컬에 저장해 둔 자격증명을 재사용**한다(키체인 / `~/.codex/auth.json`). 공식 관측값이 없을 때만 로컬 로그/세션 파일 파싱으로 사용량을 추정한다. 즉 "네트워크 호출 없음" 이 아니라 "사용자에게 추가 인증을 요구하지 않고 CLI 자격증명으로 공식 수치를 가져온다" 가 핵심 설계다.

선택적으로 OpenAI/Anthropic 의 **Admin Usage API** (`remote_sync`) 를 켜면 시간당 토큰 소비량 시계열을 받아 24h 차트를 채운다.

## Commands

모든 빌드/실행은 `src-tauri/` 디렉터리 기준으로 동작한다. 프런트엔드는 빌드 스텝 없이 정적 자산(`src/`)을 사용한다.

```bash
# 개발 실행 (Tauri dev)
cd src-tauri && cargo tauri dev

# 릴리스 빌드 + ad-hoc 코드사이닝 (macOS .app 번들 생성)
scripts/package-macos.sh

# 단위 테스트
cd src-tauri && cargo test

# 단일 테스트 실행 (예: codex rate-limit 파싱)
cd src-tauri && cargo test --lib parser::codex::tests

# lint / format
cd src-tauri && cargo clippy --all-targets && cargo fmt --check
```

`cargo tauri` CLI가 없으면 `cargo install tauri-cli --version 2.11.2` 로 설치한다.

remote_sync(Admin API) 경로는 환경변수에서 키를 읽는다: `ANTHROPIC_ADMIN_KEY`(또는 `ANTHROPIC_API_KEY`), `OPENAI_ADMIN_KEY`(또는 `OPENAI_API_KEY`). 설정에서 `remote_sync.enabled` 가 켜져 있어야 동작한다.

## Architecture

### 표시값 우선순위 (가장 중요)

화면(메뉴바/팝오버)에 찍히는 수치는 `runtime.rs::publish_tray_state` 가 `TrayDisplayState` 를 조립하며 결정한다. 소스별로 fallback 단계가 **비대칭**이다:

- **Claude Code (`apply_live_claude_rate_limit`)**: 공식 관측값이 있으면 그것만 표시(`status_source = official_observation`, `estimated = false`). 없으면 수치를 비우고 `official_unavailable` 로 둔다. **로컬 추정으로 fallback 하지 않는다.**
- **Codex (`apply_live_codex_rate_limit`)**: 3단계 - 신선한 공식값(<=5분, `official_observation`) → 오래된 공식값(`stale_observation`, 여전히 공식 %로 로컬 추정을 덮어씀) → 공식값 없으면 로컬 추정(`local_estimate`) 또는 `unavailable`.

`percent_used` 필드는 공식 경로에서 **잔여 %(`remaining_percent`)** 를 담는다(이름과 의미가 어긋나니 주의). 알림은 `estimated = true` 스냅샷에서는 절대 발사하지 않는다(`maybe_send_alert` early-return).

### 백엔드 (`src-tauri/src/`)

- **`runtime.rs`**: 부팅 진입점 `start_background_runtime`. tokio 태스크 2개를 띄운다 - (1) 메인 폴링 루프(`MIN_POLL_INTERVAL_SECS` 주기 + `request_immediate_refresh` 의 `Notify` 로 즉시 깨우기 가능), (2) `start_remote_sync_runtime`(Admin API 동기화, 자체 간격). 폴링 결과에 공식 rate-limit 관측값을 입혀 tray 를 갱신하고 알림을 평가한다.
- **`scheduler.rs`**: `UsageScheduler` 가 parser 들의 `read_delta()` 를 모아 `UsageStore` 에 dedup 기록 후 estimator 에 흘린다. `window_generation_id`(AtomicU64) 로 윈도우 경계 변경을 추적해, 리셋 oneshot 이 도는 동안 들어온 stale 폴링 결과를 버린다(`commit_events_if_fresh`). `MIN_POLL_INTERVAL_SECS = 90`.
- **`parser/`** (`mod.rs` + `claude_code.rs` + `codex.rs`): 공통 트레잇 `LocalLogParser`.
  - `read_delta()` 는 로컬 로그를 파일오프셋 기반으로 증분 읽어 `UsageEvent`(로컬 추정용)를 만든다.
  - `latest_rate_limit_status()` 는 **공식 관측값**을 반환한다(`claude_code.rs`/`codex.rs` 각각). 여기서 reqwest 로 vendor API 를 호출하고 메모리/디스크 캐시(`RATE_LIMIT_FETCH_TTL`)를 둔다.
  - **`claude_code.rs`**: `GET api.anthropic.com/api/oauth/usage` (`anthropic-beta: oauth-2025-04-20`). OAuth 토큰은 keychain 서비스 `"Claude Code-credentials"`(또는 파일)에서 읽고, 401/429 면 `console.anthropic.com/v1/oauth/token` 으로 refresh 후 재시도하고 새 토큰을 다시 저장한다. 응답의 `five_hour`/`seven_day` `utilization` 사용.
  - **`codex.rs`**: `GET chatgpt.com/backend-api/wham/usage` (Bearer = `~/.codex/auth.json` 의 `access_token`, `ChatGPT-Account-Id` 헤더). 네트워크 실패 시 `~/.codex/sessions/*` 세션 파일에 Codex CLI 가 적어둔 rate-limit 라인을 파싱하는 fallback 보유.
- **`window_estimator.rs`**: `UsageEvent` 를 5h(또는 `default_window_secs`) 윈도우로 누적해 `UsageSnapshot`(`percent_used = tokens_used*100/quota_tokens`, `estimated` 플래그) 생성. 공식 관측값이 없는 경우의 추정 경로.
- **`storage.rs`**: rusqlite(bundled) SQLite. 24h 시계열, 일/주/월 rollups, parser 오프셋/상태, `remote_hourly_bucket`(있으면 로컬 시간당 집계를 override), `remote_sync_state` 저장.
- **`remote_sync.rs`**: 선택적. OpenAI/Anthropic Admin Usage API 를 Admin 키로 호출해 시간당 토큰 소비 버킷을 받아 24h 차트에 채운다. tray 의 5h/주간 % 와는 별개 경로다.
- **`tray.rs` + `native_status.rs`**: AppKit(`NSStatusItem`, `NSPopover`) 직접 호출(objc2). `TrayDisplayState` 가 표시의 단일 진리원천.
- **`alerts.rs`**: 임계치 알림(tauri-plugin-notification). `ThresholdEvaluator` 로 중복 발사 방지. 공식 관측값에만 적용.
- **`autostart.rs`**: macOS `SMAppService` 로그인 항목(smappservice-rs).
- **`settings.rs` / `config.rs`**: 사용자 설정(TOML)과 hidden config 분리. DB 경로/윈도우 길이(`DEFAULT_WINDOW_SECS = 5h`) 결정.
- **`lib.rs`**: `#[tauri::command]` IPC 표면 - `get_24h_series`, `get_rollups`, `get_remote_sync_states`, `get_current_tray_state`, `get_settings`, `save_settings`, `get_autostart_status`, `open_login_items_settings`.

### 프런트엔드 (`src/`)

빌드 스텝 없는 정적 HTML/CSS/JS. `tauri.conf.json` 의 `frontendDist = "../src"` 로 직접 서빙. 구성은 메뉴바 팝오버 UI(`index.html` + `popover.html/css/js` + `vendor/`)다. `invoke('get_24h_series')` 등으로 데이터 수신, `usage-update`/`usage-reset` 이벤트 구독.

CSP 는 `connect-src 'none'` 으로 잠겨 있다. 이는 **프런트엔드(WebView)** 의 외부 fetch 만 차단하는 것이고, 공식 usage API 호출은 전부 **Rust 백엔드**에서 일어난다.

### 데이터 흐름

```
[공식 경로 - tray의 5h/주간 %]
 CLI 저장 자격증명(keychain / ~/.codex/auth.json)
   → parser::latest_rate_limit_status() → vendor 공식 usage API
   → runtime::apply_live_*  → TrayDisplayState → 메뉴바/팝오버 + 알림

[로컬 추정 경로 - 공식값 없을 때 fallback]
 로컬 로그(~/.claude/projects, ~/.codex/sessions)
   → Parser.read_delta() → UsageEvent → WindowEstimator → UsageSnapshot
   → UsageStore(SQLite) + (Codex 한정) tray fallback

[Admin API 경로 - 24h 차트, 선택]
 ANTHROPIC/OPENAI Admin 키 → remote_sync → remote_hourly_bucket → get_24h_series
```

## Conventions

- 폴링 주기는 `MIN_POLL_INTERVAL_SECS`(90s) 미만으로 내리지 말 것.
- Codex 는 공식 관측값 신선도를 `CODEX_RATE_LIMIT_FRESHNESS_SECS`(5분)로 체크해 fresh/stale 을 구분한다. 이 정책을 유지할 것.
- Claude Code 표시 경로는 공식 관측값 전용이다. 로컬 추정으로 fallback 시키는 변경은 설계 결정에 반하므로 신중히.
- 알림은 공식 관측값(`estimated = false`)에서만 발사한다.
- vendor 공식 엔드포인트/헤더(`anthropic-beta`, `ChatGPT-Account-Id` 등)와 OAuth client_id 는 CLI 동작을 모사한 것이므로 변경 시 실제 호출이 깨지지 않는지 확인.
- macOS 전용 코드는 `#[cfg(target_os = "macos")]` 가드 유지.

## Dev workflow

- 코드 변경 후 검증 단계에서는 **기존에 떠 있는 dev/release 인스턴스를 모두 종료**한 뒤 `cargo tauri dev` 를 새로 실행한다.
  - 이유: `NSStatusItem` / 메뉴바 아이콘과 `~/Library/Application Support/token-notifier/usage.sqlite` (WAL 잠금) 가 동시 실행 시 충돌한다.
  - 종료 명령 예: `pkill -f "target/debug/token-notifier"` + 기존 release `.app` 도 Cmd+Q.
- `cargo tauri dev` 의 hot-reload 는 Rust 변경(특히 native AppKit 코드)에 항상 안정적이지 않다. 변경 후엔 dev 를 **명시적으로 종료 후 재실행**해 새 바이너리로 띄울 것.
- **코드/파일 변경을 동반한 작업이 끝나면**: release `.app` 을 새로 빌드(`scripts/package-macos.sh`)하고, 기존에 떠 있는 앱(dev 바이너리 + release `.app`)을 모두 종료한 뒤 새 `.app` 을 실행한다.
  - 종료: `pkill -f "target/debug/token-notifier"` + `pkill -f "Token Notifier.app"`.
  - 실행: `open "src-tauri/target/release/bundle/macos/Token Notifier.app"`.
  - 이유: 메뉴바 native 렌더링은 dev hot-reload 로 검증이 불안정하므로, 최종 결과는 항상 release `.app` 으로 확인한다.

## 설계 문서

- 요구사항: `.omc/specs/deep-interview-token-notifier.md`
- 구현 계획: `.omc/plans/token-notifier-consensus-plan.md`
- 패키징: `docs/PACKAGING.md`
