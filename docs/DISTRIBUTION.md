# 지인 배포 가이드 (ad-hoc 서명)

정식 Apple Developer 공증 없이 지인 소수에게 `.app` 을 전달하는 절차다.
정식 서명/공증이 아니므로 받는 사람은 Gatekeeper 를 **한 번** 우회해야 한다.

## 1. 빌드 + 배포 zip 생성

```bash
scripts/package-macos.sh
```

산출물:

- `src-tauri/target/release/bundle/macos/Token Notifier.app` (ad-hoc 서명됨)
- `dist/Token-Notifier-<version>.zip` (전달용)

이 zip 과 아래 "받는 사람 안내문"(`docs/INSTALL-ko.md`)을 함께 전달한다.

## 2. 전제 조건 (받는 사람 환경)

- macOS 13 (Ventura) 이상.
- 이 앱은 **자격증명을 새로 요구하지 않고** CLI 가 이미 저장해 둔 로그인 정보를 재사용한다.
  따라서 받는 사람 Mac 에 다음 중 최소 하나가 로그인돼 있어야 수치가 표시된다.
  - Claude Code CLI: keychain 항목 `Claude Code-credentials`
  - Codex(ChatGPT) CLI: `~/.codex/auth.json`
- 둘 다 없으면 메뉴바에는 떠도 사용량이 비어 보인다.

## 3. 왜 공증을 안 하나

- 무료(Developer Program 미가입) 경로라 ad-hoc 서명(`codesign --sign -`)만 가능하다.
- ad-hoc 은 본인 Mac 외에서는 Gatekeeper 가 차단하므로, 받는 사람이 quarantine 속성을
  제거(`xattr`)하거나 "확인 없이 열기"를 한 번 눌러야 한다. 자세한 절차는 `docs/INSTALL-ko.md`.
- 다수/공개 배포로 넘어가면 Developer ID 인증서 + notarization 이 사실상 필수다.
