---
name: muconsole-wasm-ui
description: "Build and test the MuConsole unified WASM observability, CI/CD, Model-Runner, and Platform console across mu-console-protocol, mu-console-theme, mu-console-layout, mu-console-ui, and mu-console-data."
---

# `MuConsole` Unified WASM Frontend & UX Gate Invariants

## 1. Two-Skin Architecture & Dual-Layer Static Asset Caching

- Default to `HtmlCoreSkin` (`ConsoleSkin::Html`), locking non-`Active` sessions (`Unresolved`, `Anonymous`, `AuthorizationDenied`, `PasskeyEnrollmentRequired`) to `Html` and enabling `?skin=` + `@user -> Preferences` switching strictly on `AuthStatus::Active`.
- Serve HTML entry points with `Cache-Control: no-cache, no-store, must-revalidate` and WASM/JS/CSS assets with content-hash URL busting (`?v=<hash>`) and `Cache-Control: no-cache, must-revalidate`.
- Enforce mobile viewport constraints across all `mu-console-ui` components: contained segmented controls with horizontal scrolling, responsive labels, and `font-size: 16px` on form inputs to prevent iOS Safari auto-zoom.
