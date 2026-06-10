# CodexLaunch

CodexLaunch is an MVP launcher for the Microsoft Store Codex app on Windows. It starts the packaged
Codex app with Chromium proxy flags and per-process proxy environment variables.

## Scope

- Launches the installed Store package `app\Codex.exe` directly.
- Adds `--proxy-server=http://127.0.0.1:7897` by default.
- Sets `HTTP_PROXY`, `HTTPS_PROXY`, `ALL_PROXY`, and `NO_PROXY` only on the launched Codex process.
- Waits for the Codex `resources\codex.exe app-server` child process and verifies that it inherited
  the proxy environment variables.
- Refuses to launch over an already running `Codex.exe` by default, because startup flags only
  reliably apply to a fresh process.

## Usage

Build:

```powershell
cargo build
```

Build a release executable:

```powershell
cargo build --release
```

Install for the current user:

```powershell
.\target\release\codexlaunch.exe install
```

This copies the running executable to:

```text
%LOCALAPPDATA%\CodexLaunch\CodexLaunch.exe
```

It also creates a current-user Start Menu shortcut:

```text
%APPDATA%\Microsoft\Windows\Start Menu\Programs\CodexLaunch.lnk
```

The shortcut launches `CodexLaunch.exe launch` and uses the Microsoft Store `app\Codex.exe` icon.
Running `install` again overwrites the installed executable, refreshes the shortcut, and removes the
legacy `Codex++.lnk` shortcut if present.

Preview the direct launch command without launching Codex:

```powershell
cargo run -- launch --dry-run
```

Launch with the default local proxy:

```powershell
cargo run -- launch
```

Launch with a custom proxy:

```powershell
cargo run -- launch --proxy http://127.0.0.1:7890
```

Launch a specific Codex executable directly:

```powershell
cargo run -- launch --codex-exe "C:\Program Files\WindowsApps\OpenAI.Codex_26.608.1337.0_x64__2p2nqsd0c76g0\app\Codex.exe"
```

Launch without proxy environment variables, keeping only Chromium `--proxy-server`:

```powershell
cargo run -- launch --no-env
```

In direct mode this also removes inherited `HTTP_PROXY`, `HTTPS_PROXY`, `ALL_PROXY`, `NO_PROXY`,
and their lowercase variants from the launched process.

Launch without checking whether `app-server` inherited proxy environment variables:

```powershell
cargo run -- launch --no-env-check
```

Enable remote debugging for inspection:

```powershell
cargo run -- launch --remote-debugging-port 9229 --remote-allow-origins http://127.0.0.1:9229
```

Pass extra Codex/Electron arguments after `--`:

```powershell
cargo run -- launch -- --some-electron-flag=value
```

If Codex is already running and you intentionally want to skip the fresh-process guard:

```powershell
cargo run -- launch --allow-existing-instance
```

After installation, the installed launcher can be run directly:

```powershell
%LOCALAPPDATA%\CodexLaunch\CodexLaunch.exe launch
```

## Current Limitations

- Direct mode discovers the installed package with `Get-AppxPackage -Name OpenAI.Codex` and starts
  `app\Codex.exe`. Pass `--codex-exe` if discovery fails or the package layout changes.
- Direct mode still depends on the Store app accepting direct execution of `app\Codex.exe`.
- It does not modify `app.asar` or files under `C:\Program Files\WindowsApps`.
- It does not write registry environment values or change the global Windows proxy.
