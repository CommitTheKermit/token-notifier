# token-notifier

macOS 메뉴바에 Claude Code CLI의 로컬 사용량 추정과 Codex CLI의 공식 rate-limit 관측값이 있을 때만 남은 % 및 리셋까지 남은 시간을 표시하는 데스크톱 앱.

## 개요

터미널에서 `claude /usage` 같은 명령을 치지 않고도 메뉴바만 흘끗 보고 현재 토큰 사용 수준을 파악할 수 있도록 한다. 리셋 윈도우를 놓쳐 코딩 흐름이 끊기지 않도록 임계치 알림과 24시간 사용 패턴 시각화를 제공한다.

## 핵심 결정

- 데이터 수집은 로컬 로그/세션 파일 파싱만 사용 (외부 네트워크 호출 없음)
- 메뉴바 갱신 주기 60초 이상, 리셋 알림은 별도 oneshot 타이머로 정확하게 발사
- 기술 스택: Tauri (Rust 백엔드 + WebView 프런트엔드)
- 대상 플랫폼: macOS 13+ (SMAppService 자동 시작 사용)

## 설계 문서

- 요구사항 스펙: [`.omc/specs/deep-interview-token-notifier.md`](./.omc/specs/deep-interview-token-notifier.md)
- 구현 계획 (consensus-reviewed): [`.omc/plans/token-notifier-consensus-plan.md`](./.omc/plans/token-notifier-consensus-plan.md)

## 상태

설계 완료, 구현 착수 전. Plan은 Architect/Critic 합의로 검증되었으며 `PENDING APPROVAL` 상태이다.
