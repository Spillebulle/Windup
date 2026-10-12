# Linux support — plan

Spec: `docs/superpowers/specs/2026-10-12-linux-support-design.md`.
Executed inline, test-first for every pure piece. Each task ends with
`cargo check --target x86_64-unknown-linux-gnu`, the Windows build, and a
commit.

1. **core::placement** — move the popup placement math out of `main.rs` into a
   pure `place(cursor, work, size) -> pos`; tests for each edge flip; Windows
   path calls it.
2. **core::ipc** — `Command` enum, line encode/decode, socket path from
   `$XDG_RUNTIME_DIR` (fallback `/tmp/windup-$UID`), CLI parsing
   (`--popup`, `--paste-plain`, `--show`); tests.
3. **core::clipboard::capture** — `Ingest` (last text / last image hash),
   MIME pick, secret-hint rule; poller rewritten on top; tests.
4. **desktop::platform** — `Target`, `Backend`, pure `detect`; tests.
   `HotkeyEvent` carries `Target`.
5. **desktop::ipc_server** + CLI client in `main` + single instance.
6. **desktop::x11** — pointer/work area/active window/activate via `x11rb`;
   cursor.rs uses it.
7. **desktop::wayland** — connection thread: registry probe for `detect`,
   data-control watch + serve, virtual keyboard paste, COSMIC toplevel
   tracking + activate.
8. **desktop::wayland::popup** — layer surface + egui-wgpu host driving
   `PopupView`; placement at pointer enter; dismissal rules.
9. **desktop::portal** — GlobalShortcuts + RemoteDesktop fallbacks (ashpd).
10. **desktop::autostart** on Linux.
11. **Packaging** — app id, desktop entry, metainfo, icon PNGs, `package.sh`,
    PKGBUILD, archive enrolment, workflow rows, `tests/release.rs`, README.
12. **E2E** — `tools/linux-e2e.sh` (headless Sway, Xvfb) and its CI jobs.
13. **Docs** — `docs/linux.md` (per-desktop shortcut setup + the owner's
    COSMIC checklist), CLAUDE.md, packaging.md.
