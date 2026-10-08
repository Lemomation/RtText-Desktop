# RtText Desktop (Windows)

A lightweight, high-performance native Windows client for **RtText** written in **Rust** with a **1:1 Material 3 Dark UI** directly mirroring the Android mobile app.

## Features
- **Ultra-low RAM usage**: Runs in **~15–20 MB RAM** in the background.
- **System Tray Integration**: Minimizes to the Windows system tray on close with left-click restore and right-click context menu.
- **Material 3 Dark UI**: Styled with RtText's signature tokens (`#0B0F14` background, `#11161D` surface, `#2DD4BF` teal accent).
- **Navigation & Screens**: Material 3 bottom navigation bar with **Chats**, **Discover**, and **Profile** tabs, plus instant chat view with message bubbles.
- **Supabase Connectivity**: Ready for GoTrue authentication, PostgREST message history, and Realtime WebSocket syncing.
- **Zero Local Build Overhead**: Automated compilation, testing, and packaging handled entirely via **GitHub Actions**.

## Automated GitHub Actions CI/CD
Every push to `main` automatically triggers `.github/workflows/build.yml` on `windows-latest` to:
1. Validate Rust syntax and types (`cargo check --release`).
2. Run test suites (`cargo test --release`).
3. Compile the optimized standalone binary (`cargo build --release`).
4. Package and release `RtText-Windows-x86_64.zip` as an Action Artifact and GitHub Release.

## License
MIT
