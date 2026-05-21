# Deep Interview Spec: Token Notifier - macOS 메뉴바 토큰 사용량 앱

## Metadata
- Interview ID: token-notifier-2026-05-21
- Rounds: 12 (Round 0 topology + 12 ambiguity 라운드)
- Final Ambiguity Score: 16.4%
- Type: greenfield
- Generated: 2026-05-21
- Threshold: 0.20 (통과: 0.164)
- Initial Context Summarized: no
- Status: PASSED

## Clarity Breakdown
| Dimension | Score | Weight | Weighted |
|-----------|-------|--------|----------|
| Goal Clarity | 0.875 | 0.40 | 0.350 |
| Constraint Clarity | 0.813 | 0.30 | 0.244 |
| Success Criteria | 0.805 | 0.30 | 0.242 |
| **Total Clarity** | | | **0.836** |
| **Ambiguity** | | | **0.164 (16.4%)** |

## Topology

Round 0에서 5개 컴포넌트 모두 active로 잠금 (deferred 없음).

| Component | Status | Description | Coverage / Deferral Note |
|-----------|--------|-------------|--------------------------|
| Token Data Source | active | Claude Code CLI + Codex CLI 로컬 로그/세션 파일 파싱하여 윈도우 사용 % 산출 | Goal/Constraints/Criteria 모두 0.80↑. AC1, AC2, AC9 커버 |
| Menu Bar UI | active | 메뉴바에 CC %와 CX %를 큰 글자, 리셋 시간을 작은 글자로 표시 | Goal 0.88, Constraints 0.85, Criteria 0.85. AC3, AC4, AC5 커버 |
| Detail Popover | active | 메뉴바 클릭 시 24h 라인 그래프 + 일/주/월 누적 숫자 | Goal 0.90, Constraints 0.85, Criteria 0.80. AC6, AC7 커버 |
| Alerts | active | 사용자 정의 임계치 도달 시 macOS 시스템 알림 | Goal 0.80, Constraints 0.70, Criteria 0.80. AC8 커버 |
| Settings | active | 소스 on/off, 임계치 자유 입력, 로그인 시 자동 시작 | Goal 0.90, Constraints 0.75, Criteria 0.75. AC10 커버 |

## Goal

macOS 메뉴바 상단에 Claude Code CLI와 Codex CLI 각각의 **rate-limit 윈도우 남은 % (큰 글자)** 와 **리셋까지 남은 시간 (작은 글자)** 을 항상 보여주는 가벼운 데스크톱 앱을 만든다. 사용자가 터미널에서 `claude /usage` 같은 명령을 치지 않고도 메뉴바만 흘끗 보고 현재 사용 수준을 파악할 수 있으며, 리셋 윈도우를 놓쳐 코딩 흐름이 끊기지 않도록 임계치 알림과 시간대별 사용 패턴 시각화를 함께 제공한다.

## Constraints

- 데이터 수집은 **로컬 로그/세션 파일 파싱** 만 사용한다. 외부 네트워크 호출(공식 API, 비공식 엔드포인트) 없음.
- 메뉴바 표시 갱신 주기는 **1분 이상**. 배터리/디스크 IO 부담을 낮추는 것이 실시간성보다 우선.
- 단, "리셋 임박 알림"은 폴링과 별도의 정확한 타이머로 처리(폴링 1분 주기로는 리셋 직전 알림이 부정확하기 때문).
- 기술 스택: **Tauri (Rust 백엔드 + WebView 프런트엔드)**. 최종 바이너리 가벼움, Rust의 파일시스템/타이머 처리 강점, WebView로 차트 자유도 확보.
- 대상 플랫폼: macOS만. 다른 OS 지원 명시적 비목표.
- 메뉴바 표시 모드: CC와 CX 두 소스를 **동시에** 한 메뉴바에 표시(토글 아님). 단, 설정에서 소스별 on/off 가능.
- 메뉴바 색상 임계치는 **개발자 정의 기본값(예: 70% 노랑, 90% 빨강)** 으로 고정. 사용자 설정 항목 아님.
- Settings 항목 3개로 한정: 소스 on/off, 소스별 알림 임계치 자유 입력, 로그인 시 자동 시작.
- Detail Popover 콘텐츠 범위: 24시간 라인 그래프 + 일/주/월 누적 숫자. **모델별 분해와 최근 세션 목록은 비포함**.

## Non-Goals

- 토큰 단위 절대값(input/output 토큰 수) 표시 (% 단위로 통일).
- Anthropic Usage API / Codex 공식 사용량 API 통합.
- Opus/Sonnet/Haiku 등 모델별 사용량 분해.
- 최근 세션 리스트 / 세션별 드릴다운.
- USD 비용 계산.
- 두 CLI 사용량 비교 추천 ("지금은 어느 CLI를 쓰는 게 유리하다" 같은 추천).
- Windows/Linux 지원.
- 다중 사용자/팀 공유 기능, 클라우드 동기화.
- 메뉴바 색상 구간 사용자 커스터마이징.

## Acceptance Criteria

- [ ] **AC1.** Claude Code CLI가 세션을 진행하면, 그 사용량이 1분 이내에 메뉴바 % 숫자에 반영된다.
- [ ] **AC2.** Codex CLI가 세션을 진행하면, 그 사용량이 1분 이내에 메뉴바 % 숫자에 반영된다.
- [ ] **AC3.** 메뉴바에는 `CC {큰%}  CX {큰%}` 형태로 두 소스가 큰 글자로, 그 아래(또는 옆에) 각 리셋까지 남은 시간이 작은 글자로 표시된다.
- [ ] **AC4.** 메뉴바 % 숫자 색상은 0~70% 기본색, 70~90% 노랑, 90% 이상 빨강으로 자동 변경된다.
- [ ] **AC5.** 메뉴바 갱신 주기는 60초 이상이고, 백그라운드에서 CPU/배터리 사용이 무시할 만한 수준이다 (Activity Monitor 기준 1% 미만).
- [ ] **AC6.** 메뉴바 아이콘 클릭 시 팝오버가 열리고, 지난 24시간 사용량을 라인 그래프로 보여준다.
- [ ] **AC7.** 팝오버 하단에 오늘(일) / 이번 주(주) / 이번 달(월) 누적 사용량 숫자가 표시된다.
- [ ] **AC8.** 사용자가 설정한 임계치(예: CC 75%, CC 90%, CX 80%)에 도달하면 macOS 시스템 알림센터에 알림이 표시된다. 같은 임계치에 대해 같은 윈도우 내에서는 중복 알림이 발생하지 않는다.
- [ ] **AC9.** 외부 네트워크 호출 없이 동작한다(네트워크 차단 환경에서도 % 표시가 정상 작동).
- [ ] **AC10.** 설정 창에서 (a) CC on/off, (b) CX on/off, (c) 소스별 알림 임계치 자유 입력(최소 1개, 최대 3개), (d) 로그인 시 자동 시작 on/off 4가지를 변경할 수 있고, 변경이 즉시 반영된다.
- [ ] **AC11.** 윈도우 리셋 시각 도달 시 메뉴바 % 표시와 리셋 카운트다운이 정확히 재계산되며, 새 윈도우가 시작된다(폴링 주기와 무관하게 타이머 정확).
- [ ] **AC12.** 앱이 macOS 로그인 시 자동 시작되도록 설정하면, 재부팅 후에도 사용자 액션 없이 메뉴바에 나타난다.

## Assumptions Exposed & Resolved

| Assumption | Challenge | Resolution |
|------------|-----------|------------|
| "토큰 사용량" = 절대 토큰 수 | Round 2: 어떤 메트릭이 가장 만족스러운가? | **% 단위(rate-limit 윈도우 남은 비율)** 로 통일 |
| 한 서비스(Claude API)의 토큰만 추적 | Round 1: 어떤 서비스의 토큰? | **Claude Code CLI + Codex CLI 동시**, 설정으로 각각 on/off |
| 공식 API 호출로 데이터 가져옴 | Round 4 (Contrarian): 공식 API 없으면? | **로컬 로그/세션 파일 파싱** 명시적 허용, 네트워크 호출 없음 |
| 알림은 필수 | Round 6 (Simplifier): 메뉴바 색상만으로 충분한가? | **알림 유지** - 메뉴바 표시와 별개로 시스템 알림 필요 |
| 알림 임계치는 고정값 | Round 7: 75/90 고정 vs 자유 입력? | **자유 입력**, 소스별로 1~3개까지 |
| 모델별 분해/세션 목록도 필요 | Round 5: 팝오버에 무엇이? | **불필요**, 시간대별 그래프 + 일/주/월 누적만 |
| 폴링은 빨라야 함 | Round 9: 갱신 주기? | **1분 이상** (배터리 우선). 리셋 알림은 별도 타이머로 정확성 보장 |
| Swift 네이티브가 표준 | Round 10: 기술 스택? | **Tauri (Rust + WebView)** - 경량 바이너리, Rust로 파일/타이머 처리 |
| 메뉴바 색상도 사용자가 조절 | Round 11: 설정 항목? | **개발자 기본값 고정** (70% 노랑, 90% 빨강) |

## Technical Context (Greenfield)

- **언어/프레임워크:** Tauri 2.x (Rust 백엔드 + HTML/CSS/JS 프런트엔드).
- **메뉴바 통합:** Tauri의 `tray` API로 macOS NSStatusItem 사용. 텍스트 라벨에 `CC {n}%  CX {n}%` 동적 갱신.
- **로그 파싱 (Rust 백엔드):**
  - Claude Code: `~/.claude/` 하위 세션/로그 파일 (`projects/*/`, `history/`, `usage.json` 등 - 실제 경로/포맷은 1단계 스파이크에서 검증).
  - Codex CLI: 설치 디렉토리 및 사용자 캐시 디렉토리 (1단계 스파이크에서 위치/포맷 확인).
- **윈도우 모델:** 각 CLI별로 "윈도우 시작 시각 + 윈도우 길이"를 추정/감지하고, 그 안에서의 사용량을 합산해 한도 대비 %로 변환.
- **폴링 + 타이머:**
  - 메뉴바 갱신: 60초 주기 Rust 타이머가 로그 파일 다시 읽기 트리거.
  - 리셋 알림: 현재 윈도우의 리셋 시각이 정해지면 그 시각에 정확히 fire하는 별도 oneshot 타이머.
- **알림:** Tauri Notification API 또는 `mac-notification-sys` crate로 macOS 알림센터 호출. 알림 발사 후 (소스, 윈도우 ID, 임계치) 튜플로 중복 방지.
- **저장:** 일/주/월 누적은 SQLite (Rust `rusqlite`) 또는 단순 append-only JSONL. 저장소 위치 `~/Library/Application Support/token-notifier/`.
- **로그인 시 자동 시작:** Tauri의 `autostart` 플러그인 (LaunchAgent plist 자동 생성).
- **권한:** 사용자 홈 디렉토리 읽기, macOS 알림 권한. 첫 실행 시 권한 안내 다이얼로그 필요.

## Ontology (Key Entities)

| Entity | Type | Fields | Relationships |
|--------|------|--------|---------------|
| ClaudeCodeUsage | core domain | windowStart, windowDurationSec, tokensUsed, quotaLimit, percentUsed | belongs to RateLimitWindow |
| CodexUsage | core domain | windowStart, windowDurationSec, tokensUsed, quotaLimit, percentUsed | belongs to RateLimitWindow |
| MenuBarDisplay | core domain | ccText, cxText, ccColor, cxColor, ccResetText, cxResetText | renders from ClaudeCodeUsage, CodexUsage |
| RemainingPercent | core domain | sourceId, percent, color | computed from {Source}Usage |
| ResetWindow | core domain | sourceId, startedAt, resetAt, durationSec | parent of RateLimitWindow timers |
| DisplayToggle | supporting | sourceId, enabled | per-source setting |
| LocalLogParser | supporting | sourceId, logPath, fileFormat, lastReadOffset | reads {Source}Usage |
| HourlyBucket | supporting | sourceId, hourStart, tokensUsed | aggregates into DailyRollup |
| DailyRollup | supporting | sourceId, date, tokensUsed | aggregates into Weekly/Monthly views |
| ThresholdConfig | supporting | sourceId, thresholdPercent[], notified[] | drives Alerts |
| ThresholdRule | supporting | sourceId, threshold, lastNotifiedWindowId | enforces dedup |

## Ontology Convergence

| Round | Entity Count | New | Changed | Stable | Stability Ratio |
|-------|-------------|-----|---------|--------|----------------|
| 1 | 4 | 4 | - | - | N/A |
| 2 | 6 | 2 (RemainingTokens, ResetWindow) | 0 | 4 | 67% |
| 3 | 6 | 0 | 1 (RemainingTokens → RemainingPercent) | 4 | 83% |
| 4 | 7 | 1 (LocalLogParser) | 0 | 6 | 86% |
| 5 | 9 | 2 (HourlyBucket, DailyRollup) | 0 | 7 | 78% |
| 6 | 10 | 1 (ThresholdRule) | 0 | 9 | 90% |
| 7 | 11 | 1 (ThresholdConfig) | 0 | 10 | 91% |
| 8 | 11 | 0 | 0 | 11 | 100% |
| 9 | 11 | 0 | 0 | 11 | 100% |
| 10 | 11 | 0 | 0 | 11 | 100% |
| 11 | 11 | 0 | 0 | 11 | 100% |
| 12 | 11 | 0 | 0 | 11 | 100% |

5라운드 연속 신규/변경 엔티티 0개 - 도메인 모델 완전 수렴.

## Open Questions (남은 미세 갭, 구현 1단계 스파이크에서 해소)

1. Claude Code CLI의 로컬 사용량 데이터가 실제로 어디에 저장되는지 (`~/.claude/projects/*/` JSONL? `usage.json`? 세션 디렉토리?) - 1단계 스파이크 필수.
2. Codex CLI의 로컬 로그/세션 위치와 포맷 (Codex 버전별로 다를 수 있음) - 1단계 스파이크 필수.
3. 각 CLI의 "윈도우 길이"가 실제로 5시간 고정인지, 구독 플랜별로 다른지.
4. 메뉴바 표시 텍스트 한계 길이 (긴 텍스트가 시스템에 의해 잘릴 가능성) 검증.
5. macOS 권한: 첫 실행 시 알림 권한 요청 흐름, 그리고 홈 디렉토리 접근에 권한 다이얼로그가 필요한지 (sandbox 비활성 가정).

## Interview Transcript

<details>
<summary>Full Q&A (12 rounds + Round 0 topology)</summary>

### Round 0 (Topology)
**Q:** 5개 최상위 컴포넌트 구성이 맞나요?
**A:** 5개 그대로 OK
**Lock:** Token Data Source / Menu Bar UI / Detail Popover / Alerts / Settings 모두 active.

### Round 1 (Goal Clarity / Token Data Source)
**Q:** 메뉴바에 띄울 '토큰 사용량'은 구체적으로 어떤 서비스의 토큰인가요?
**A:** Claude Code CLI 사용량 + Codex CLI 사용량, 설정으로 각각 따로 보여줄 수 있으면 좋겠다.
**Ambiguity:** 70.5%

### Round 2 (Goal Clarity / Menu Bar UI)
**Q:** 메뉴바 아이콘 옆에 항상 보이는 '한 줄짜리 숫자'가 무엇이면 가장 만족스러울까요?
**A:** 리셋까지 남은 시간은 작게, 남은 토큰량은 크게 보여주면 좋겠다.
**Ambiguity:** 61.5%

### Round 3 (Goal Clarity / Menu Bar UI)
**Q:** 메뉴바 레이아웃과 '남은 토큰량' 단위는?
**A:** 두 소스 동시 표시 + % 단위 (`CC 73%  CX 91%`, 그 아래 작게 리셋 시간).
**Ambiguity:** 49.8%

### Round 4 (Constraint / Token Data Source) - 🔥 Contrarian
**Q:** 공식 API 노출 없으면 어디까지 괴로워해도 OK?
**A:** 로컬 로그/세션 파일 파싱 OK.
**Ambiguity:** 42.3% → (보정) ~60% (이후 일관 가중평균 적용)

### Round 5 (Goal Clarity / Detail Popover)
**Q:** 상세 팝오버에 무엇이 있으면 만족스러울까?
**A:** 시간대별 사용량 그래프 + 일·주·월 누적 사용량. (모델별 분해, 세션 목록은 비포함)
**Ambiguity:** 57.5%

### Round 6 (Goal Clarity / Alerts) - 🪒 Simplifier
**Q:** 메뉴바에 % 있으면 알림은 불필요하지 않나?
**A:** 시스템 알림 필요 (임계치 초과 시).
**Ambiguity:** 55.9%

### Round 7 (Goal Clarity / Alerts)
**Q:** 알림 트리거 규칙은?
**A:** 임계치 설정에서 자유 입력 (소스별).
**Ambiguity:** 52.1%

### Round 8 (Success Criteria / 전체)
**Q:** 하루 돌아간 후 '이게 바로 내가 원한 거야'라고 느끼게 해주는 장면은?
**A:** 메뉴바만 흘끗 보고 터미널로 안 갈 수 있음 + 윈도우 리셋을 놓치지 않아 코딩 흐름이 끊기지 않음.
**Ambiguity:** 38.6%

### Round 9 (Constraints / Token Data Source + Alerts)
**Q:** 메뉴바 숫자 갱신 주기는?
**A:** 1분 이상 간격.
**Ambiguity:** 30.9%

### Round 10 (Constraints / 전체)
**Q:** 기술 스택은?
**A:** Tauri (Rust + WebView).
**Ambiguity:** 26.8%

### Round 11 (Goal Clarity / Settings)
**Q:** 설정 창에 어떤 항목이 들어가야 할까요?
**A:** 소스별 on/off (CC, CX), 소스별 알림 임계치 자유 입력, 로그인 시 자동 시작 여부.
**Ambiguity:** 21.7%

### Round 12 (Goal+Constraint / Detail Popover)
**Q:** 팝오버의 시간대별 그래프와 누적 통계는?
**A:** 24h 라인 그래프 + 일/주/월 누적 숫자.
**Ambiguity:** 16.4% 🎯 임계치 통과

</details>
