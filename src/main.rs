use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command as ProcessCommand;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use clap::{Args, Parser, Subcommand};

const DEFAULT_PACKAGE_NAME: &str = "OpenAI.Codex";
const DEFAULT_PROXY: &str = "http://127.0.0.1:7897";
const DEFAULT_NO_PROXY: &str = "localhost,127.0.0.1,::1";
const APP_DISPLAY_NAME: &str = "CodexLaunch";
const INSTALLED_EXE_NAME: &str = "CodexLaunch.exe";
const START_MENU_SHORTCUT_NAME: &str = "CodexLaunch.lnk";
const LEGACY_START_MENU_SHORTCUT_NAME: &str = "Codex++.lnk";

#[derive(Debug, Parser)]
#[command(name = "CodexLaunch", bin_name = "CodexLaunch", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Launch the Microsoft Store Codex app with proxy arguments and proxy env.
    Launch(LaunchOptions),
    /// Install CodexLaunch into the current user profile and create a Start Menu shortcut.
    Install(InstallOptions),
}

#[derive(Debug, Args, Clone)]
struct InstallOptions {
    /// Print install paths without copying files or creating the shortcut.
    #[arg(long)]
    dry_run: bool,
}

#[derive(Debug, Args, Clone)]
struct LaunchOptions {
    /// Codex.exe path. Defaults to the installed OpenAI.Codex package path.
    #[arg(long)]
    codex_exe: Option<PathBuf>,

    /// Local HTTP proxy used by Chromium and child-process proxy env.
    #[arg(long, default_value = DEFAULT_PROXY)]
    proxy: String,

    /// NO_PROXY/no_proxy value injected into child processes.
    #[arg(long, default_value = DEFAULT_NO_PROXY)]
    no_proxy: String,

    /// Do not set proxy environment variables; keep only Chromium --proxy-server.
    #[arg(long)]
    no_env: bool,

    /// Skip checking whether the Codex app-server inherited proxy environment variables.
    #[arg(long)]
    no_env_check: bool,

    /// Milliseconds to wait for the Codex app-server environment check.
    #[arg(long, default_value_t = 10_000)]
    env_check_timeout_ms: u64,

    /// Add --remote-debugging-port=<PORT> to Codex startup arguments.
    #[arg(long)]
    remote_debugging_port: Option<u16>,

    /// Add --remote-allow-origins=<VALUE> when remote debugging is enabled.
    #[arg(long)]
    remote_allow_origins: Option<String>,

    /// Print the launch target and arguments without launching Codex.
    #[arg(long)]
    dry_run: bool,

    /// Allow launch when Codex.exe is already running.
    #[arg(long)]
    allow_existing_instance: bool,

    /// Extra arguments passed to Codex after `--`.
    #[arg(last = true)]
    extra_args: Vec<String>,
}

#[derive(Debug)]
struct LaunchPlan {
    codex_exe: Option<PathBuf>,
    args: Vec<String>,
    proxy_env: Vec<(String, String)>,
    proxy_env_removals: Vec<String>,
    dry_run: bool,
    allow_existing_instance: bool,
    env_check_timeout: Option<Duration>,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command.unwrap_or_else(default_command) {
        Command::Launch(options) => launch(options),
        Command::Install(options) => install(options),
    }
}

fn default_command() -> Command {
    Command::Launch(default_launch_options())
}

fn default_launch_options() -> LaunchOptions {
    LaunchOptions {
        codex_exe: None,
        proxy: DEFAULT_PROXY.to_string(),
        no_proxy: DEFAULT_NO_PROXY.to_string(),
        no_env: false,
        no_env_check: false,
        env_check_timeout_ms: 10_000,
        remote_debugging_port: None,
        remote_allow_origins: None,
        dry_run: false,
        allow_existing_instance: false,
        extra_args: Vec::new(),
    }
}

fn launch(options: LaunchOptions) -> Result<()> {
    let plan = build_launch_plan(options)?;
    let codex_exe = resolve_codex_exe(plan.codex_exe.as_deref())?;

    if plan.proxy_env.is_empty() {
        if plan.proxy_env_removals.is_empty() {
            println!("Proxy env: disabled");
        } else {
            println!("Proxy env: disabled; inherited proxy env will be removed");
        }
    } else {
        println!("Proxy env: per-process HTTP_PROXY/HTTPS_PROXY/ALL_PROXY/NO_PROXY");
    }

    println!("Codex exe: {}", codex_exe.display());
    println!(
        "Starting: {}",
        format_direct_command(&codex_exe, &plan.args)
    );

    if plan.dry_run {
        return Ok(());
    }

    if !plan.allow_existing_instance && process_is_running("Codex.exe")? {
        bail!(
            "Codex.exe is already running. Close Codex first so startup proxy settings are applied to a fresh process, or pass --allow-existing-instance to skip this guard."
        );
    }

    let pid = spawn_codex_direct(
        &codex_exe,
        &plan.args,
        &plan.proxy_env,
        &plan.proxy_env_removals,
    )?;
    println!("Started Codex process id: {pid}");

    if let Some(timeout) = plan.env_check_timeout {
        wait_for_app_server_proxy_env(pid, &plan.proxy_env, timeout)?;
        println!("Proxy env confirmed in Codex app-server.");
    }

    Ok(())
}

fn install(options: InstallOptions) -> Result<()> {
    let source_exe = env::current_exe().context("failed to resolve current executable path")?;
    let install_dir = default_install_dir()?;
    let target_exe = install_dir.join(INSTALLED_EXE_NAME);
    let shortcut_path = default_start_menu_shortcut_path()?;
    let icon_path =
        resolve_codex_exe(None).context("failed to resolve Store Codex.exe for shortcut icon")?;

    println!(
        "{APP_DISPLAY_NAME} install source: {}",
        source_exe.display()
    );
    println!(
        "{APP_DISPLAY_NAME} install target: {}",
        target_exe.display()
    );
    println!("{APP_DISPLAY_NAME} shortcut: {}", shortcut_path.display());
    println!(
        "{APP_DISPLAY_NAME} shortcut icon: {},0",
        icon_path.display()
    );

    if options.dry_run {
        return Ok(());
    }

    fs::create_dir_all(&install_dir).with_context(|| {
        format!(
            "failed to create install directory {}",
            install_dir.display()
        )
    })?;

    if target_exe.exists() && same_file_path(&source_exe, &target_exe) {
        println!("{APP_DISPLAY_NAME} is already running from the install target; skipping copy.");
    } else {
        fs::copy(&source_exe, &target_exe).with_context(|| {
            format!(
                "failed to copy {} to {}",
                source_exe.display(),
                target_exe.display()
            )
        })?;
        println!("Installed {APP_DISPLAY_NAME} executable.");
    }

    create_start_menu_shortcut(&shortcut_path, &target_exe, &install_dir, &icon_path)?;
    println!("Installed {APP_DISPLAY_NAME} Start Menu shortcut.");
    remove_legacy_start_menu_shortcut()?;

    Ok(())
}

fn build_launch_plan(options: LaunchOptions) -> Result<LaunchPlan> {
    let mut args = Vec::new();
    args.push(format!("--proxy-server={}", options.proxy));
    let proxy_env = if options.no_env {
        Vec::new()
    } else {
        proxy_env_vars(&options.proxy, &options.no_proxy)
    };
    let proxy_env_removals = if options.no_env {
        proxy_env_removal_keys()
            .into_iter()
            .map(ToOwned::to_owned)
            .collect()
    } else {
        Vec::new()
    };

    if let Some(port) = options.remote_debugging_port {
        args.push(format!("--remote-debugging-port={port}"));
        if let Some(origins) = options.remote_allow_origins {
            args.push(format!("--remote-allow-origins={origins}"));
        }
    }

    args.extend(options.extra_args);

    Ok(LaunchPlan {
        codex_exe: options.codex_exe,
        args,
        proxy_env,
        proxy_env_removals,
        dry_run: options.dry_run,
        allow_existing_instance: options.allow_existing_instance,
        env_check_timeout: if options.no_env_check
            || options.no_env
            || options.env_check_timeout_ms == 0
        {
            None
        } else {
            Some(Duration::from_millis(options.env_check_timeout_ms))
        },
    })
}

fn proxy_env_vars(proxy: &str, no_proxy: &str) -> Vec<(String, String)> {
    [
        ("HTTP_PROXY", proxy),
        ("HTTPS_PROXY", proxy),
        ("ALL_PROXY", proxy),
        ("NO_PROXY", no_proxy),
    ]
    .into_iter()
    .map(|(key, value)| (key.to_string(), value.to_string()))
    .collect()
}

fn proxy_env_removal_keys() -> [&'static str; 8] {
    [
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "ALL_PROXY",
        "NO_PROXY",
        "http_proxy",
        "https_proxy",
        "all_proxy",
        "no_proxy",
    ]
}

fn missing_env_vars(env_vars: &[String], expected: &[(String, String)]) -> Vec<String> {
    expected
        .iter()
        .filter_map(|(key, expected_value)| {
            let found = env_vars.iter().any(|entry| {
                let Some((entry_key, entry_value)) = entry.split_once('=') else {
                    return false;
                };
                entry_key.eq_ignore_ascii_case(key) && entry_value == expected_value
            });
            if found { None } else { Some(key.clone()) }
        })
        .collect()
}

fn wait_for_app_server_proxy_env(
    parent_pid: u32,
    expected: &[(String, String)],
    timeout: Duration,
) -> Result<()> {
    if expected.is_empty() {
        return Ok(());
    }

    let deadline = Instant::now() + timeout;

    while Instant::now() < deadline {
        if let Some(app_server_pid) = find_child_process_by_name(parent_pid, "codex.exe")? {
            let env_vars = read_process_environment(app_server_pid).with_context(|| {
                format!("failed to read app-server env from pid {app_server_pid}")
            })?;
            let missing = missing_env_vars(&env_vars, expected);
            if missing.is_empty() {
                return Ok(());
            }

            bail!(
                "Codex app-server pid {app_server_pid} did not inherit proxy env; missing {}.",
                missing.join(", ")
            );
        }

        thread::sleep(Duration::from_millis(200));
    }

    bail!(
        "Codex app-server child process was not observed within {} ms",
        timeout.as_millis()
    )
}

#[cfg(windows)]
fn find_child_process_by_name(parent_pid: u32, image_name: &str) -> Result<Option<u32>> {
    use std::mem;

    use windows::Win32::Foundation::{CloseHandle, HANDLE};
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
        TH32CS_SNAPPROCESS,
    };

    struct HandleGuard(HANDLE);

    impl Drop for HandleGuard {
        fn drop(&mut self) {
            unsafe {
                let _ = CloseHandle(self.0);
            }
        }
    }

    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) }
        .context("failed to create process snapshot")?;
    let _guard = HandleGuard(snapshot);

    let mut entry = PROCESSENTRY32W {
        dwSize: mem::size_of::<PROCESSENTRY32W>() as u32,
        ..Default::default()
    };

    if unsafe { Process32FirstW(snapshot, &mut entry) }.is_err() {
        return Ok(None);
    }

    loop {
        let exe = nul_terminated_utf16_to_string(&entry.szExeFile);
        if entry.th32ParentProcessID == parent_pid && exe.eq_ignore_ascii_case(image_name) {
            return Ok(Some(entry.th32ProcessID));
        }

        if unsafe { Process32NextW(snapshot, &mut entry) }.is_err() {
            break;
        }
    }

    Ok(None)
}

#[cfg(not(windows))]
fn find_child_process_by_name(_parent_pid: u32, _image_name: &str) -> Result<Option<u32>> {
    Ok(None)
}

#[cfg(windows)]
fn read_process_environment(pid: u32) -> Result<Vec<String>> {
    use std::mem;

    use windows::Win32::Foundation::{CloseHandle, HANDLE};
    use windows::Win32::System::Threading::{
        OpenProcess, PROCESS_QUERY_INFORMATION, PROCESS_VM_READ,
    };

    #[repr(C)]
    #[derive(Default)]
    struct ProcessBasicInformation {
        reserved1: isize,
        peb_base_address: usize,
        reserved2: [usize; 2],
        unique_process_id: usize,
        inherited_from_unique_process_id: usize,
    }

    #[link(name = "ntdll")]
    unsafe extern "system" {
        fn NtQueryInformationProcess(
            process_handle: HANDLE,
            process_information_class: u32,
            process_information: *mut ProcessBasicInformation,
            process_information_length: u32,
            return_length: *mut u32,
        ) -> i32;
    }

    struct HandleGuard(HANDLE);

    impl Drop for HandleGuard {
        fn drop(&mut self) {
            unsafe {
                let _ = CloseHandle(self.0);
            }
        }
    }

    let handle = unsafe { OpenProcess(PROCESS_QUERY_INFORMATION | PROCESS_VM_READ, false, pid) }
        .with_context(|| format!("failed to open process {pid}"))?;
    let _guard = HandleGuard(handle);

    let mut pbi = ProcessBasicInformation::default();
    let mut return_length = 0;
    let status = unsafe {
        NtQueryInformationProcess(
            handle,
            0,
            &mut pbi,
            mem::size_of::<ProcessBasicInformation>() as u32,
            &mut return_length,
        )
    };
    if status != 0 {
        bail!("NtQueryInformationProcess failed with status 0x{status:x}");
    }

    #[cfg(not(target_pointer_width = "64"))]
    bail!("remote environment reading is only implemented for 64-bit processes");

    #[cfg(target_pointer_width = "64")]
    {
        const PEB_PROCESS_PARAMETERS_OFFSET: usize = 0x20;
        const PROCESS_PARAMETERS_ENVIRONMENT_OFFSET: usize = 0x80;

        let process_parameters =
            read_remote_usize(handle, pbi.peb_base_address + PEB_PROCESS_PARAMETERS_OFFSET)?;
        let environment_address = read_remote_usize(
            handle,
            process_parameters + PROCESS_PARAMETERS_ENVIRONMENT_OFFSET,
        )?;

        let raw = read_remote_utf16_double_nul_string(handle, environment_address)?;
        Ok(raw
            .split('\0')
            .filter(|item| !item.is_empty())
            .map(ToOwned::to_owned)
            .collect())
    }
}

#[cfg(not(windows))]
fn read_process_environment(_pid: u32) -> Result<Vec<String>> {
    bail!("remote environment reading is only supported on Windows")
}

#[cfg(windows)]
fn read_remote_usize(handle: windows::Win32::Foundation::HANDLE, address: usize) -> Result<usize> {
    use std::ffi::c_void;
    use std::mem;

    use windows::Win32::System::Diagnostics::Debug::ReadProcessMemory;

    let mut buffer = [0u8; mem::size_of::<usize>()];
    let mut bytes_read = 0;
    unsafe {
        ReadProcessMemory(
            handle,
            address as *const c_void,
            buffer.as_mut_ptr() as *mut c_void,
            buffer.len(),
            Some(&mut bytes_read),
        )
    }
    .with_context(|| format!("failed to read process memory at 0x{address:x}"))?;

    if bytes_read != buffer.len() {
        bail!(
            "short read at 0x{address:x}: expected {}, got {bytes_read}",
            buffer.len()
        );
    }

    Ok(usize::from_ne_bytes(buffer))
}

#[cfg(windows)]
fn read_remote_utf16_double_nul_string(
    handle: windows::Win32::Foundation::HANDLE,
    address: usize,
) -> Result<String> {
    use std::ffi::c_void;

    use windows::Win32::System::Diagnostics::Debug::ReadProcessMemory;

    let mut bytes = Vec::new();
    let mut offset = 0usize;

    while offset < 256 * 1024 {
        let mut chunk = [0u8; 4096];
        let mut bytes_read = 0;
        let read_result = unsafe {
            ReadProcessMemory(
                handle,
                (address + offset) as *const c_void,
                chunk.as_mut_ptr() as *mut c_void,
                chunk.len(),
                Some(&mut bytes_read),
            )
        };

        if read_result.is_err() {
            if bytes.is_empty() {
                read_result.with_context(|| {
                    format!("failed to read environment at 0x{:x}", address + offset)
                })?;
            }
            break;
        }

        bytes.extend_from_slice(&chunk[..bytes_read]);
        if contains_utf16_double_nul(&bytes) {
            break;
        }
        offset += bytes_read;
    }

    let byte_limit = utf16_double_nul_position(&bytes).unwrap_or(bytes.len());
    let utf16 = bytes[..byte_limit]
        .chunks_exact(2)
        .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]))
        .collect::<Vec<_>>();

    Ok(String::from_utf16_lossy(&utf16))
}

#[cfg(windows)]
fn contains_utf16_double_nul(bytes: &[u8]) -> bool {
    utf16_double_nul_position(bytes).is_some()
}

#[cfg(windows)]
fn utf16_double_nul_position(bytes: &[u8]) -> Option<usize> {
    bytes
        .windows(4)
        .step_by(2)
        .position(|window| window == [0, 0, 0, 0])
        .map(|index| index * 2)
}

#[cfg(windows)]
fn nul_terminated_utf16_to_string(buffer: &[u16]) -> String {
    let len = buffer
        .iter()
        .position(|ch| *ch == 0)
        .unwrap_or(buffer.len());
    String::from_utf16_lossy(&buffer[..len])
}

#[cfg(windows)]
fn process_is_running(image_name: &str) -> Result<bool> {
    let filter = format!("IMAGENAME eq {image_name}");
    let output = ProcessCommand::new("tasklist")
        .args(["/FI", filter.as_str(), "/FO", "CSV", "/NH"])
        .output()
        .context("failed to run tasklist")?;

    if !output.status.success() {
        bail!("tasklist failed with status {}", output.status);
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let expected_prefix = format!("\"{image_name}\"");
    Ok(stdout
        .lines()
        .any(|line| line.trim_start().starts_with(&expected_prefix)))
}

#[cfg(not(windows))]
fn process_is_running(_image_name: &str) -> Result<bool> {
    Ok(false)
}

fn quote_windows_arg(arg: &str) -> String {
    if arg.is_empty() {
        return "\"\"".to_string();
    }

    if !arg.chars().any(|c| c == '"' || c.is_whitespace()) {
        return arg.to_string();
    }

    let mut quoted = String::from("\"");
    let mut backslashes = 0;

    for ch in arg.chars() {
        match ch {
            '\\' => backslashes += 1,
            '"' => {
                quoted.push_str(&"\\".repeat(backslashes * 2 + 1));
                quoted.push('"');
                backslashes = 0;
            }
            _ => {
                quoted.push_str(&"\\".repeat(backslashes));
                backslashes = 0;
                quoted.push(ch);
            }
        }
    }

    quoted.push_str(&"\\".repeat(backslashes * 2));
    quoted.push('"');
    quoted
}

fn format_direct_command(executable: &Path, args: &[String]) -> String {
    let executable = executable.display().to_string();
    let mut parts = Vec::with_capacity(args.len() + 1);
    parts.push(quote_windows_arg(&executable));
    parts.extend(args.iter().map(|arg| quote_windows_arg(arg)));
    parts.join(" ")
}

fn resolve_codex_exe(explicit: Option<&Path>) -> Result<PathBuf> {
    if let Some(path) = explicit {
        if path.is_file() {
            return Ok(path.to_path_buf());
        }

        bail!("Codex executable does not exist: {}", path.display());
    }

    default_codex_exe_path()
}

fn default_install_dir() -> Result<PathBuf> {
    let local_app_data = env::var_os("LOCALAPPDATA").context("LOCALAPPDATA is not set")?;
    Ok(PathBuf::from(local_app_data).join(APP_DISPLAY_NAME))
}

fn default_start_menu_shortcut_path() -> Result<PathBuf> {
    let app_data = env::var_os("APPDATA").context("APPDATA is not set")?;
    Ok(PathBuf::from(app_data)
        .join("Microsoft")
        .join("Windows")
        .join("Start Menu")
        .join("Programs")
        .join(START_MENU_SHORTCUT_NAME))
}

fn legacy_start_menu_shortcut_path() -> Result<PathBuf> {
    let app_data = env::var_os("APPDATA").context("APPDATA is not set")?;
    Ok(PathBuf::from(app_data)
        .join("Microsoft")
        .join("Windows")
        .join("Start Menu")
        .join("Programs")
        .join(LEGACY_START_MENU_SHORTCUT_NAME))
}

fn remove_legacy_start_menu_shortcut() -> Result<()> {
    let legacy_shortcut = legacy_start_menu_shortcut_path()?;
    if legacy_shortcut.exists() {
        fs::remove_file(&legacy_shortcut).with_context(|| {
            format!(
                "failed to remove legacy Start Menu shortcut {}",
                legacy_shortcut.display()
            )
        })?;
        println!(
            "Removed legacy Start Menu shortcut {}.",
            legacy_shortcut.display()
        );
    }

    Ok(())
}

fn same_file_path(left: &Path, right: &Path) -> bool {
    let Ok(left) = left.canonicalize() else {
        return false;
    };
    let Ok(right) = right.canonicalize() else {
        return false;
    };
    left == right
}

#[cfg(windows)]
fn default_codex_exe_path() -> Result<PathBuf> {
    let install_location = find_appx_install_location(DEFAULT_PACKAGE_NAME)?;
    let codex_exe = install_location.join("app").join("Codex.exe");

    if !codex_exe.is_file() {
        bail!("resolved Codex.exe does not exist: {}", codex_exe.display());
    }

    Ok(codex_exe)
}

#[cfg(not(windows))]
fn default_codex_exe_path() -> Result<PathBuf> {
    bail!("automatic Codex.exe discovery is only supported on Windows; pass --codex-exe")
}

#[cfg(windows)]
fn find_appx_install_location(package_name: &str) -> Result<PathBuf> {
    let script = format!(
        "$pkg = Get-AppxPackage -Name '{package_name}' | Sort-Object Version -Descending | Select-Object -First 1; if ($null -eq $pkg) {{ exit 2 }}; Write-Output $pkg.InstallLocation"
    );
    let output = ProcessCommand::new("powershell.exe")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            &script,
        ])
        .output()
        .context("failed to run powershell.exe for Get-AppxPackage")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!(
            "failed to resolve Codex install location with Get-AppxPackage -Name {package_name}: {}",
            stderr.trim()
        );
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let install_location = stdout
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .context("Get-AppxPackage returned an empty InstallLocation")?;

    Ok(PathBuf::from(install_location))
}

#[cfg(windows)]
fn create_start_menu_shortcut(
    shortcut_path: &Path,
    target_exe: &Path,
    working_dir: &Path,
    icon_path: &Path,
) -> Result<()> {
    if let Some(parent) = shortcut_path.parent() {
        fs::create_dir_all(parent).with_context(|| {
            format!("failed to create Start Menu directory {}", parent.display())
        })?;
    }

    let ps_shortcut_path = quote_powershell_string(
        shortcut_path
            .to_str()
            .context("shortcut path is not valid UTF-8")?,
    );
    let ps_target_path = quote_powershell_string(
        target_exe
            .to_str()
            .context("target path is not valid UTF-8")?,
    );
    let ps_arguments = quote_powershell_string("launch");
    let ps_working_directory = quote_powershell_string(
        working_dir
            .to_str()
            .context("working directory path is not valid UTF-8")?,
    );
    let ps_icon_location = quote_powershell_string(&format!("{},0", icon_path.display()));

    let script = format!(
        r#"
$ErrorActionPreference = 'Stop'
$shell = New-Object -ComObject WScript.Shell
$shortcut = $shell.CreateShortcut({ps_shortcut_path})
$shortcut.TargetPath = {ps_target_path}
$shortcut.Arguments = {ps_arguments}
$shortcut.WorkingDirectory = {ps_working_directory}
$shortcut.IconLocation = {ps_icon_location}
$shortcut.Save()
"#
    );

    let output = ProcessCommand::new("powershell.exe")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            &script,
        ])
        .output()
        .context("failed to run powershell.exe to create Start Menu shortcut")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!(
            "failed to create Start Menu shortcut {}: {}",
            shortcut_path.display(),
            stderr.trim()
        );
    }

    Ok(())
}

fn quote_powershell_string(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

#[cfg(not(windows))]
fn create_start_menu_shortcut(
    _shortcut_path: &Path,
    _target_exe: &Path,
    _working_dir: &Path,
    _icon_path: &Path,
) -> Result<()> {
    bail!("Start Menu shortcut installation is only supported on Windows")
}

fn spawn_codex_direct(
    codex_exe: &Path,
    args: &[String],
    proxy_env: &[(String, String)],
    proxy_env_removals: &[String],
) -> Result<u32> {
    let mut command = ProcessCommand::new(codex_exe);
    command.args(args);
    for key in proxy_env_removals {
        command.env_remove(key);
    }
    for (key, value) in proxy_env {
        command.env(key, value);
    }

    let child = command
        .spawn()
        .with_context(|| format!("failed to start {}", codex_exe.display()))?;
    Ok(child.id())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotes_plain_args_without_changes() {
        assert_eq!(
            quote_windows_arg("--proxy-server=http://127.0.0.1:7897"),
            "--proxy-server=http://127.0.0.1:7897"
        );
    }

    #[test]
    fn quotes_paths_with_spaces() {
        assert_eq!(
            quote_windows_arg(r"C:\Users\Name With Space\Codex.exe"),
            r#""C:\Users\Name With Space\Codex.exe""#
        );
    }

    #[test]
    fn formats_direct_command_with_quoted_executable() {
        let command = format_direct_command(
            Path::new(r"C:\Program Files\WindowsApps\OpenAI.Codex\app\Codex.exe"),
            &["--proxy-server=http://127.0.0.1:7897".to_string()],
        );

        assert_eq!(
            command,
            r#""C:\Program Files\WindowsApps\OpenAI.Codex\app\Codex.exe" --proxy-server=http://127.0.0.1:7897"#
        );
    }

    #[test]
    fn doubles_trailing_backslashes_inside_quotes() {
        assert_eq!(
            quote_windows_arg(r"C:\Temp Folder\"),
            r#""C:\Temp Folder\\""#
        );
    }

    #[test]
    fn quotes_powershell_strings() {
        assert_eq!(
            quote_powershell_string(r"C:\Users\O'Brien\CodexLaunch.lnk"),
            r#"'C:\Users\O''Brien\CodexLaunch.lnk'"#
        );
    }

    #[test]
    fn builds_proxy_env_vars() {
        let vars = proxy_env_vars("http://127.0.0.1:7897", "localhost");
        assert!(vars.contains(&(
            "HTTP_PROXY".to_string(),
            "http://127.0.0.1:7897".to_string()
        )));
        assert!(vars.contains(&("NO_PROXY".to_string(), "localhost".to_string())));
    }

    #[test]
    fn default_plan_uses_proxy_env() {
        let plan = build_launch_plan(default_launch_options()).unwrap();

        assert!(plan.codex_exe.is_none());
        assert_eq!(
            plan.args,
            vec!["--proxy-server=http://127.0.0.1:7897".to_string()]
        );
        assert_eq!(plan.proxy_env.len(), 4);
        assert!(plan.env_check_timeout.is_some());
    }

    #[test]
    fn no_env_disables_proxy_env_and_env_check() {
        let mut options = default_launch_options();
        options.no_env = true;

        let plan = build_launch_plan(options).unwrap();

        assert!(plan.proxy_env.is_empty());
        assert!(plan.proxy_env_removals.contains(&"HTTP_PROXY".to_string()));
        assert!(plan.proxy_env_removals.contains(&"http_proxy".to_string()));
        assert!(plan.env_check_timeout.is_none());
    }

    #[test]
    fn custom_codex_exe_is_kept_in_plan() {
        let mut options = default_launch_options();
        options.codex_exe = Some(PathBuf::from(r"C:\Codex.exe"));

        let plan = build_launch_plan(options).unwrap();

        assert_eq!(plan.codex_exe, Some(PathBuf::from(r"C:\Codex.exe")));
    }
}
