# Token Notifier - Consensus Implementation Plan (RALPLAN-DR short mode)

- **Status: PENDING APPROVAL** (consensus reached at iteration 2; awaiting explicit execution approval)
- Source spec: `/Users/ujeonghyeon/Desktop/dev/myDev/token-notifier/.omc/specs/deep-interview-token-notifier.md`
- Spec ambiguity: 16.4% (PASSED)
- Mode: short (greenfield, no destructive/security/migration risk)
- Consensus: Architect v2 APPROVE, Critic v2 APPROVE (10/10 required improvements + 5/5 quality areas PASS)
- Date: 2026-05-21

---

## 1. Requirements Summary

- macOS 메뉴바 상단에 Claude Code CLI(CC)와 Codex CLI(CX) 각각의 rate-limit 윈도우 남은 % (큰 글자) + 리셋까지 남은 시간 (작은 글자) 상시 표시.
- 데이터 수집은 외부 네트워크 호출 금지, `~/.claude/` 및 Codex 로컬 캐시의 로그/세션 파일 파싱만 사용.
- 메뉴바 갱신 주기 60초 이상 (배터리 우선), 리셋 알림은 별도 oneshot 타이머로 정확하게 발사.
- 메뉴바 색상 구간 0~70 기본 / 70~90 노랑 / 90+ 빨강 고정, 사용자 커스터마이즈 비목표.
- 팝오버: 24h 라인 그래프 + 일/주/월 누적 숫자 (모델별 분해/세션 목록 비포함).
- Settings 4항목만: CC on/off, CX on/off, 소스별 임계치 자유 입력(1~3개), 로그인 시 자동 시작.
- 기술 스택: Tauri 2.x (Rust 백엔드 + WebView 프런트). macOS만 지원.

## 2. RALPLAN-DR Summary (short mode)

### Principles
1. **외부 네트워크 호출 0건.** 모든 데이터는 로컬 파일에서만 파생. AC9가 곧 신뢰의 근거.
2. **배터리 친화 폴링 + 정확성 분리 타이머.** 표시 갱신(>=60s)과 리셋 시각 fire(oneshot)을 절대 섞지 않는다.
3. **메뉴바는 항상 한 줄 - 정보 압축이 곧 가치.** "CC 73% CX 91%" + 작은 리셋 텍스트 이상으로 절대 늘리지 않는다.
4. **추정은 명시적으로 분리.** 윈도우 길이/한도 추정 로직(`WindowEstimator`)을 파서와 분리해 스파이크 결과에 따라 교체 가능.
5. **사용자 노출 UI 설정은 4개로 고정.** 스코프 크리프(색상 커스터마이즈, 모델 분해 등) 방지. (개발자 디버그 override는 사용자 노출 UI가 아니므로 본 원칙과 무관 - 아래 B3 참조.)

### Decision Drivers (top 3)
1. **AC1/AC2/AC11 정확성** - 1분 이내 반영 + 리셋 시각 정확 fire (둘은 별도 메커니즘).
2. **AC5 배터리/CPU 무시 가능 수준** - Activity Monitor 기준 1% 미만 유지.
3. **개인용 단일 머신 데스크톱 - 운영 복잡도 최소화** (코드 서명/배포는 ad-hoc 수준 OK).

### Viable Options

#### Option A. 로그 파싱 전략
- **A1. Pull 모델 (60s 폴링 + 전체 재읽기 with offset 기억).**
  - pros: 구현 단순, FSEvents 권한 다이얼로그 회피, 1분 갱신 요구에 정확히 부합.
  - cons: 로그 파일이 매우 커지면 매 폴링마다 IO 증가 (offset 기반 incremental read로 완화).
- **A2. Push 모델 (`notify` crate FSEvents 워치 + 디바운스).**
  - pros: idle IO ~ 0, 변경 즉시 반응 (이론상 sub-second 갱신).
  - cons (정량적 거부 사유):
    1. `notify` crate macOS backend(FSEvents)는 일부 에디터/CLI의 atomic rename + truncate-write rotate 패턴에서 이벤트를 누락한다고 보고됨 (notify-rs GitHub issues #240, #345 계열). CC/CX가 어떤 write 패턴을 쓰는지는 Step 1 전엔 미확정 → push 모델은 silent miss 리스크.
    2. 디바운스 윈도우(보통 100-500ms) + 다중 파일 디스크립터 합치기 로직이 필요. 코드 복잡도 추정: scheduler.rs +150~200 LoC vs A1의 +30~50 LoC.
    3. FSEvents callback이 별도 thread에서 발사되므로 offset 추적 mutex 경합 → race condition 표면적 ↑.
    4. AC1/AC2가 "1분 이내"이므로 sub-second 반응은 over-engineering.
- **Planner 권장: A1.** 위 4가지 정량 trade-off를 종합하면 A1이 단순성/정확성 모두 우위. A2는 향후 옵션으로 보류.

#### Option B. 윈도우 추정 방식
- **B1. 로그에 명시된 윈도우 메타데이터를 신뢰 (있을 경우).**
  - pros: 정확. 스파이크에서 발견되면 즉시 채택.
  - cons: 포맷 변경 리스크. CC/CX 둘 다 명시 안 될 수도.
- **B2. 첫 사용 시각 + 고정 길이(예: 5h)로 추정.**
  - pros: 메타 없어도 동작. 단순.
  - cons (정량적): 윈도우 길이 ±10분 오차 시 % 표시 ±3%p 오차 (예: 5h 윈도우에서 10분 차이 = 3.3%). AC11 ±100ms 정확성 요구와 양립 불가. 플랜별 길이 차이(Pro/Team) 미보정.
- **B3. Hybrid - 메타데이터 우선 + fallback은 코드 상수(`DEFAULT_WINDOW_SECS: u64 = 5 * 3600`)로 고정.**
  - pros: 견고. 사용자 UI에 노출되지 않으므로 Principle 5(UI 4항목)와 충돌 없음. 디버그 시에는 dev-only 환경변수(`TOKEN_NOTIFIER_DEBUG_WINDOW_SECS`)로 일시 override 가능하되 영구 보존 안 함.
  - cons: 메타데이터 없을 때 정확도는 B2 수준으로 떨어짐. → R2 mitigation에서 트레이에 `~` 표식.
- **Planner 권장: B3 (코드 상수 fallback).** 1단계 스파이크에서 B1이 확정되면 fallback 경로는 dead code 아닌 보험으로 유지. hidden config 파일 방식은 채택하지 않는다 (사용자 발견성 0이라 가치 없음).

#### Option C. 차트 라이브러리
- **C1. Chart.js (CDN-less, 단일 파일 번들).**
  - pros: 라인 차트 1개에 충분, 라이선스 자유, 번들 작음.
  - cons: 의존성 1개 추가.
- **C2. 순수 SVG/Canvas 수기 구현.**
  - pros: 0 의존성.
  - cons: 24h 라인 그래프 1개에 너무 비용 큼.
- **Planner 권장: C1.** 24h 라인 + 누적 숫자라는 단일 화면에 가장 비용 효율적.

#### Invalidation rationale (배제된 안)
- **Swift 네이티브 메뉴바 앱.** Round 10에서 Tauri로 확정. 추가 검토 불필요.
- **공식 API 사용.** Constraints에 의해 명시적으로 배제.
- **모델별/세션별 드릴다운 UI.** Non-Goals에 의해 배제.

---

## 3. Acceptance Criteria (검증 방법 추가)

| ID | 기준 | 검증 방법 |
|----|------|----------|
| AC1 | CC 세션 진행 시 1분 이내 반영 | 통합 테스트: fake `~/.claude` fixture에 줄 추가 후 60s tick 트리거 → 트레이 라벨 업데이트 단위 테스트로 검증 |
| AC2 | CX 세션 진행 시 1분 이내 반영 | 동일 패턴의 fixture 기반 테스트 (CX 로그 위치 스파이크 결과 사용) |
| AC3 | `CC {n}% CX {n}%` + 작은 리셋 텍스트 | 트레이 라벨 포맷 단위 테스트 + macOS 실기 스크린샷 검증 |
| AC4 | 0~70 기본 / 70~90 노랑 / 90+ 빨강 | `color_for_percent(p)` 단위 테스트 (경계값 69/70/71/89/90/91) |
| AC5 | 60s+ 주기, Activity Monitor 1% 미만 | 수동: 앱 30분 실행 후 Activity Monitor 평균 CPU 확인 |
| AC6 | 클릭 시 팝오버 + 24h 라인 차트 | 수동 클릭 시나리오 + 24개 시간버킷 mock으로 차트 렌더 확인 |
| AC7 | 일/주/월 누적 숫자 표시 | DB에 시드 데이터 주입 후 `/api/rollups` Tauri command 응답 검증 |
| AC8 | 임계치 도달 시 알림, 같은 윈도우 내 중복 방지 | 단위 테스트: `ThresholdEvaluator`에 동일 윈도우 ID로 2회 호출 시 1회만 emit |
| AC9 | 네트워크 차단 환경에서 정상 동작 | 수동: macOS Network Link Conditioner로 100% loss 설정 후 % 정상 표시 확인 |
| AC10 | Settings 4항목 즉시 반영 | 수동: 각 토글/입력 변경 시 1초 이내 트레이 반영 확인 |
| AC11 | 리셋 시각 도달 시 정확 재계산 | 단위 테스트: oneshot 타이머에 t+5s 시각 주입 → fire 시점 검증 (±100ms) |
| AC12 | 재부팅 후 자동 시작 | 수동: 자동 시작 on → 재부팅 → 메뉴바 표시 확인 |

---

## 4. Implementation Steps

### Step 1. CLI 로그 스파이크 (S)
- 산출물: `/Users/ujeonghyeon/Desktop/dev/myDev/token-notifier/.omc/notes/log-format-spike.md`
- 작업:
  - Claude Code: `~/.claude/projects/*/`, `~/.claude/history/`, `~/.claude/usage.json` 등 실제 파일 inventory.
  - Codex CLI: 설치 경로 및 `~/.codex/`, `~/.cache/codex/` 후보 탐색.
  - 각 파일의 한 줄/한 엔트리 샘플 + 윈도우 메타데이터(시작 시각/한도) 명시 여부 결론.
  - `tauri-plugin-autostart` 소스 확인하여 macOS에서 LaunchAgent plist 경로를 쓰는지 `SMAppService.mainApp.register()` 경로를 쓰는지 결론.
- 통과 기준 (명시적 산출물):
  - **CC 로그 포맷:** JSONL / plaintext / SQLite / binary 중 무엇인지 한 줄로 결론.
  - **CX 로그 포맷:** 동일하게 한 줄 결론.
  - **윈도우 메타데이터 명시 여부:** CC/CX 각각 yes/no + 발견 위치.
  - **한도(quota) 수치 출처:** 로그 내 명시 / 코드 상수 추정 / 미상 중 택1.
  - **autostart 메커니즘:** plist / SMAppService / 둘 다 지원 중 택1.
- **CX 스파이크 실패 분기 (의사결정 트리):**
  ```
  CX 로그 발견?
   ├── No 또는 포맷이 epoch마다 가변 → CX 파서를 stub(`Codex(NotImplemented)`)으로 두고
   │    AC2를 "conditional fail (Codex 지원 보류)"로 마킹.
   │    Settings UI는 CX 토글을 disabled + "Codex 로그 포맷 확정 후 활성화"  툴팁.
   │    트레이는 `CC {n}%`만 표시.
   └── Yes 안정 포맷 → 정식 파서 구현 진행.
  ```
- `LocalLogParser` trait 시그니처는 라인 단위가 아닌 **`fn read_delta(&mut self) -> Vec<UsageEvent>`** 유지. trait doc-comment에 다음을 명시:
  > 구현체는 내부에서 라인 단위 JSONL이든 plaintext 청크든 자유롭게 디코드한다.
  > 한 청크 = 한 `UsageEvent`로 매핑되도록 구현체가 흡수 책임을 진다.
  > offset/inode는 구현체 내부 상태로 보관.
- 의존: 없음.

### Step 2. Tauri 프로젝트 부트스트랩 (S)
- 산출물:
  - `/Users/ujeonghyeon/Desktop/dev/myDev/token-notifier/src-tauri/Cargo.toml`
  - `/Users/ujeonghyeon/Desktop/dev/myDev/token-notifier/src-tauri/tauri.conf.json`
  - `/Users/ujeonghyeon/Desktop/dev/myDev/token-notifier/src/index.html` (popover root)
- 사용 crate/plugin:
  - `tauri` 2.x
  - `tauri-plugin-autostart`
  - `tauri-plugin-notification`
  - `tokio` (runtime)
  - `chrono` (시간)
  - `serde`, `serde_json`
  - `rusqlite` (bundled feature)
  - `dirs` (홈 디렉토리)
  - `anyhow`, `thiserror`
- 검증: `cargo build --manifest-path src-tauri/Cargo.toml` 성공, 빈 트레이 아이콘 macOS에서 표시.
- 의존: Step 1 (로그 경로 알아야 권한 신청 항목 결정).

### Step 3. Rust 백엔드 - 파서/추정기/저장소 (M)
- 산출물:
  - `src-tauri/src/parser/claude_code.rs`
  - `src-tauri/src/parser/codex.rs`
  - `src-tauri/src/parser/mod.rs` (trait `LocalLogParser { fn read_delta(&mut self) -> Vec<UsageEvent> }`)
  - `src-tauri/src/window_estimator.rs` (B3 hybrid: 메타데이터 우선 + fallback 길이)
  - `src-tauri/src/storage.rs` (`rusqlite`: tables `hourly_bucket`, `daily_rollup`, `threshold_state`)
  - `src-tauri/src/config.rs` (hidden config `~/Library/Application Support/token-notifier/config.toml`)
- 단위 테스트:
  - `parser::tests::reads_incremental_with_offset`
  - `window_estimator::tests::prefers_metadata_over_fallback`
  - `storage::tests::aggregates_hourly_to_daily`
- 의존: Step 2.

### Step 4. 폴링 + 리셋 타이머 (M)
- 산출물: `src-tauri/src/scheduler.rs`
- 동작:
  - 60s `tokio::time::interval` → 파서 → 추정기 → DB 기록 → 트레이 갱신 이벤트 emit.
  - 윈도우 reset 시각이 변경될 때마다 기존 oneshot 취소 후 `tokio::time::sleep_until(reset_at)` 재예약.
  - 리셋 fire 시 즉시 트레이 강제 갱신 + 알림 상태 초기화.
- **Race condition 방지 (window_generation_id):**
  - `Scheduler { window_generation_id: AtomicU64, ... }` 도입.
  - 60s tick과 reset oneshot은 fire 시점에 자신이 캡쳐한 generation을 현재값과 비교 → 다르면 결과 drop.
  - reset oneshot fire 직후 `generation_id.fetch_add(1)` → 진행 중이던 in-flight tick은 stale로 자동 폐기 + 60s 타이머도 재시작.
  - 코드 스케치:
    ```rust
    let gen_at_dispatch = self.window_generation_id.load(Ordering::Acquire);
    let snapshot = self.collect_usage().await;
    if self.window_generation_id.load(Ordering::Acquire) != gen_at_dispatch {
        tracing::debug!("stale tick dropped (gen {} vs current)", gen_at_dispatch);
        return;
    }
    self.emit_tray_update(snapshot).await;
    ```
  - reset 처리:
    ```rust
    // oneshot fire
    self.window_generation_id.fetch_add(1, Ordering::AcqRel);
    self.cancel_and_restart_60s_interval();
    self.clear_threshold_state_for_window();
    self.force_tray_refresh().await;
    ```
- 단위 테스트:
  - scheduler가 가짜 시계(`tokio::time::pause`)에서 60s tick과 reset oneshot을 독립적으로 트리거하는지.
  - reset 직전에 tick이 인플라이트인 경우 stale로 폐기되는지.
- 의존: Step 3.

### Step 5. 메뉴바 트레이 통합 (S)
- 산출물: `src-tauri/src/tray.rs`
- 동작:
  - `TrayIconBuilder` 라벨에 `format!("CC {cc}% CX {cx}%")` (소스 비활성 시 빈 문자열로 생략).
  - 색상 임계치 적용 (텍스트 색 attribute 사용; macOS NSStatusItem 텍스트 색 한계가 있으면 emoji 대용으로 fallback 결정 - Step 1 스파이크 후 보완).
  - 리셋 카운트다운 (작은 글자)은 다음 줄 또는 옆 텍스트로 표시 (NSStatusItem 멀티라인 제약 검증 후 확정).
- 단위 테스트: `format_tray_label(state)` 순수 함수 테스트.
- **IPC 방향성:**

  | Direction | Mechanism | Examples |
  |-----------|-----------|----------|
  | Backend → Frontend | `app.emit("usage-update", { cc, cx, reset_at })` | scheduler가 트레이 모듈에 갱신 신호 |
  | Frontend → Backend | (해당 없음 - 트레이는 Rust 단독) | - |
- 의존: Step 4.

### Step 6. 알림 시스템 (S)
- 산출물: `src-tauri/src/alerts.rs`
- 동작:
  - `ThresholdEvaluator::evaluate(source, percent, window_id) -> Option<Notification>`.
  - DB `threshold_state` 테이블에 `(source, window_id, threshold)` 튜플 unique 제약으로 중복 방지.
  - 발사는 `tauri-plugin-notification`로 위임.
- **Oscillation 흡수 의사코드:**
  ```
  fn evaluate(source, percent, window_id, thresholds):
      for t in thresholds:
          key = (source, t, window_id)
          if percent >= t and key not in lastNotifiedWindowId:
              insert key into lastNotifiedWindowId  // DB unique 제약
              return Notification { source, threshold: t }
      return None

  // 같은 윈도우 안에서 percent가 t 위↔아래를 오가도 key가 존재하므로 재발사 없음.
  // window_generation_id +1 (Step 4 reset) 시 새 window_id가 부여되므로
  // lastNotifiedWindowId의 이전 윈도우 키는 자연히 stale → 다음 윈도우에서 정상 재발사.
  ```
- 단위 테스트:
  - 동일 (source, window, threshold) 2회 → 1회만 emit.
  - 76% → 65% → 76% 같은 윈도우 → 1회만 emit (oscillation 흡수).
  - 윈도우 generation +1 후 76% → 다시 1회 emit.
- 의존: Step 4.

### Step 7. WebView 팝오버 - 차트 + 누적 (M)
- **WebView lazy lifecycle (R8 mitigation):** 앱 시작 시 팝오버 윈도우를 생성하지 **않는다**. 메뉴바 클릭 이벤트에서 lazy create, 팝오버 close/blur 이벤트에서 destroy. idle RAM 30~50MB 절약.
  - Trade-off: 최초 클릭 시 1-2초 hang. 사용자 인지 가능. 필요 시 hidden preload(`createOnStartup` opt-in) 옵션을 risk mitigation으로 보존.
- 산출물:
  - `src/popover.html`, `src/popover.css`, `src/popover.js`
  - `src/vendor/chart.umd.js` (Chart.js 로컬 번들, 네트워크 호출 없음)
  - Tauri commands: `get_24h_series`, `get_rollups`
- 동작:
  - 24h 시간버킷 24개 라인 차트 (CC/CX 2 dataset).
  - 하단 카드 3개: 일/주/월 누적 (DB `daily_rollup` 집계).
- **IPC 방향성:**

  | Direction | Mechanism | Examples |
  |-----------|-----------|----------|
  | Backend → Frontend | `app.emit("usage-update", payload)` | 트레이 갱신 트리거, 팝오버 라이브 갱신 |
  | Frontend → Backend | `invoke("get_24h_series")`, `invoke("get_rollups", { range })` | 팝오버 오픈 시 차트/누적 데이터 조회 |
- 의존: Step 3 (DB 스키마), Step 5 (트레이 클릭 → 윈도우 show).

### Step 8. Settings UI + 영속화 (S)
- 산출물:
  - `src/settings.html`, `src/settings.js`
  - Tauri commands: `get_settings`, `save_settings`
  - `~/Library/Application Support/token-notifier/settings.json`
- 항목 4가지: CC on/off, CX on/off, 소스별 임계치 입력(1~3개, 1~99 정수), 자동 시작 on/off.
- 즉시 반영: 저장 시 scheduler에 reload 이벤트 emit.
- **IPC 방향성:**

  | Direction | Mechanism | Examples |
  |-----------|-----------|----------|
  | Frontend → Backend | `invoke("get_settings")`, `invoke("save_settings", { settings })` | 설정 조회/저장 |
  | Backend → Frontend | `app.emit("settings-reloaded", payload)` | 다른 윈도우에 변경 알림 |
- 의존: Step 4.

### Step 9. 자동 시작 (S)
- 산출물: `tauri-plugin-autostart` 통합 in `src-tauri/src/lib.rs`.
- **메커니즘 선택 (Step 1 스파이크 결과 의존):**
  - 본 프로젝트는 macOS 13+ 전제(스펙 명시 부재이나 Tauri 2.x 권장 최소). macOS 13+에서는 LaunchAgent plist 대신 **`SMAppService.mainApp.register()`** 권장 (Apple 공식 권장 경로, App Sandbox 미래 호환).
  - `tauri-plugin-autostart`가 어느 경로를 쓰는지를 Step 1에서 확인:
    - plist 경로만 지원 → 그대로 사용. AC12 검증 시 `~/Library/LaunchAgents/<bundle>.plist` 확인.
    - SMAppService 지원 → 선호. AC12 검증 시 `launchctl print system | grep <bundle>` 확인.
  - macOS < 13 fallback은 본 프로젝트 범위에서 제외 (macOS 13+ 전제).
- 검증:
  - Settings 토글 on → 위 메커니즘별 등록 흔적 확인.
  - 재부팅 시나리오 수동 검증.
- 의존: Step 8.

### Step 10. 패키징 (S)
- 산출물: `cargo tauri build` 결과 `.app` 번들.
- 서명: 개인 용도 ad-hoc 서명 (`codesign --force --deep --sign -`).
- 첫 실행 시 알림 권한 다이얼로그 + 홈 디렉토리 접근 권한 다이얼로그(필요 시) 흐름 수동 확인.
- 의존: Step 5-9 모두.

---

## 5. Risks and Mitigations

| # | Risk | 영향도 | 가능성 | Mitigation |
|---|------|-------|--------|-----------|
| R1 | Claude Code / Codex CLI 로그 포맷 변경 | H | M | 파서를 trait + 버전 가드로 분리. 포맷 미스매치 시 마지막 알려진 값 유지 + 트레이에 `CC -%` 표시. Step 1 스파이크 결과를 `LOG_FORMAT.md`로 박제. |
| R2 | 윈도우 길이/한도 추정 부정확 (B2 fallback 시) | H | M | B3 hybrid 채택. hidden config로 length override 노출. 추정값 사용 시 트레이에 작은 `~` 표식 추가 검토. |
| R3 | macOS 알림 권한 다이얼로그 누락 | M | M | 첫 실행 시 `request_permission()` 명시 호출 + 거부 시 Settings에 가이드 텍스트 표시. AC8 수동 검증 항목에 포함. |
| R4 | 폴링 60s tick과 reset oneshot의 시계 어긋남 | H | M | 두 타이머는 독립 task로 분리. reset oneshot fire 시 60s tick 보다 우선 트레이 강제 갱신. `tokio::time::pause` 기반 단위 테스트로 동시 동작 검증. |
| R5 | WebView ↔ Rust IPC 오류 (commands 미등록/serde 실패) | M | M | 모든 Tauri command에 `Result<T, String>` 반환 + frontend에서 toast로 에러 노출. e2e 시나리오로 popover 첫 오픈 시 차트/누적 둘 다 로드되는지 확인. |
| R6 | NSStatusItem 텍스트 멀티라인/색상 제약으로 AC3/AC4 미충족 | M | M | Step 1 스파이크에서 트레이 라벨 가능 길이/색상 attribute 가능 여부 검증. 불가 시 색상은 NSAttributedString, 멀티라인은 한 줄 형식 `CC73%↻4h12m CX91%↻2h05m`으로 fallback 결정. |
| R7 | DB 파일 손상 (예기치 못한 종료) | L | L | `rusqlite`에 WAL 모드 + 시작 시 `PRAGMA integrity_check`. 실패 시 daily_rollup만 재계산하고 hourly_bucket은 비우는 복구 경로. |
| R8 | WebView 부팅 비용 (idle RAM, 첫 클릭 지연) | M | H | Step 7 lazy create + close 시 destroy. idle RAM 30~50MB 절약. 최초 클릭 시 1-2초 hang은 수용. 필요 시 hidden preload opt-in. |
| R9 | 시계 점프 (sleep/wake, NTP 보정, DST) | H | M | `NSWorkspaceDidWakeNotification` hook → 깨어나면 `window_generation_id.fetch_add(1)` + 모든 reset 타이머 재평가 + 즉시 파서 1회 강제 실행. |
| R10 | 로그 파일 rotation / truncation | M | L | 파서가 inode 변경 또는 size shrink 감지 시 offset을 0으로 리셋. `LocalLogParser` 구현체 책임. |
| R11 | 메뉴바 폭 jitter (% 자릿수 변동) | L | M | 라벨에 모노스페이스 폰트 + 고정 자릿수 포맷(`format!("CC {:>3}% CX {:>3}%", cc, cx)`)으로 jitter 억제. |

---

## 6. Verification Steps

### 단위 테스트 (`cargo test`)
- `parser::*::reads_incremental_with_offset` (AC1, AC2)
- `window_estimator::prefers_metadata_over_fallback` (AC11)
- `window_estimator::falls_back_to_fixed_length_when_no_metadata` (AC11)
- `color_for_percent` (경계값 69/70/89/90, AC4)
- `format_tray_label` (소스 한쪽 비활성 케이스 포함, AC3)
- `threshold_evaluator::dedup_within_window` (AC8)
- `scheduler::poll_and_reset_independent` (`tokio::time::pause`, AC11)
- `storage::aggregates_hourly_to_daily` (AC7)

### 통합/수동 시나리오
- **S1. AC1/AC2:** fake CC/CX 로그 fixture에 1개 엔트리 추가 → 60s 이내에 트레이 라벨 변화 (Activity Monitor로 동시에 측정).
- **S2. AC5:** 30분 실행 후 Activity Monitor 평균 CPU < 1%, 에너지 영향 "낮음".
- **S3. AC6/AC7:** 트레이 클릭 → 팝오버 오픈, 차트 24 포인트 + 일/주/월 카드 3개 렌더.
- **S4. AC8:** Settings에 임계치 75 입력 → 가짜 사용량을 76%로 주입 → 알림 1회 발사 → 76% 유지 동안 추가 알림 없음 → 다음 윈도우 리셋 후 다시 76% 도달 시 다시 1회 발사.
- **S5. AC9:** Network Link Conditioner 100% loss → 트레이 정상 갱신 (오프라인 5분 후에도 % 변화 반영).
- **S6. AC10:** 4개 항목 각각 변경 후 1초 이내 트레이/scheduler 반영.
- **S7. AC11:** 윈도우 길이 5분으로 강제 override → 5분 fire 시점 ±2초 이내 트레이 0% 재시작.
- **S8. AC12:** 자동 시작 on → 재부팅 → 로그인 직후 메뉴바에 표시.

### 빌드 검증
- `cargo build --release --manifest-path src-tauri/Cargo.toml` 성공.
- `cargo tauri build` `.app` 산출.
- `.app` ad-hoc 서명 후 첫 실행 시 권한 다이얼로그 흐름 검증.

### AC 검증 자동화 스크립트
- **AC5 (CPU < 1%):** `scripts/verify-ac5.sh`
  ```bash
  #!/usr/bin/env bash
  PID=$(pgrep -f token-notifier | head -1)
  for i in {1..30}; do
    ps -o %cpu= -p "$PID"
    sleep 1
  done | awk '{s+=$1} END {avg=s/NR; print "avg cpu:", avg; exit (avg<1.0?0:1)}'
  ```
- **AC9 (네트워크 호출 0건):**
  - `ggrep -rE "(reqwest|http://|https://|ureq|hyper::Client|isahc|surf)" src-tauri/src` 결과가 비어있어야 함.
  - `tauri.conf.json`의 `security.csp`에 `connect-src 'none'` 명시 + 검증.
- **AC11 (리셋 ±100ms):**
  - 단위 테스트: `scheduler.rs`에서 `tokio::time::pause` + mocked clock으로 reset oneshot fire 시점 검증 (±100ms tolerance).
  - 시나리오 테스트: `NSWorkspaceDidWakeNotification` 모킹 → wake 시 generation +1 및 reset 재평가 동작 확인.
- **AC12 (자동 시작):**
  - 자동 시작 등록 후 `ls ~/Library/LaunchAgents/ | grep token-notifier` (plist 경로 사용 시) 또는 `launchctl print system | grep com.tokennotifier.app` (SMAppService 사용 시).

---

## ADR

- **Decision:** **A1 (60s pull) + B3 (메타 우선, 코드 상수 fallback) + C1 (Chart.js 로컬 번들)** 조합으로 Tauri 2.x + Rust 백엔드 + WebView 프런트엔드를 구현. scheduler는 `window_generation_id: AtomicU64` 기반 race-safe 모델, WebView는 lazy create/destroy, autostart는 Step 1 결과에 따라 SMAppService 또는 plist 경로 택1.
- **Drivers:**
  - 스펙 Principles: 외부 네트워크 0건, 1분 갱신 + 정확한 reset fire 분리, UI 설정 4항목 고정.
  - 사용자 명시 요건: 배터리 친화 (Activity Monitor 1% 미만), 메뉴바 한 줄 정보 압축, macOS 단일 머신 데스크톱.
  - AC1/AC2/AC11 정확성, AC5 배터리, AC9 네트워크 0건이 차별 결정 요인.
- **Alternatives considered:**
  - Swift 네이티브: Round 10에서 Tauri로 확정 → 배제.
  - 공식 Anthropic Usage API: Constraints에서 명시적으로 배제.
  - **A2 (FSEvents push):** notify-rs macOS backend의 atomic rename/truncate 이벤트 누락 보고, 디바운스 로직 +150~200 LoC, race condition 표면적 ↑, AC1 "1분 이내" 요구에 sub-second 반응은 over-engineering → 정량적 거부.
  - **B2 (고정 길이 단독):** 윈도우 길이 ±10분 오차 시 % ±3%p 오차 → AC11 ±100ms 양립 불가 → 거부.
  - **C2 (수기 SVG/Canvas):** Chart.js 1 dependency vs 라인 차트 1개에 수백 LoC → ROI 낮음 → 거부.
  - **B3 hidden config 파일 방식:** 사용자 발견성 0이라 가치 없음 → 코드 상수 + dev-only 환경변수로 대체.
- **Why chosen:**
  - A1: AC1/AC2 "1분 이내" + AC5 "1% 미만"을 60s tick + offset incremental read로 모두 충족.
  - B3 (코드 상수 fallback): 스파이크 결과 메타 명시 여부 불확실 - 두 케이스 모두 견디는 설계가 risk-adjusted 최적. Principle 5 (UI 4항목)와도 호환.
  - C1: 라이선스 자유 + 단일 파일 + 외부 네트워크 없음 (AC9 호환).
  - lazy WebView: idle RAM 30~50MB 절약, AC5 마진 확보.
- **Consequences:**
  - (+) AC5 마진 큼: 폴링 60s + lazy WebView + idle IO 최소화.
  - (+) 로그 포맷 변경에 trait 가드로 격리 (R1).
  - (+) race-safe 윈도우 reset (window_generation_id).
  - (-) 최초 팝오버 클릭 시 1-2초 hang (R8 trade-off).
  - (-) 메타데이터 없을 때 % 오차 ±3%p 잔존 (R2). 트레이에 `~` 표식으로 통지.
  - (-) Chart.js 의존성 +60KB 정도.
- **Follow-ups:**
  - Step 1 스파이크 완료 후:
    - CC/CX 로그 포맷(JSONL/plaintext/SQLite/binary) 본 문서에 박제.
    - B1 채택 가능 여부 확정 → 가능 시 B3 fallback은 보험 dead path로 유지.
    - **CX 로그 미확정 시 Codex 지원 보류** 결정 (Step 1 의사결정 트리 참조).
    - autostart 메커니즘 (SMAppService vs plist) 확정.
  - R6 (NSStatusItem 제약) 확정 결과를 Step 5 라벨 포맷에 반영.
  - 향후 다른 CLI(예: Cursor) 추가 시 `LocalLogParser` trait 한 곳만 확장.

---

## Changelog

- **Iteration 1 → 2 (Critic + Architect feedback 반영):**
  - A. Principle 5에 "사용자 노출 UI 설정 4개" 단서 명시 + B3을 코드 상수 fallback으로 재정의 (hidden config 폐기).
  - B. A2 거부 사유를 notify-rs 이슈/LoC/race 표면적의 4가지 정량 trade-off로 재작성, B2 거부를 ±10분 → ±3%p 정량 근거로 보강.
  - C. Step 1에 통과 기준(로그 포맷 결론, autostart 메커니즘 결론, CX 실패 분기 의사결정 트리) 추가. `LocalLogParser` trait doc-comment 명시.
  - D. Step 4에 `window_generation_id: AtomicU64` 도입 + race-safe tick drop / reset 처리 코드 스케치 추가.
  - E. Step 7에 WebView lazy create/destroy 정책 명시 + trade-off 기재.
  - F. Step 9에 SMAppService vs plist 분기 + tauri-plugin-autostart 경로 확인 명시.
  - G. Step 6에 oscillation 흡수 의사코드 + window_generation_id 연동 + 76→65→76 시나리오 단위 테스트 추가.
  - H. Step 5/7/8에 IPC 방향성 표 추가.
  - I. R8 (WebView 부팅), R9 (시계 점프), R10 (rotation), R11 (jitter) 위험 4개 추가 및 코드 레벨 mitigation 기재.
  - J. AC5/AC9/AC11/AC12 검증 자동화 스크립트 섹션 추가 (`scripts/verify-ac5.sh`, ggrep CSP 검증, mocked clock + NSWorkspaceDidWakeNotification 모킹, launchctl 검증).
  - K. ADR을 Decision/Drivers/Alternatives considered/Why chosen/Consequences/Follow-ups 6필드로 재구조화 + 정량 근거 포함.
  - L. 본 Changelog 섹션 신설.
