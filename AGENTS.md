# AGENTS.md

## Project

CodexLaunch is a standalone Windows launcher for the Microsoft Store Codex app. It is not a Codex
plugin package.

The current verified goal is to start Store Codex with a local proxy without enabling the global
Windows proxy and without modifying files under `C:\Program Files\WindowsApps`.

## Launch Method

The launcher starts the Store app executable directly with `std::process::Command`:

```text
<InstallLocation>\app\Codex.exe --proxy-server=http://127.0.0.1:7897
```

It sets proxy environment variables only on that launched process:

```text
HTTP_PROXY=http://127.0.0.1:7897
HTTPS_PROXY=http://127.0.0.1:7897
ALL_PROXY=http://127.0.0.1:7897
NO_PROXY=localhost,127.0.0.1,::1
```

Windows child processes inherit this environment by default, so `resources\codex.exe app-server`
and tools it starts, such as `git.exe`, inherit the proxy env unless Codex explicitly overrides the
environment for that child.

Do not reintroduce the discarded approaches unless explicitly requested:

- `IApplicationActivationManager` / AUMID activation.
- Node `--require` preload injection.
- Registry or user environment writes.
- Local API relay.
- `app.asar` or `WindowsApps` file modification.

## Discovering Codex.exe

When `--codex-exe` is not provided, the launcher discovers the Store package install directory by
running PowerShell:

```powershell
Get-AppxPackage -Name OpenAI.Codex |
  Sort-Object Version -Descending |
  Select-Object -First 1
```

It reads the package `InstallLocation`, then appends:

```text
app\Codex.exe
```

For the current tested package this resolves to a path like:

```text
C:\Program Files\WindowsApps\OpenAI.Codex_26.608.1337.0_x64__2p2nqsd0c76g0\app\Codex.exe
```

If discovery fails or the Store package layout changes, pass an explicit executable path:

```powershell
cargo run -- launch --codex-exe "C:\Program Files\WindowsApps\...\app\Codex.exe"
```

## Verification

Use the narrow checks below after code changes:

```powershell
cargo test
cargo clippy -- -D warnings
cargo build
cargo run -- launch --dry-run
```

For real launch verification, close existing `Codex.exe` windows first, then run:

```powershell
cargo run -- launch
```

The launcher waits for `resources\codex.exe app-server` and verifies that it inherited the expected
proxy environment variables.

## Install Command

`install` copies the running executable into the current user's local app data directory:

```text
%LOCALAPPDATA%\CodexLaunch\CodexLaunch.exe
```

It then creates or refreshes this current-user Start Menu shortcut:

```text
%APPDATA%\Microsoft\Windows\Start Menu\Programs\CodexLaunch.lnk
```

The shortcut target is the installed launcher, its arguments are:

```text
launch
```

The shortcut icon must come from the Microsoft Store Codex executable discovered by the normal
Codex path lookup:

```text
<InstallLocation>\app\Codex.exe,0
```

The install command supports overwrite installation. If the launcher is already running from
`%LOCALAPPDATA%\CodexLaunch\CodexLaunch.exe`, skip self-copy and still refresh the shortcut. It also
removes the legacy `%APPDATA%\Microsoft\Windows\Start Menu\Programs\Codex++.lnk` shortcut if present.
