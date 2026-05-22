# Packaging Token Notifier

Step 10 packaging uses the Tauri 2 CLI and ad-hoc signing for local macOS use.

```bash
cargo install tauri-cli --version 2.11.2 # if `cargo tauri` is missing
scripts/package-macos.sh
```

Expected app bundle:

```text
src-tauri/target/release/bundle/macos/Token Notifier.app
```

Verification performed in Step 10:

```bash
cargo tauri build
codesign --force --deep --sign - "src-tauri/target/release/bundle/macos/Token Notifier.app"
codesign --verify --deep --strict --verbose=2 "src-tauri/target/release/bundle/macos/Token Notifier.app"
```

Manual checks that still require a live user session:

- First-run Notification Center permission prompt.
- Home-directory log access behavior on the target machine.
- SMAppService Login Items approval and reboot/login relaunch.
