# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Overview

macOS 메뉴바 앱으로, Claude Code CLI의 로컬 사용량 추정 + Codex CLI의 공식 rate-limit 관측값을 표시한다. Tauri 2 (Rust 백엔드 + WebView 프런트엔드), 대상은 macOS 13+. 외부 네트워크 호출 없이 로컬 로그/세션 파일 파싱만 수행한다.

## Commands

모든 빌드/실행은 `src-tauri/` 디렉터리 기준으로 동작한다. 프런트엔드는 빌드 스텝 없이 정적 자산(`src/`)을 사용한다.

```bash
# 개발 실행 (Tauri dev)
cd src-tauri && cargo tauri dev

# 릴리스 빌드 + ad-hoc 코드사이닝 (macOS .app 번들 생성)
scripts/package-macos.sh

# 단위 테스트
cd src-tauri && cargo test

# 단일 테스트 실행 (예: parser 모듈)
cd src-tauri && cargo test --lib parser::tests::reads_incremental_with_offset

# lint / format
cd src-tauri && cargo clippy --all-targets && cargo fmt --check
```

`cargo tauri` CLI가 없으면 `cargo install tauri-cli --version 2.11.2` 로 설치한다.

## Architecture

### 백엔드 (`src-tauri/src/`)

- **`runtime.rs`**: Tauri 앱 부팅 시 백그라운드 tokio 태스크를 띄워 폴링 루프와 remote sync 루프를 돌린다. `start_background_runtime`이 진입점이며 parsers (`ClaudeCodeParser`, `CodexParser`), `WindowEstimator`, `UsageStore`를 조립해 `UsageScheduler`에 주입한다.
- **`scheduler.rs`**: 폴링 주기 (`MIN_POLL_INTERVAL_SECS = 60s`)로 parser들에서 delta를 읽고 estimator에 흘려보낸 뒤 reset 타이머를 oneshot으로 재예약. `window_generation_id`로 윈도우 경계 변경을 추적.
- **`parser/`** (`mod.rs` + `claude_code.rs` + `codex.rs`): 로컬 로그/JSONL 파일 파일오프셋 기반 증분 읽기. 공통 트레잇 `LocalLogParser::read_delta() -> Vec<UsageEvent>`. `UsageSource` enum이 두 소스를 구분. Claude Code는 사용량을 직접 합산하지만, Codex는 공식 rate-limit 관측값(`CodexRateLimitStatus`)이 있을 때만 표시하는 정책 (`CODEX_RATE_LIMIT_FRESHNESS_SECS = 300s`).
- **`window_estimator.rs`**: 파싱된 이벤트를 5h(또는 소스별) 윈도우로 누적해 `UsageSnapshot` 생성. Claude의 공식 throttling/usage 관측값으로 로컬 회계를 rebase 하는 로직 포함 (최근 커밋 흐름).
- **`storage.rs`**: rusqlite (bundled) 기반 SQLite. 24h 시계열, 일/주/월 rollups, remote sync 상태를 저장.
- **`tray.rs` + `native_status.rs`**: AppKit (`NSStatusItem`, `NSPopover`) 직접 호출 (objc2 family). 메뉴바 텍스트/팝오버 윈도우 관리. `TrayDisplayState`가 단일 진리원천.
- **`alerts.rs`**: 임계치 기반 알림 (tauri-plugin-notification). `ThresholdEvaluator`로 중복 발사 방지.
- **`autostart.rs`**: macOS `SMAppService` 기반 로그인 항목 등록 (smappservice-rs).
- **`settings.rs` / `config.rs`**: 사용자 설정 (TOML)과 hidden config 분리. DB 경로/디렉터리 결정.
- **`remote_sync.rs`**: 선택적 외부 동기화 (스펙상 "외부 네트워크 호출 없음" 원칙과 충돌할 수 있으므로 변경 시 주의 - 실제 호출 경로 확인 후 작업).
- **`lib.rs`**: `#[tauri::command]` 들이 모인 IPC 표면 (`get_24h_series`, `get_rollups`, `get_current_tray_state`, `get_settings`, `save_settings` 등). 프런트엔드는 여기서만 백엔드 데이터를 가져옴.

### 프런트엔드 (`src/`)

빌드 스텝 없는 정적 HTML/CSS/JS. `tauri.conf.json`의 `frontendDist = "../src"`로 직접 서빙.
- `popover.html/js/css`: 메뉴바 팝오버 UI. `invoke('get_24h_series')` 등으로 데이터 수신, `settings-reloaded` 같은 이벤트 구독.
- `settings.html/js/css`: 별도 설정 윈도우.
- CSP는 `connect-src 'none'`으로 잠겨 있어 프런트엔드에서 외부 fetch 불가.

### 데이터 흐름

```
로컬 로그 파일 → Parser.read_delta() → UsageEvent
                                  ↓
                          WindowEstimator (5h 윈도우 누적 + 공식 관측값 rebase)
                                  ↓
                          UsageSnapshot ─┬─→ UsageStore (SQLite)
                                         ├─→ ThresholdEvaluator → 알림
                                         └─→ update_main_tray → TrayDisplayState
                                                                       ↓
                                              프런트엔드 invoke / Tauri Emit
```

## Conventions

- 폴링 주기는 `MIN_POLL_INTERVAL_SECS` 미만으로 내리지 말 것 (스펙 결정 사항).
- Codex 데이터는 공식 rate-limit 관측값이 있을 때만 표시하는 정책을 유지할 것 (`CODEX_RATE_LIMIT_FRESHNESS_SECS` 신선도 체크).
- 외부 네트워크 호출 추가는 README의 핵심 결정과 CSP 정책에 반하므로 신중히 검토.
- macOS 전용 코드는 `#[cfg(target_os = "macos")]` 가드 유지.

## Dev workflow

- 코드 변경 후 검증 단계에서는 **기존에 떠 있는 dev/release 인스턴스를 모두 종료**한 뒤 `cargo tauri dev` 를 새로 실행한다.
  - 이유: `NSStatusItem` / 메뉴바 아이콘과 `~/Library/Application Support/token-notifier/usage.sqlite` (WAL 잠금) 가 동시 실행 시 충돌한다.
  - 종료 명령 예: `pkill -f "target/debug/token-notifier"` + 기존 release `.app` 도 Cmd+Q.
- `cargo tauri dev` 가 가진 파일 watcher 의 hot-reload 는 Rust 변경 시 항상 안정적이지 않다 (특히 native AppKit 코드). 변경 후엔 dev 를 **명시적으로 종료 후 재실행**해 새 바이너리로 띄울 것.

## 설계 문서

- 요구사항: `.omc/specs/deep-interview-token-notifier.md`
- 구현 계획: `.omc/plans/token-notifier-consensus-plan.md`
- 패키징: `docs/PACKAGING.md`
