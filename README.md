# token-notifier

macOS 메뉴바에 Claude Code 와 Codex 의 rate-limit 사용률(남은 %)과 리셋까지 남은 시간을 표시하는 데스크톱 앱.

## 개요

터미널에서 `claude /usage` 같은 명령을 치지 않고도 메뉴바만 흘끗 보고 현재 토큰 사용 수준을 파악할 수 있도록 한다. 리셋 윈도우를 놓쳐 코딩 흐름이 끊기지 않도록 임계치 알림과 24시간 사용 패턴 시각화를 제공한다.

## 핵심 결정

- 사용량 수치는 각 CLI 가 이미 로컬에 저장해 둔 OAuth 자격증명을 재사용해 vendor 공식 usage API 에서 가져온다 (자격증명은 읽기 전용, [자격증명 처리](#자격증명-처리-읽기-전용) 참고). 공식 데이터가 없을 때만 로컬 로그 파싱으로 추정한다.
- 메뉴바 갱신 주기 90초 이상, 리셋 알림은 별도 oneshot 타이머로 정확하게 발사
- 기술 스택: Tauri (Rust 백엔드 + WebView 프런트엔드)
- 대상 플랫폼: macOS 13+ (SMAppService 자동 시작 사용)

## 자격증명 처리 (읽기 전용)

사용량은 각 CLI 가 로컬에 저장해 둔 OAuth 자격증명을 그대로 재사용해 공식 usage API 에서 조회한다. 사용자에게 별도 로그인이나 쿠키 입력을 요구하지 않는다.

- Claude Code: 키체인 `Claude Code-credentials` → `api.anthropic.com/api/oauth/usage`
- Codex: `~/.codex/auth.json` → `chatgpt.com/backend-api/wham/usage`

자격증명은 **읽기 전용으로만** 사용한다. 토큰이 만료돼도 token-notifier 는 OAuth refresh 를 수행하거나 키체인에 토큰을 다시 쓰지 않는다.

이유: Anthropic OAuth 는 rotating refresh token 이라 갱신 한 번에 기존 refresh token 이 무효화된다. token-notifier 가 Claude Code CLI 와 같은 키체인 자격증명을 공유하므로, 앱이 토큰을 갱신하면 CLI 가 들고 있던 토큰이 무효화되어 사용자가 매일 아침 `/login` 을 다시 요구받는 회귀가 있었다. 만료된 토큰은 CLI 가 스스로 갱신할 때 키체인에 반영되며, 앱은 다음 폴링에서 그 새 토큰을 따라간다. (코드상 `CLAUDE_OAUTH_REFRESH_ENABLED = false`)

### 토큰 scope 와 재로그인 안내

읽기 전용 정책 때문에, 저장된 토큰만으로는 공식 usage API 를 호출할 수 없는 경우가 생긴다. 이때 앱은 토큰을 갱신하지 않고 패널 해당 행에 **재로그인 안내**를 앤버색으로 띄운다.

- **Claude Code**: usage 엔드포인트(`/api/oauth/usage`)는 `user:profile` scope 를 요구한다. `user:profile` 이 로그인 scope 에 추가되기 전 발급된 레거시 토큰(예: `user:inference` 만 보유)은 `403 scope requirement user:profile` 을 받아 공식값을 못 가져온다. Claude Code 에서 `/login` 으로 한 번 재로그인하면 키체인 토큰에 `user:profile` 이 포함되어 해결된다.
- **Codex**: `~/.codex/auth.json` 을 **읽기만** 한다(앱은 자기 캐시 파일만 쓰고 auth.json 은 절대 쓰지 않음). 따라서 Codex CLI 세션을 무효화할 위험이 구조적으로 없다. 토큰이 만료돼 401/403 이 나면 세션파일·캐시 관측값으로 fallback 하고, 그마저 없으면 `codex login` 재로그인 안내를 띄운다.

재로그인 안내는 공식 인증 실패가 원인일 때만(Claude 403 scope / Codex 401·403) 나타난다. 네트워크 장애나 429 같은 일시적 실패는 기존 "공식 데이터 없음" 으로 둔다.

## 설계 문서

- 요구사항 스펙: [`.omc/specs/deep-interview-token-notifier.md`](./.omc/specs/deep-interview-token-notifier.md)
- 구현 계획 (consensus-reviewed): [`.omc/plans/token-notifier-consensus-plan.md`](./.omc/plans/token-notifier-consensus-plan.md)
