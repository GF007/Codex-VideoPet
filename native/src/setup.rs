use crate::{core::now_ms, hooks, runtime::API_ADDRESS, settings};
use serde::Deserialize;
use std::{
    ffi::OsStr,
    io::{Read, Write},
    net::{SocketAddr, TcpStream},
    os::windows::{ffi::OsStrExt, process::CommandExt},
    process::{Command, Stdio},
    time::Duration,
};
use windows::{
    core::{w, PCWSTR},
    Win32::{
        Foundation::{
            CloseHandle, GetLastError, ERROR_ALREADY_EXISTS, HANDLE, HINSTANCE, HWND, LPARAM,
            LRESULT, WPARAM,
        },
        Graphics::Gdi::{GetStockObject, UpdateWindow, DEFAULT_GUI_FONT},
        System::{
            LibraryLoader::GetModuleHandleW,
            Threading::{CreateMutexW, CREATE_NO_WINDOW},
        },
        UI::{
            Controls::BST_CHECKED,
            HiDpi::{SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2},
            WindowsAndMessaging::{
                CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetMessageW,
                GetSystemMetrics, GetWindowLongPtrW, LoadCursorW, PostQuitMessage, RegisterClassW,
                SendMessageW, SetWindowLongPtrW, SetWindowTextW, ShowWindow, TranslateMessage,
                BN_CLICKED, BS_AUTOCHECKBOX, BS_DEFPUSHBUTTON, BS_PUSHBUTTON, CS_HREDRAW,
                CS_VREDRAW, GWLP_USERDATA, HMENU, IDC_ARROW, MSG, SM_CXSCREEN, SM_CYSCREEN,
                SW_SHOW, WINDOW_EX_STYLE, WM_CLOSE, WM_COMMAND, WM_DESTROY, WM_SETFONT, WNDCLASSW,
                WS_CAPTION, WS_CHILD, WS_EX_CONTROLPARENT, WS_MINIMIZEBOX, WS_OVERLAPPED,
                WS_SYSMENU, WS_VISIBLE,
            },
        },
    },
};

const SETUP_CLASS: PCWSTR = w!("CodexPetSetupWindowV1");
const WINDOW_WIDTH: i32 = 620;
const WINDOW_HEIGHT: i32 = 430;
const ID_STARTUP: i32 = 1001;
const ID_PRIMARY: i32 = 1002;
const ID_SECONDARY: i32 = 1003;
const ID_BACK: i32 = 1004;
const BM_GETCHECK: u32 = 0x00F0;
const BM_SETCHECK: u32 = 0x00F1;

struct SetupInstance(HANDLE);

impl SetupInstance {
    fn acquire() -> Result<Option<Self>, String> {
        let handle = unsafe { CreateMutexW(None, true, w!("Local\\CodexPetSetup-v1")) }
            .map_err(|_| "unable to create setup instance lock".to_owned())?;
        if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
            let _ = unsafe { CloseHandle(handle) };
            return Ok(None);
        }
        Ok(Some(Self(handle)))
    }
}

impl Drop for SetupInstance {
    fn drop(&mut self) {
        let _ = unsafe { CloseHandle(self.0) };
    }
}

struct SetupContext {
    page: u8,
    title: HWND,
    body: HWND,
    status: HWND,
    startup: HWND,
    primary: HWND,
    secondary: HWND,
    back: HWND,
    baseline_hook_count: u64,
    verification_started_at_ms: u64,
    startup_enabled: bool,
}

#[derive(Deserialize)]
struct RuntimeProbe {
    #[serde(rename = "hookEventCount")]
    hook_event_count: u64,
    #[serde(rename = "lastHookEventAtMs")]
    last_hook_event_at_ms: Option<u64>,
}

pub fn run() -> Result<(), String> {
    let Some(_instance_lock) = SetupInstance::acquire()? else {
        return Ok(());
    };
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
    let instance: HINSTANCE = unsafe { GetModuleHandleW(None) }
        .map_err(|error| format!("unable to get setup module: {error}"))?
        .into();
    let cursor = unsafe { LoadCursorW(None, IDC_ARROW) }
        .map_err(|error| format!("unable to load setup cursor: {error}"))?;
    let class = WNDCLASSW {
        style: CS_HREDRAW | CS_VREDRAW,
        lpfnWndProc: Some(setup_proc),
        hInstance: instance,
        hCursor: cursor,
        lpszClassName: SETUP_CLASS,
        ..Default::default()
    };
    if unsafe { RegisterClassW(&class) } == 0 {
        return Err(format!("unable to register setup window: {:?}", unsafe {
            GetLastError()
        }));
    }
    let x = (unsafe { GetSystemMetrics(SM_CXSCREEN) } - WINDOW_WIDTH) / 2;
    let y = (unsafe { GetSystemMetrics(SM_CYSCREEN) } - WINDOW_HEIGHT) / 2;
    let window = unsafe {
        CreateWindowExW(
            WS_EX_CONTROLPARENT,
            SETUP_CLASS,
            w!("Codex Pet 安装与连接"),
            WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX,
            x,
            y,
            WINDOW_WIDTH,
            WINDOW_HEIGHT,
            None,
            None,
            Some(instance),
            None,
        )
    }
    .map_err(|error| format!("unable to create setup window: {error}"))?;

    let title = create_control(window, w!("STATIC"), "", 28, 24, 550, 42, 0, 0)?;
    let body = create_control(window, w!("STATIC"), "", 28, 78, 550, 118, 0, 0)?;
    let status = create_control(window, w!("STATIC"), "", 28, 208, 550, 62, 0, 0)?;
    let startup = create_control(
        window,
        w!("BUTTON"),
        "随 Windows 登录启动（收到 Codex 任务后显示）",
        28,
        282,
        430,
        28,
        ID_STARTUP,
        BS_AUTOCHECKBOX as u32,
    )?;
    let back = create_control(
        window,
        w!("BUTTON"),
        "返回",
        28,
        338,
        90,
        32,
        ID_BACK,
        BS_PUSHBUTTON as u32,
    )?;
    let secondary = create_control(
        window,
        w!("BUTTON"),
        "取消",
        374,
        338,
        90,
        32,
        ID_SECONDARY,
        BS_PUSHBUTTON as u32,
    )?;
    let primary = create_control(
        window,
        w!("BUTTON"),
        "安装并继续",
        474,
        338,
        104,
        32,
        ID_PRIMARY,
        BS_DEFPUSHBUTTON as u32,
    )?;

    let startup_enabled = settings::startup_enabled_readback().unwrap_or(false);
    if startup_enabled {
        unsafe {
            SendMessageW(
                startup,
                BM_SETCHECK,
                Some(WPARAM(BST_CHECKED.0 as usize)),
                Some(LPARAM(0)),
            );
        }
    }
    let context = Box::new(SetupContext {
        page: 0,
        title,
        body,
        status,
        startup,
        primary,
        secondary,
        back,
        baseline_hook_count: 0,
        verification_started_at_ms: 0,
        startup_enabled,
    });
    let context_ptr = Box::into_raw(context);
    unsafe { SetWindowLongPtrW(window, GWLP_USERDATA, context_ptr as isize) };
    unsafe { render_page(context_ptr) };
    unsafe {
        let _ = ShowWindow(window, SW_SHOW);
        let _ = UpdateWindow(window);
    }

    let mut message = MSG::default();
    loop {
        let result = unsafe { GetMessageW(&mut message, None, 0, 0) }.0;
        if result == -1 {
            unsafe { drop(Box::from_raw(context_ptr)) };
            return Err("setup message loop failed".to_owned());
        }
        if result == 0 {
            break;
        }
        unsafe {
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
    unsafe { drop(Box::from_raw(context_ptr)) };
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn create_control(
    parent: HWND,
    class: PCWSTR,
    text: &str,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    id: i32,
    control_style: u32,
) -> Result<HWND, String> {
    let text = wide(text);
    let control = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            class,
            PCWSTR(text.as_ptr()),
            WS_CHILD
                | WS_VISIBLE
                | windows::Win32::UI::WindowsAndMessaging::WINDOW_STYLE(control_style),
            x,
            y,
            width,
            height,
            Some(parent),
            (id != 0).then_some(HMENU(id as isize as *mut _)),
            None,
            None,
        )
    }
    .map_err(|error| format!("unable to create setup control: {error}"))?;
    let font = unsafe { GetStockObject(DEFAULT_GUI_FONT) };
    unsafe {
        SendMessageW(
            control,
            WM_SETFONT,
            Some(WPARAM(font.0 as usize)),
            Some(LPARAM(1)),
        );
    }
    Ok(control)
}

unsafe extern "system" fn setup_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match message {
        WM_COMMAND => {
            let code = ((wparam.0 >> 16) & 0xffff) as u16;
            if code != BN_CLICKED as u16 {
                return DefWindowProcW(hwnd, message, wparam, lparam);
            }
            let id = (wparam.0 & 0xffff) as i32;
            let context = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut SetupContext;
            if context.is_null() {
                return DefWindowProcW(hwnd, message, wparam, lparam);
            }
            handle_action(hwnd, context, id);
            LRESULT(0)
        }
        WM_CLOSE => {
            let _ = DestroyWindow(hwnd);
            LRESULT(0)
        }
        WM_DESTROY => {
            PostQuitMessage(0);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, message, wparam, lparam),
    }
}

unsafe fn handle_action(hwnd: HWND, context: *mut SetupContext, id: i32) {
    match id {
        ID_PRIMARY if (*context).page == 0 => {
            set_text((*context).status, "正在安装 Codex Hooks…");
            let checked = SendMessageW(
                (*context).startup,
                BM_GETCHECK,
                Some(WPARAM(0)),
                Some(LPARAM(0)),
            )
            .0
                == BST_CHECKED.0 as isize;
            let result = hooks::install_current().and_then(|_| {
                settings::set_startup_enabled(checked)?;
                let mut current = settings::load().unwrap_or_default();
                current.setup_complete = false;
                current.startup_enabled = checked;
                settings::save(&current)
            });
            if let Err(error) = result {
                set_text((*context).status, &format!("安装失败：{error}"));
                return;
            }
            (*context).startup_enabled = checked;
            if let Err(error) = start_pet() {
                set_text((*context).status, &format!("Hooks 已安装，但桌宠启动失败：{error}"));
                return;
            }
            (*context).baseline_hook_count = wait_for_runtime().unwrap_or(0);
            (*context).verification_started_at_ms = now_ms();
            (*context).page = 1;
            render_page(context);
        }
        ID_PRIMARY => match probe_runtime() {
            Ok(probe)
                if probe.hook_event_count > (*context).baseline_hook_count
                    && probe
                        .last_hook_event_at_ms
                        .is_some_and(|time| time >= (*context).verification_started_at_ms) =>
            {
                let mut current = settings::load().unwrap_or_default();
                current.setup_complete = true;
                current.startup_enabled = (*context).startup_enabled;
                current.last_hook_verified_at_ms = probe.last_hook_event_at_ms;
                match settings::save(&current) {
                    Ok(()) => {
                        set_text(
                            (*context).status,
                            "已确认 Hook 传输链可用。仍请在 /hooks 中确认 11 项都已信任。",
                        );
                        let _ = DestroyWindow(hwnd);
                    }
                    Err(error) => set_text((*context).status, &format!("验证成功但保存失败：{error}")),
                }
            }
            Ok(_) => set_text(
                (*context).status,
                "尚未检测到新 Hook。请在 Codex /hooks 信任全部 11 项，重启或新建任务后发送一句消息，再点检测。",
            ),
            Err(error) => set_text((*context).status, &format!("桌宠连接尚未就绪：{error}")),
        },
        ID_SECONDARY if (*context).page == 0 => {
            let _ = DestroyWindow(hwnd);
        }
        ID_SECONDARY => {
            set_text(
                (*context).status,
                "已保留 Hooks 配置；验证尚未完成，可稍后重新运行 CodexPet.exe --setup。",
            );
            let _ = DestroyWindow(hwnd);
        }
        ID_BACK if (*context).page == 1 => {
            (*context).page = 0;
            render_page(context);
        }
        _ => {}
    }
}

unsafe fn render_page(context: *mut SetupContext) {
    if (*context).page == 0 {
        set_text((*context).title, "1 / 2  安装并连接");
        set_text(
            (*context).body,
            "Codex Pet 将为当前用户安装 11 个 Codex Hooks。Hook 只发送 schema 版本、Hook 事件名、Session ID、可选 Turn ID、事件 ID、状态、动作类别和时间，不发送 prompt、转录、工作目录或工具输入输出。修改前会备份 hooks.json。",
        );
        set_text(
            (*context).status,
            "点击“安装并继续”后仍需在 Codex /hooks 中由你亲自授权。",
        );
        set_text((*context).primary, "安装并继续");
        set_text((*context).secondary, "取消");
        let _ = ShowWindow((*context).startup, SW_SHOW);
        let _ = ShowWindow(
            (*context).back,
            windows::Win32::UI::WindowsAndMessaging::SW_HIDE,
        );
    } else {
        set_text((*context).title, "2 / 2  授权并验证");
        set_text(
            (*context).body,
            "请回到 Codex，打开 /hooks，逐项信任全部 11 个 Codex Pet Hooks。然后重启 Desktop 或新建一个任务，发送一句简单消息。Codex Pet 不会替你自动授权。",
        );
        set_text(
            (*context).status,
            "完成授权并产生一个新任务事件后，点击“检测传输”。该检测证明链路收到新事件；11 项信任状态仍以 Codex /hooks 的逐项显示为准。",
        );
        set_text((*context).primary, "检测传输并完成");
        set_text((*context).secondary, "稍后完成");
        let _ = ShowWindow(
            (*context).startup,
            windows::Win32::UI::WindowsAndMessaging::SW_HIDE,
        );
        let _ = ShowWindow((*context).back, SW_SHOW);
    }
}

fn start_pet() -> Result<(), String> {
    let executable =
        std::env::current_exe().map_err(|_| "unable to locate Codex Pet".to_owned())?;
    Command::new(executable)
        .arg("--from-setup")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW.0)
        .spawn()
        .map(|_| ())
        .map_err(|_| "unable to start Codex Pet".to_owned())
}

fn wait_for_runtime() -> Result<u64, String> {
    for _ in 0..50 {
        if let Ok(probe) = probe_runtime() {
            return Ok(probe.hook_event_count);
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    Err("Codex Pet health check timed out".to_owned())
}

fn probe_runtime() -> Result<RuntimeProbe, String> {
    let address: SocketAddr = API_ADDRESS
        .parse()
        .map_err(|_| "invalid Codex Pet endpoint".to_owned())?;
    let mut stream = TcpStream::connect_timeout(&address, Duration::from_millis(300))
        .map_err(|_| "Codex Pet is not running".to_owned())?;
    let _ = stream.set_read_timeout(Some(Duration::from_millis(500)));
    let _ = stream.set_write_timeout(Some(Duration::from_millis(300)));
    let request =
        format!("GET /v1/state HTTP/1.1\r\nHost: {API_ADDRESS}\r\nConnection: close\r\n\r\n");
    stream
        .write_all(request.as_bytes())
        .map_err(|_| "unable to query Codex Pet".to_owned())?;
    let mut response = Vec::new();
    stream
        .read_to_end(&mut response)
        .map_err(|_| "unable to read Codex Pet status".to_owned())?;
    let body = response
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .map(|index| &response[index + 4..])
        .ok_or_else(|| "Codex Pet returned an invalid response".to_owned())?;
    serde_json::from_slice(body).map_err(|_| "Codex Pet returned invalid status JSON".to_owned())
}

unsafe fn set_text(hwnd: HWND, value: &str) {
    let value = wide(value);
    let _ = SetWindowTextW(hwnd, PCWSTR(value.as_ptr()));
}

fn wide(value: &str) -> Vec<u16> {
    OsStr::new(value).encode_wide().chain(Some(0)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_probe_requires_a_fresh_event_after_the_baseline() {
        let baseline = 7;
        let started = 100;
        let stale = RuntimeProbe {
            hook_event_count: 8,
            last_hook_event_at_ms: Some(99),
        };
        let fresh = RuntimeProbe {
            hook_event_count: 8,
            last_hook_event_at_ms: Some(100),
        };
        assert!(
            !(stale.hook_event_count > baseline
                && stale
                    .last_hook_event_at_ms
                    .is_some_and(|time| time >= started))
        );
        assert!(
            fresh.hook_event_count > baseline
                && fresh
                    .last_hook_event_at_ms
                    .is_some_and(|time| time >= started)
        );
    }
}
