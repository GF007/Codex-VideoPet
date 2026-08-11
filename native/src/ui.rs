use crate::{
    metadata, official_pet,
    runtime::{
        RuntimeData, SetWindowRequest, UiSnapshot, WM_APP_EXIT, WM_APP_SHOW, WM_APP_STATE,
        WM_APP_TRAY, WM_APP_WINDOW,
    },
    settings::{self, WindowSizePreset},
    theme::{UiColor, UiTheme},
};
use std::{
    ffi::OsStr, mem::size_of, path::PathBuf, process::Command, ptr, sync::Arc, thread,
    time::Duration,
};
use windows::{
    core::{w, Error, PCWSTR},
    Win32::{
        Foundation::{
            CloseHandle, GetLastError, COLORREF, ERROR_ALREADY_EXISTS, HANDLE, HINSTANCE, HWND,
            LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM,
        },
        Graphics::{
            Direct2D::{
                Common::{
                    D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_COLOR_F, D2D1_GRADIENT_STOP,
                    D2D1_PIXEL_FORMAT, D2D_RECT_F,
                },
                D2D1CreateFactory, ID2D1DCRenderTarget, ID2D1Factory, ID2D1LinearGradientBrush,
                ID2D1SolidColorBrush, D2D1_DRAW_TEXT_OPTIONS_CLIP, D2D1_EXTEND_MODE_CLAMP,
                D2D1_FACTORY_TYPE_SINGLE_THREADED, D2D1_FEATURE_LEVEL_DEFAULT, D2D1_GAMMA_2_2,
                D2D1_LINEAR_GRADIENT_BRUSH_PROPERTIES, D2D1_RENDER_TARGET_PROPERTIES,
                D2D1_RENDER_TARGET_TYPE_DEFAULT, D2D1_RENDER_TARGET_USAGE_NONE, D2D1_ROUNDED_RECT,
            },
            DirectWrite::{
                DWriteCreateFactory, IDWriteFactory, IDWriteFontCollection, IDWriteTextFormat,
                DWRITE_FACTORY_TYPE_SHARED, DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_NORMAL,
                DWRITE_FONT_WEIGHT_BOLD, DWRITE_FONT_WEIGHT_EXTRA_BOLD, DWRITE_FONT_WEIGHT_NORMAL,
                DWRITE_MEASURING_MODE_NATURAL, DWRITE_TEXT_ALIGNMENT_LEADING, DWRITE_TRIMMING,
                DWRITE_TRIMMING_GRANULARITY_CHARACTER, DWRITE_WORD_WRAPPING_NO_WRAP,
            },
            Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM,
            Gdi::{
                BeginPaint, CreateCompatibleDC, CreateDIBSection, CreateRoundRectRgn, DeleteDC,
                DeleteObject, EndPaint, GetMonitorInfoW, MonitorFromPoint, MonitorFromWindow,
                ScreenToClient, SelectObject, SetWindowRgn, AC_SRC_ALPHA, AC_SRC_OVER, BITMAPINFO,
                BITMAPINFOHEADER, BI_RGB, BLENDFUNCTION, DIB_RGB_COLORS, HBITMAP, HDC, HGDIOBJ,
                MONITORINFO, MONITOR_DEFAULTTONEAREST, MONITOR_DEFAULTTOPRIMARY, PAINTSTRUCT,
            },
        },
        System::{
            Com::{CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED},
            LibraryLoader::GetModuleHandleW,
            Threading::CreateMutexW,
        },
        UI::{
            HiDpi::{SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2},
            Input::KeyboardAndMouse::{
                EnableWindow, TrackMouseEvent, TME_LEAVE, TME_NONCLIENT, TRACKMOUSEEVENT,
            },
            Shell::{
                Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE,
                NOTIFYICONDATAW,
            },
            WindowsAndMessaging::{
                AppendMenuW, CheckMenuItem, CreatePopupMenu, CreateWindowExW, DefWindowProcW,
                DestroyMenu, DestroyWindow, DispatchMessageW, FindWindowW, GetClientRect,
                GetCursorPos, GetMessageW, GetWindowLongPtrW, GetWindowRect, LoadCursorW,
                LoadIconW, PostMessageW, PostQuitMessage, RegisterClassW, SetForegroundWindow,
                SetWindowLongPtrW, SetWindowPos, ShowWindow, TrackPopupMenu, TranslateMessage,
                UpdateLayeredWindow, CS_HREDRAW, CS_VREDRAW, CW_USEDEFAULT, GWLP_USERDATA,
                GWL_EXSTYLE, HTCAPTION, HTCLIENT, HTTRANSPARENT, HWND_NOTOPMOST, HWND_TOPMOST,
                IDC_ARROW, IDI_APPLICATION, MA_NOACTIVATE, MF_BYCOMMAND, MF_CHECKED, MF_SEPARATOR,
                MF_STRING, MF_UNCHECKED, MSG, SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE,
                SWP_NOOWNERZORDER, SWP_NOSIZE, SW_HIDE, SW_SHOWNA, TPM_BOTTOMALIGN, TPM_LEFTALIGN,
                TPM_RETURNCMD, TPM_RIGHTBUTTON, ULW_ALPHA, UPDATE_LAYERED_WINDOW_FLAGS, WM_CLOSE,
                WM_COMMAND, WM_DESTROY, WM_DPICHANGED, WM_ENTERSIZEMOVE, WM_ERASEBKGND,
                WM_EXITSIZEMOVE, WM_LBUTTONUP, WM_MOUSEACTIVATE, WM_MOUSEWHEEL, WM_MOVE, WM_MOVING,
                WM_NCHITTEST, WM_NCLBUTTONDOWN, WM_NCMOUSELEAVE, WM_NCMOUSEMOVE, WM_PAINT,
                WM_RBUTTONUP, WM_SIZE, WNDCLASSW, WS_EX_LAYERED, WS_EX_NOACTIVATE,
                WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
            },
        },
    },
};

const VIDEO_CLASS: PCWSTR = w!("CodexPetNativeVideo");
const INFO_CLASS: PCWSTR = w!("CodexPetNativeInfo");
const MUTEX_NAME: PCWSTR = w!("Local\\CodexPetNativeOverlay-v2");
const VISUAL_SCALE: f32 = 1.5;
const MIN_WIDTH: i32 = 270;
const MIN_HEIGHT: i32 = 450;
const MENU_SHOW: u16 = 1001;
const MENU_CLICK_THROUGH: u16 = 1002;
const MENU_TOPMOST: u16 = 1003;
const MENU_QUIT: u16 = 1004;
const MENU_SIZE_SMALL: u16 = 1005;
const MENU_SIZE_MEDIUM: u16 = 1006;
const MENU_SIZE_LARGE: u16 = 1007;
const TRAY_ID: u32 = 1;
const VISIBLE_SESSION_ROWS: usize = 3;
const PANEL_MARGIN: f32 = 10.0;
const PANEL_GAP: f32 = 8.0;
const SESSION_ROW_PITCH: f32 = 47.0;
const SESSION_MAX_HEIGHT: f32 = VISIBLE_SESSION_ROWS as f32 * SESSION_ROW_PITCH;
const STATUS_PANEL_HEIGHT: f32 = 91.0;
const STATUS_PROGRESS_GAP: f32 = 5.0;
const STATUS_PROGRESS_HEIGHT: f32 = 4.0;
const INFO_TOP_EXTENT: f32 = PANEL_MARGIN + SESSION_MAX_HEIGHT + PANEL_GAP;
const INFO_BOTTOM_EXTENT: f32 =
    PANEL_GAP + STATUS_PANEL_HEIGHT + STATUS_PROGRESS_GAP + STATUS_PROGRESS_HEIGHT + PANEL_MARGIN;

pub struct SingleInstance(HANDLE);

impl SingleInstance {
    pub fn acquire() -> Result<Option<Self>, String> {
        let handle = unsafe { CreateMutexW(None, true, MUTEX_NAME) }
            .map_err(|error| format!("unable to create single-instance mutex: {error}"))?;
        if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
            if let Ok(hwnd) = unsafe { FindWindowW(VIDEO_CLASS, None) } {
                if !hwnd.is_invalid() {
                    let _ = unsafe { PostMessageW(Some(hwnd), WM_APP_SHOW, WPARAM(0), LPARAM(0)) };
                }
            }
            let _ = unsafe { CloseHandle(handle) };
            Ok(None)
        } else {
            Ok(Some(Self(handle)))
        }
    }
}

impl Drop for SingleInstance {
    fn drop(&mut self) {
        let _ = unsafe { CloseHandle(self.0) };
    }
}

pub fn request_existing_exit(timeout: Duration) -> Result<bool, String> {
    let Ok(hwnd) = (unsafe { FindWindowW(VIDEO_CLASS, None) }) else {
        return Ok(false);
    };
    if hwnd.is_invalid() {
        return Ok(false);
    }
    unsafe { PostMessageW(Some(hwnd), WM_APP_EXIT, WPARAM(0), LPARAM(0)) }
        .map_err(|error| format!("unable to request Codex Pet shutdown: {error}"))?;
    let deadline = std::time::Instant::now() + timeout;
    while std::time::Instant::now() < deadline {
        let current = unsafe { FindWindowW(VIDEO_CLASS, None) };
        if current.map_or(true, |window| window.is_invalid()) {
            return Ok(true);
        }
        thread::sleep(Duration::from_millis(50));
    }
    Err("Codex Pet did not exit within the shutdown timeout".to_owned())
}

struct Surface {
    dc: HDC,
    bitmap: HBITMAP,
    previous: HGDIOBJ,
    bits: *mut u8,
    target: ID2D1DCRenderTarget,
    width: i32,
    height: i32,
}

impl Drop for Surface {
    fn drop(&mut self) {
        unsafe {
            SelectObject(self.dc, self.previous);
            let _ = DeleteObject(HGDIOBJ(self.bitmap.0));
            let _ = DeleteDC(self.dc);
        }
    }
}

struct SessionHit {
    rect: RECT,
    session_id: String,
    thread_id: String,
    revision: u64,
}

#[derive(Clone, Copy, Debug)]
struct VerticalLayout {
    video_top: f32,
    tray_top: f32,
    status_top: f32,
    status_bottom: f32,
    progress_top: f32,
    progress_bottom: f32,
}

fn vertical_layout(layout_height: f32, tray_height: f32) -> VerticalLayout {
    let video_top = INFO_TOP_EXTENT;
    let video_bottom = layout_height - INFO_BOTTOM_EXTENT;
    let tray_bottom = video_top - PANEL_GAP;
    let status_top = video_bottom + PANEL_GAP;
    let status_bottom = status_top + STATUS_PANEL_HEIGHT;
    let progress_top = status_bottom + STATUS_PROGRESS_GAP;
    VerticalLayout {
        video_top,
        tray_top: tray_bottom - tray_height,
        status_top,
        status_bottom,
        progress_top,
        progress_bottom: progress_top + STATUS_PROGRESS_HEIGHT,
    }
}

fn layout_to_pixels(value: f32) -> i32 {
    (value * VISUAL_SCALE).round() as i32
}

fn preset_dimensions(preset: WindowSizePreset) -> (i32, i32) {
    match preset {
        WindowSizePreset::Small => (288, 512),
        WindowSizePreset::Medium => (360, 640),
        WindowSizePreset::Large => (450, 800),
    }
}

fn preset_window_radius(preset: WindowSizePreset) -> i32 {
    match preset {
        WindowSizePreset::Small => 52,
        WindowSizePreset::Medium => 64,
        WindowSizePreset::Large => 80,
    }
}

fn video_position_in_work_area(work_area: RECT, width: i32) -> (i32, i32) {
    let margin = layout_to_pixels(PANEL_MARGIN);
    let top_extent = layout_to_pixels(INFO_TOP_EXTENT);
    let x = (work_area.right - width - margin).max(work_area.left);
    let y = work_area.top + top_extent + margin;
    (x, y)
}

fn initial_video_position(width: i32) -> (i32, i32) {
    let mut cursor = POINT::default();
    let _ = unsafe { GetCursorPos(&mut cursor) };
    let monitor = unsafe { MonitorFromPoint(cursor, MONITOR_DEFAULTTOPRIMARY) };
    let mut monitor_info = MONITORINFO {
        cbSize: size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    if unsafe { GetMonitorInfoW(monitor, &mut monitor_info) }.as_bool() {
        video_position_in_work_area(monitor_info.rcWork, width)
    } else {
        (CW_USEDEFAULT, CW_USEDEFAULT)
    }
}

struct Renderer {
    d2d: ID2D1Factory,
    kicker: IDWriteTextFormat,
    state: IDWriteTextFormat,
    detail: IDWriteTextFormat,
    title: IDWriteTextFormat,
    small: IDWriteTextFormat,
    surface: Option<Surface>,
    hits: Vec<SessionHit>,
    session_offset: usize,
}

impl Renderer {
    fn new() -> Result<Self, String> {
        let d2d: ID2D1Factory =
            unsafe { D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None) }
                .map_err(win_error)?;
        let dwrite: IDWriteFactory =
            unsafe { DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED) }.map_err(win_error)?;
        let kicker = create_format(&dwrite, 9.0, DWRITE_FONT_WEIGHT_BOLD)?;
        let state = create_format(&dwrite, 30.0, DWRITE_FONT_WEIGHT_EXTRA_BOLD)?;
        let detail = create_format(&dwrite, 10.0, DWRITE_FONT_WEIGHT_NORMAL)?;
        let title = create_format(&dwrite, 11.0, DWRITE_FONT_WEIGHT_BOLD)?;
        let small = create_format(&dwrite, 9.0, DWRITE_FONT_WEIGHT_NORMAL)?;
        Ok(Self {
            d2d,
            kicker,
            state,
            detail,
            title,
            small,
            surface: None,
            hits: Vec::new(),
            session_offset: 0,
        })
    }

    fn render(&mut self, hwnd: HWND, snapshot: &UiSnapshot) -> Result<(), String> {
        let mut client = RECT::default();
        unsafe { GetClientRect(hwnd, &mut client) }.map_err(win_error)?;
        let width = (client.right - client.left).max(1);
        let height = (client.bottom - client.top).max(1);
        let layout_width = width as f32 / VISUAL_SCALE;
        let layout_height = height as f32 / VISUAL_SCALE;
        if self
            .surface
            .as_ref()
            .is_none_or(|surface| surface.width != width || surface.height != height)
        {
            self.surface = Some(create_surface(&self.d2d, width, height)?);
        }
        let surface = self.surface.as_mut().expect("surface initialized");
        let rect = RECT {
            left: 0,
            top: 0,
            right: width,
            bottom: height,
        };
        unsafe { surface.target.BindDC(surface.dc, &rect) }.map_err(win_error)?;
        unsafe {
            surface.target.BeginDraw();
            surface.target.Clear(Some(&D2D1_COLOR_F::default()));
        }

        let border = brush(&surface.target, theme_color(snapshot.theme.accent, 0.26))?;
        let white = brush(&surface.target, theme_color(snapshot.theme.light, 1.0))?;
        let muted = brush(&surface.target, theme_color(snapshot.theme.light, 0.68))?;
        let subtle = brush(&surface.target, theme_color(snapshot.theme.light, 0.50))?;
        let accent = brush(
            &surface.target,
            state_color(&snapshot.state.state, &snapshot.theme),
        )?;
        self.hits.clear();
        self.session_offset =
            clamp_session_offset(self.session_offset, snapshot.sessions.sessions.len());
        let visible_sessions = snapshot
            .sessions
            .sessions
            .iter()
            .skip(self.session_offset)
            .take(VISIBLE_SESSION_ROWS)
            .collect::<Vec<_>>();
        let tray_height = visible_sessions.len() as f32 * SESSION_ROW_PITCH;
        let vertical = vertical_layout(layout_height, tray_height);
        let status_rect = rounded(
            12.0,
            vertical.status_top,
            layout_width - 12.0,
            vertical.status_bottom,
            15.0,
        );
        let panel_start = scale_color(snapshot.theme.base, 0.72);
        let panel_end = mix_color(panel_start, snapshot.theme.accent, 0.28);
        let panel = gradient_brush(
            &surface.target,
            theme_color(panel_start, 0.92),
            theme_color(panel_end, 0.88),
            12.0,
            vertical.status_top,
            layout_width - 12.0,
            vertical.status_bottom,
        )?;
        unsafe {
            surface.target.FillRoundedRectangle(&status_rect, &panel);
            surface
                .target
                .DrawRoundedRectangle(&status_rect, &border, 1.0, None);
        }
        draw_text(
            &surface.target,
            "CURRENT STATE",
            &self.kicker,
            &muted,
            24.0,
            vertical.status_top + 10.0,
            layout_width - 24.0,
            vertical.status_top + 24.0,
        );
        draw_text(
            &surface.target,
            &snapshot.state.state.to_uppercase(),
            &self.state,
            &accent,
            24.0,
            vertical.status_top + 22.0,
            layout_width - 24.0,
            vertical.status_top + 60.0,
        );
        let detail = snapshot
            .session_error
            .as_ref()
            .map(|error| format!("跳转失败：{error}"))
            .unwrap_or_else(|| selected_detail(snapshot));
        let detail_brush = if snapshot.session_error.is_some() {
            brush(&surface.target, color(255, 114, 133, 1.0))?
        } else {
            muted.clone()
        };
        draw_text(
            &surface.target,
            &detail,
            &self.detail,
            &detail_brush,
            24.0,
            vertical.status_top + 67.0,
            layout_width - 24.0,
            vertical.status_top + 86.0,
        );

        let tray_top = vertical.tray_top;
        for (index, session) in visible_sessions.into_iter().enumerate() {
            let top = tray_top + index as f32 * SESSION_ROW_PITCH;
            let selected =
                snapshot.sessions.selected_session_id.as_deref() == Some(session.id.as_str());
            let card_start = mix_color(
                scale_color(snapshot.theme.base, 0.82),
                snapshot.theme.light,
                if selected { 0.08 } else { 0.04 },
            );
            let card_end = mix_color(
                scale_color(snapshot.theme.base, 0.82),
                snapshot.theme.accent,
                if selected { 0.58 } else { 0.28 },
            );
            let card_brush = gradient_brush(
                &surface.target,
                theme_color(card_start, if selected { 0.92 } else { 0.82 }),
                theme_color(card_end, if selected { 0.90 } else { 0.78 }),
                16.0,
                top,
                layout_width - 16.0,
                top + 42.0,
            )?;
            let card = rounded(16.0, top, layout_width - 16.0, top + 42.0, 10.0);
            unsafe { surface.target.FillRoundedRectangle(&card, &card_brush) };
            let title_left = if session.unread { 36.0 } else { 24.0 };
            if session.unread {
                let review = brush(&surface.target, state_color("review", &snapshot.theme))?;
                draw_text(
                    &surface.target,
                    "●",
                    &self.small,
                    &review,
                    23.0,
                    top + 6.0,
                    34.0,
                    top + 22.0,
                );
            }
            draw_text(
                &surface.target,
                &session.title,
                &self.title,
                &white,
                title_left,
                top + 6.0,
                layout_width - 90.0,
                top + 22.0,
            );
            let session_accent = brush(
                &surface.target,
                state_color(&session.state, &snapshot.theme),
            )?;
            draw_text(
                &surface.target,
                &session.state.to_uppercase(),
                &self.kicker,
                &session_accent,
                layout_width - 84.0,
                top + 7.0,
                layout_width - 22.0,
                top + 21.0,
            );
            let session_detail = if session.target_thread_id.is_none() {
                "正在确认主体任务"
            } else if session.unread {
                "未读完成，点击查看"
            } else {
                activity_label(&session.activity)
            };
            draw_text(
                &surface.target,
                session_detail,
                &self.small,
                &subtle,
                24.0,
                top + 23.0,
                layout_width - 22.0,
                top + 38.0,
            );
            if let Some(thread_id) = session.target_thread_id.clone() {
                self.hits.push(SessionHit {
                    rect: RECT {
                        left: (16.0 * VISUAL_SCALE) as i32,
                        top: (top * VISUAL_SCALE) as i32,
                        right: ((layout_width - 16.0) * VISUAL_SCALE) as i32,
                        bottom: ((top + 42.0) * VISUAL_SCALE) as i32,
                    },
                    session_id: session.id.clone(),
                    thread_id,
                    revision: session.revision,
                });
            }
        }

        let progress = if snapshot.playback.error.is_some() || snapshot.playback.playing {
            1.0
        } else if snapshot.playback.ready {
            0.66
        } else {
            0.32
        };
        let progress_back = brush(&surface.target, theme_color(snapshot.theme.light, 0.14))?;
        let progress_rect = rounded(
            18.0,
            vertical.progress_top,
            layout_width - 18.0,
            vertical.progress_bottom,
            2.0,
        );
        let progress_fill = rounded(
            18.0,
            vertical.progress_top,
            18.0 + (layout_width - 36.0) * progress,
            vertical.progress_bottom,
            2.0,
        );
        unsafe {
            surface
                .target
                .FillRoundedRectangle(&progress_rect, &progress_back);
            surface.target.FillRoundedRectangle(&progress_fill, &accent);
        }

        if let Some(error) = snapshot.playback.error.as_ref() {
            let fatal = brush(&surface.target, color(28, 6, 12, 0.93))?;
            let fatal_top = vertical.video_top + 10.0;
            let fatal_rect = rounded(10.0, fatal_top, layout_width - 10.0, fatal_top + 70.0, 16.0);
            unsafe { surface.target.FillRoundedRectangle(&fatal_rect, &fatal) };
            draw_text(
                &surface.target,
                error,
                &self.detail,
                &white,
                20.0,
                fatal_top + 16.0,
                layout_width - 20.0,
                fatal_top + 60.0,
            );
        }

        unsafe { surface.target.EndDraw(None, None) }.map_err(win_error)?;
        let byte_len = (surface.width * surface.height * 4).max(0) as usize;
        let pixels = unsafe { std::slice::from_raw_parts(surface.bits, byte_len) };
        let lower_start = ((surface.height / 2) * surface.width * 4).max(0) as usize;
        let lower_alpha = pixels[lower_start..]
            .chunks_exact(4)
            .filter(|pixel| pixel[3] != 0)
            .count();
        if lower_alpha == 0 {
            return Err("Direct2D overlay lower half rendered fully transparent".to_owned());
        }
        update_layered(hwnd, surface)
    }

    fn hit_session(&self, point: POINT) -> Option<(String, String, u64)> {
        self.hits
            .iter()
            .find(|hit| {
                point.x >= hit.rect.left
                    && point.x < hit.rect.right
                    && point.y >= hit.rect.top
                    && point.y < hit.rect.bottom
            })
            .map(|hit| (hit.session_id.clone(), hit.thread_id.clone(), hit.revision))
    }

    fn scroll_sessions(&mut self, wheel_delta: i32, total: usize) -> bool {
        let next = scroll_session_offset(self.session_offset, total, wheel_delta);
        if next == self.session_offset {
            return false;
        }
        self.session_offset = next;
        true
    }
}

struct AppContext {
    runtime: Arc<RuntimeData>,
    renderer: Renderer,
    video_hwnd: HWND,
    info_hwnd: HWND,
    tray: NOTIFYICONDATAW,
    size_preset: WindowSizePreset,
    interaction: InteractionState,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum InteractionKind {
    Hover,
    Drag,
}

struct ActiveTransient {
    kind: InteractionKind,
    state: &'static str,
    base_state: String,
    revision: u64,
    sessions_revision: u64,
}

#[derive(Default)]
struct InteractionState {
    hover_tracking: bool,
    drag_active: bool,
    drag_last_x: Option<i32>,
    drag_direction: Option<&'static str>,
    active: Option<ActiveTransient>,
}

impl InteractionState {
    fn begin_drag(&mut self, x: i32) {
        self.drag_active = true;
        self.drag_last_x = Some(x);
        self.drag_direction = None;
    }

    fn update_drag(&mut self, x: i32) -> Option<&'static str> {
        if !self.drag_active {
            return None;
        }
        let previous_x = self.drag_last_x.get_or_insert(x);
        let direction = official_pet::drag_state(x - *previous_x)?;
        *previous_x = x;
        if self.drag_direction == Some(direction) {
            return None;
        }
        self.drag_direction = Some(direction);
        Some(direction)
    }

    fn end_drag(&mut self) {
        self.drag_active = false;
        self.drag_last_x = None;
        self.drag_direction = None;
    }
}

pub fn run(runtime: Arc<RuntimeData>) -> Result<(), String> {
    unsafe {
        SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2)
            .map_err(win_error)?;
        CoInitializeEx(None, COINIT_APARTMENTTHREADED)
            .ok()
            .map_err(win_error)?;
    }
    let result = run_inner(runtime);
    unsafe { CoUninitialize() };
    result
}

fn run_inner(runtime: Arc<RuntimeData>) -> Result<(), String> {
    let size_preset = settings::load()
        .map(|settings| settings.window_size_preset)
        .unwrap_or_default();
    let (video_width, video_height) = preset_dimensions(size_preset);
    let instance: HINSTANCE = unsafe { GetModuleHandleW(None) }.map_err(win_error)?.into();
    let cursor = unsafe { LoadCursorW(None, IDC_ARROW) }.map_err(win_error)?;
    let icon = unsafe { LoadIconW(None, IDI_APPLICATION) }.map_err(win_error)?;
    let video_class = WNDCLASSW {
        style: CS_HREDRAW | CS_VREDRAW,
        lpfnWndProc: Some(video_proc),
        hInstance: instance,
        hCursor: cursor,
        hIcon: icon,
        lpszClassName: VIDEO_CLASS,
        ..Default::default()
    };
    let info_class = WNDCLASSW {
        style: CS_HREDRAW | CS_VREDRAW,
        lpfnWndProc: Some(info_proc),
        hInstance: instance,
        hCursor: cursor,
        lpszClassName: INFO_CLASS,
        ..Default::default()
    };
    if unsafe { RegisterClassW(&video_class) } == 0 || unsafe { RegisterClassW(&info_class) } == 0 {
        return Err(format!("unable to register Win32 classes: {:?}", unsafe {
            GetLastError()
        }));
    }

    let (video_x, video_y) = initial_video_position(video_width);
    let video_hwnd = unsafe {
        CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_NOACTIVATE,
            VIDEO_CLASS,
            w!("Codex Pet Overlay"),
            WS_POPUP,
            video_x,
            video_y,
            video_width,
            video_height,
            None,
            None,
            Some(instance),
            None,
        )
    }
    .map_err(|error| format!("unable to create video host window: {error}"))?;
    let info_hwnd = unsafe {
        CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_NOACTIVATE,
            INFO_CLASS,
            w!("Codex Pet Information"),
            WS_POPUP,
            0,
            0,
            video_width,
            video_height + layout_to_pixels(INFO_TOP_EXTENT + INFO_BOTTOM_EXTENT),
            Some(video_hwnd),
            None,
            Some(instance),
            None,
        )
    }
    .map_err(|error| format!("unable to create information window: {error}"))?;
    runtime.register_windows(video_hwnd.0 as isize, info_hwnd.0 as isize);

    let mut tray = NOTIFYICONDATAW {
        cbSize: size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: video_hwnd,
        uID: TRAY_ID,
        uFlags: NIF_MESSAGE | NIF_ICON | NIF_TIP,
        uCallbackMessage: WM_APP_TRAY,
        hIcon: icon,
        ..Default::default()
    };
    copy_wide_fixed("Codex Pet Overlay", &mut tray.szTip);
    if !unsafe { Shell_NotifyIconW(NIM_ADD, &tray) }.as_bool() {
        return Err("unable to create tray icon".to_owned());
    }

    let context = Box::new(AppContext {
        runtime: runtime.clone(),
        renderer: Renderer::new()?,
        video_hwnd,
        info_hwnd,
        tray,
        size_preset,
        interaction: InteractionState::default(),
    });
    let context_ptr = Box::into_raw(context);
    unsafe {
        SetWindowLongPtrW(video_hwnd, GWLP_USERDATA, context_ptr as isize);
        SetWindowLongPtrW(info_hwnd, GWLP_USERDATA, context_ptr as isize);
    }
    if !official_pet::is_attention_state(&runtime.selected_base_state()?) {
        runtime.transition_state(
            "waving",
            "official:first-awake",
            Some(official_pet::FIRST_AWAKE_DURATION_MS),
        )?;
    }
    unsafe {
        sync_overlay(context_ptr);
        apply_window_flags(context_ptr);
    }
    runtime.attach_player(player_release_after(), video_width, video_height)?;
    runtime.notify_state();

    let mut message = MSG::default();
    let mut message_error = None;
    loop {
        let message_result = unsafe { GetMessageW(&mut message, None, 0, 0) }.0;
        if message_result == -1 {
            message_error = Some(format!("GetMessageW failed: {:?}", unsafe {
                GetLastError()
            }));
            break;
        }
        if message_result == 0 {
            break;
        }
        unsafe {
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }

    runtime.stop();
    unsafe {
        SetWindowLongPtrW(video_hwnd, GWLP_USERDATA, 0);
        SetWindowLongPtrW(info_hwnd, GWLP_USERDATA, 0);
        drop(Box::from_raw(context_ptr));
    }
    message_error.map_or(Ok(()), Err)
}

unsafe extern "system" fn video_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    let context = context(hwnd);
    match message {
        WM_CLOSE => {
            if let Some(context) = context {
                let _ = context.runtime.update_window(SetWindowRequest {
                    visible: Some(false),
                    click_through: None,
                    always_on_top: None,
                });
            }
            LRESULT(0)
        }
        WM_DESTROY => {
            if let Some(context) = context {
                context.runtime.stop();
                let _ = Shell_NotifyIconW(NIM_DELETE, &context.tray);
            }
            PostQuitMessage(0);
            LRESULT(0)
        }
        WM_NCHITTEST => {
            if let Some(context) = context {
                if context
                    .runtime
                    .ui_snapshot()
                    .is_some_and(|snapshot| snapshot.window.click_through)
                {
                    return LRESULT(HTTRANSPARENT as isize);
                }
            }
            LRESULT(HTCAPTION as isize)
        }
        WM_NCMOUSEMOVE => {
            if let Some(context) = context {
                if interaction_allowed(context)
                    && !context.interaction.hover_tracking
                    && !context.interaction.drag_active
                {
                    let mut tracking = TRACKMOUSEEVENT {
                        cbSize: size_of::<TRACKMOUSEEVENT>() as u32,
                        dwFlags: TME_LEAVE | TME_NONCLIENT,
                        hwndTrack: hwnd,
                        dwHoverTime: 0,
                    };
                    if TrackMouseEvent(&mut tracking).is_ok() {
                        context.interaction.hover_tracking = true;
                        start_interaction_transient(context, InteractionKind::Hover, "jumping");
                    }
                }
            }
            DefWindowProcW(hwnd, message, wparam, lparam)
        }
        WM_NCMOUSELEAVE => {
            if let Some(context) = context {
                context.interaction.hover_tracking = false;
                end_interaction_transient(context, Some(InteractionKind::Hover));
            }
            LRESULT(0)
        }
        WM_NCLBUTTONDOWN => {
            if let Some(context) = context {
                if !interaction_allowed(context) {
                    reset_interaction(context);
                    return LRESULT(0);
                }
                if wparam.0 as u32 == HTCAPTION {
                    let mut rect = RECT::default();
                    if GetWindowRect(hwnd, &mut rect).is_ok() {
                        context.interaction.begin_drag(rect.left);
                    }
                }
            }
            DefWindowProcW(hwnd, message, wparam, lparam)
        }
        WM_ENTERSIZEMOVE => {
            if let Some(context) = context {
                if !interaction_allowed(context) {
                    reset_interaction(context);
                    return LRESULT(0);
                }
                if !context.interaction.drag_active {
                    let mut rect = RECT::default();
                    if GetWindowRect(hwnd, &mut rect).is_ok() {
                        context.interaction.begin_drag(rect.left);
                    }
                }
            }
            DefWindowProcW(hwnd, message, wparam, lparam)
        }
        WM_MOVING => {
            if let Some(context) = context {
                if interaction_allowed(context) {
                    if let Some(rect) = (lparam.0 as *const RECT).as_ref() {
                        if let Some(state) = context.interaction.update_drag(rect.left) {
                            start_interaction_transient(context, InteractionKind::Drag, state);
                        }
                    }
                } else {
                    reset_interaction(context);
                    return LRESULT(1);
                }
            }
            DefWindowProcW(hwnd, message, wparam, lparam)
        }
        WM_EXITSIZEMOVE => {
            if let Some(context) = context {
                context.interaction.end_drag();
                end_interaction_transient(context, Some(InteractionKind::Drag));
                context.interaction.hover_tracking = false;
            }
            DefWindowProcW(hwnd, message, wparam, lparam)
        }
        WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
        WM_MOVE | WM_SIZE | WM_DPICHANGED => {
            if let Some(context) = context {
                let ptr = context as *mut AppContext;
                sync_overlay(ptr);
            }
            DefWindowProcW(hwnd, message, wparam, lparam)
        }
        WM_APP_WINDOW => {
            if let Some(context) = context {
                apply_window_flags(context as *mut AppContext);
            }
            LRESULT(0)
        }
        WM_APP_SHOW => {
            if let Some(context) = context {
                let _ = context.runtime.update_window(SetWindowRequest {
                    visible: Some(true),
                    click_through: None,
                    always_on_top: None,
                });
            }
            LRESULT(0)
        }
        WM_APP_EXIT => {
            let _ = DestroyWindow(hwnd);
            LRESULT(0)
        }
        WM_APP_TRAY => {
            if let Some(context) = context {
                handle_tray(context as *mut AppContext, lparam.0 as u32);
            }
            LRESULT(0)
        }
        WM_COMMAND => {
            if let Some(context) = context {
                handle_menu(context as *mut AppContext, (wparam.0 & 0xffff) as u16);
            }
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, message, wparam, lparam),
    }
}

unsafe extern "system" fn info_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    let context = context(hwnd);
    match message {
        WM_ERASEBKGND => LRESULT(1),
        WM_PAINT => {
            let mut paint = PAINTSTRUCT::default();
            BeginPaint(hwnd, &mut paint);
            if let Some(context) = context {
                render_info(context);
            }
            let _ = EndPaint(hwnd, &paint);
            LRESULT(0)
        }
        WM_APP_STATE => {
            if let Some(context) = context {
                render_info(context);
            }
            LRESULT(0)
        }
        WM_NCHITTEST => {
            if let Some(context) = context {
                if context
                    .runtime
                    .ui_snapshot()
                    .is_some_and(|snapshot| snapshot.window.click_through)
                {
                    return LRESULT(HTTRANSPARENT as isize);
                }
                let mut point = point_from_lparam(lparam);
                let _ = ScreenToClient(hwnd, &mut point);
                if context.renderer.hit_session(point).is_some() {
                    return LRESULT(HTCLIENT as isize);
                }
            }
            LRESULT(HTTRANSPARENT as isize)
        }
        WM_LBUTTONUP => {
            if let Some(context) = context {
                if interaction_allowed(context) {
                    let point = POINT {
                        x: signed_low_word(lparam.0),
                        y: signed_high_word(lparam.0),
                    };
                    if let Some((session_id, thread_id, revision)) =
                        context.renderer.hit_session(point)
                    {
                        if let Err(error) =
                            open_session(&context.runtime, &session_id, &thread_id, revision)
                        {
                            context.runtime.set_session_error(Some(error));
                        }
                    }
                }
            }
            LRESULT(0)
        }
        WM_MOUSEWHEEL => {
            if let Some(context) = context {
                if interaction_allowed(context) {
                    let mut point = point_from_lparam(lparam);
                    let _ = ScreenToClient(hwnd, &mut point);
                    let total = context
                        .runtime
                        .ui_snapshot()
                        .map(|snapshot| snapshot.sessions.session_count)
                        .unwrap_or(0);
                    if context.renderer.hit_session(point).is_some()
                        && context
                            .renderer
                            .scroll_sessions(signed_high_word(wparam.0 as isize), total)
                    {
                        render_info(context);
                    }
                }
            }
            LRESULT(0)
        }
        WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
        _ => DefWindowProcW(hwnd, message, wparam, lparam),
    }
}

unsafe fn context(hwnd: HWND) -> Option<&'static mut AppContext> {
    let pointer = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut AppContext;
    pointer.as_mut()
}

unsafe fn sync_overlay(context: *mut AppContext) {
    let Some(context) = context.as_mut() else {
        return;
    };
    let mut rect = RECT::default();
    if GetWindowRect(context.video_hwnd, &mut rect).is_err() {
        return;
    }
    let width = (rect.right - rect.left).max(MIN_WIDTH);
    let height = (rect.bottom - rect.top).max(MIN_HEIGHT);
    let info_top_extent = layout_to_pixels(INFO_TOP_EXTENT);
    let info_bottom_extent = layout_to_pixels(INFO_BOTTOM_EXTENT);
    let insert_after = context
        .runtime
        .ui_snapshot()
        .map(|snapshot| {
            if snapshot.window.always_on_top {
                HWND_TOPMOST
            } else {
                HWND_NOTOPMOST
            }
        })
        .unwrap_or(HWND_TOPMOST);
    let _ = SetWindowPos(
        context.info_hwnd,
        Some(insert_after),
        rect.left,
        rect.top - info_top_extent,
        width,
        height + info_top_extent + info_bottom_extent,
        SWP_NOACTIVATE | SWP_NOOWNERZORDER,
    );
    let radius = preset_window_radius(context.size_preset);
    let region = CreateRoundRectRgn(0, 0, width + 1, height + 1, radius, radius);
    if !region.is_invalid() && SetWindowRgn(context.video_hwnd, Some(region), true) == 0 {
        let _ = DeleteObject(HGDIOBJ(region.0));
    }
    context.renderer.surface = None;
    render_info(context);
}

fn render_info(context: &mut AppContext) {
    let Some(snapshot) = context.runtime.ui_snapshot() else {
        return;
    };
    match context.renderer.render(context.info_hwnd, &snapshot) {
        Ok(()) => context.runtime.set_ui_error(None),
        Err(error) => context.runtime.set_ui_error(Some(error)),
    }
}

fn player_release_after() -> Duration {
    let milliseconds = std::env::var("CODEX_PET_RELEASE_AFTER_MS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .map(|value| value.clamp(1_000, 3_600_000))
        .unwrap_or(300_000);
    Duration::from_millis(milliseconds)
}

unsafe fn apply_window_flags(context: *mut AppContext) {
    let Some(context) = context.as_mut() else {
        return;
    };
    let Some(snapshot) = context.runtime.ui_snapshot() else {
        return;
    };
    if snapshot.window.click_through || !snapshot.window.visible {
        reset_interaction(context);
    }
    set_click_through(context.video_hwnd, snapshot.window.click_through);
    set_click_through(context.info_hwnd, snapshot.window.click_through);
    let insert_after = if snapshot.window.always_on_top {
        HWND_TOPMOST
    } else {
        HWND_NOTOPMOST
    };
    let _ = SetWindowPos(
        context.video_hwnd,
        Some(insert_after),
        0,
        0,
        0,
        0,
        SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
    );
    let _ = SetWindowPos(
        context.info_hwnd,
        Some(insert_after),
        0,
        0,
        0,
        0,
        SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
    );
    if snapshot.window.visible {
        let _ = ShowWindow(context.video_hwnd, SW_SHOWNA);
        let _ = ShowWindow(context.info_hwnd, SW_SHOWNA);
    } else {
        let _ = ShowWindow(context.info_hwnd, SW_HIDE);
        let _ = ShowWindow(context.video_hwnd, SW_HIDE);
    }
    context
        .runtime
        .apply_window_visibility_to_player(snapshot.window.visible);
}

fn interaction_allowed(context: &AppContext) -> bool {
    context
        .runtime
        .ui_snapshot()
        .is_some_and(|snapshot| snapshot.window.visible && !snapshot.window.click_through)
}

fn start_interaction_transient(
    context: &mut AppContext,
    kind: InteractionKind,
    state: &'static str,
) {
    let Some(snapshot) = context.runtime.ui_snapshot() else {
        return;
    };
    if official_pet::is_attention_state(&snapshot.state.state)
        || snapshot.state.source == "official:first-awake"
    {
        return;
    }
    if context
        .interaction
        .active
        .as_ref()
        .is_some_and(|active| active.state == state)
    {
        return;
    }
    let (base_state, sessions_revision) = context
        .interaction
        .active
        .as_ref()
        .map(|active| (active.base_state.clone(), active.sessions_revision))
        .unwrap_or_else(|| (snapshot.state.state.clone(), snapshot.sessions.revision));
    if let Ok(snapshot) = context
        .runtime
        .transition_state(state, "official:interaction", None)
    {
        context.interaction.active = Some(ActiveTransient {
            kind,
            state,
            base_state,
            revision: snapshot.revision,
            sessions_revision,
        });
    }
}

fn end_interaction_transient(context: &mut AppContext, kind: Option<InteractionKind>) {
    if context
        .interaction
        .active
        .as_ref()
        .is_some_and(|active| kind.is_some_and(|kind| active.kind != kind))
    {
        return;
    }
    let Some(active) = context.interaction.active.take() else {
        return;
    };
    let _ = context.runtime.restore_state_if_revision(
        active.revision,
        active.base_state,
        active.sessions_revision,
        "official:interaction-end",
    );
}

fn reset_interaction(context: &mut AppContext) {
    context.interaction.hover_tracking = false;
    context.interaction.end_drag();
    end_interaction_transient(context, None);
}

unsafe fn set_click_through(hwnd: HWND, enabled: bool) {
    let _ = EnableWindow(hwnd, !enabled);
    let mut style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
    if enabled {
        style |= WS_EX_TRANSPARENT.0;
    } else {
        style &= !WS_EX_TRANSPARENT.0;
    }
    SetWindowLongPtrW(hwnd, GWL_EXSTYLE, style as isize);
    let _ = SetWindowPos(
        hwnd,
        None,
        0,
        0,
        0,
        0,
        SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_NOOWNERZORDER | SWP_FRAMECHANGED,
    );
}

unsafe fn handle_tray(context: *mut AppContext, event: u32) {
    match event {
        WM_LBUTTONUP => toggle_visibility(context),
        WM_RBUTTONUP => show_tray_menu(context),
        _ => {}
    }
}

unsafe fn show_tray_menu(context: *mut AppContext) {
    let Some(context) = context.as_mut() else {
        return;
    };
    let Some(snapshot) = context.runtime.ui_snapshot() else {
        return;
    };
    let menu = CreatePopupMenu().unwrap_or_default();
    if menu.is_invalid() {
        return;
    }
    let _ = AppendMenuW(menu, MF_STRING, MENU_SHOW as usize, w!("显示 / 隐藏"));
    let _ = AppendMenuW(menu, MF_SEPARATOR, 0, None);
    let _ = AppendMenuW(
        menu,
        MF_STRING,
        MENU_SIZE_SMALL as usize,
        w!("窗口大小：小（288 × 512）"),
    );
    let _ = AppendMenuW(
        menu,
        MF_STRING,
        MENU_SIZE_MEDIUM as usize,
        w!("窗口大小：中（360 × 640）"),
    );
    let _ = AppendMenuW(
        menu,
        MF_STRING,
        MENU_SIZE_LARGE as usize,
        w!("窗口大小：大（450 × 800）"),
    );
    let _ = AppendMenuW(menu, MF_SEPARATOR, 0, None);
    let _ = AppendMenuW(menu, MF_STRING, MENU_CLICK_THROUGH as usize, w!("鼠标穿透"));
    let _ = AppendMenuW(menu, MF_STRING, MENU_TOPMOST as usize, w!("始终置顶"));
    let _ = AppendMenuW(menu, MF_SEPARATOR, 0, None);
    let _ = AppendMenuW(menu, MF_STRING, MENU_QUIT as usize, w!("退出"));
    CheckMenuItem(
        menu,
        size_menu_command(context.size_preset) as u32,
        (MF_BYCOMMAND | MF_CHECKED).0,
    );
    CheckMenuItem(
        menu,
        MENU_CLICK_THROUGH as u32,
        (MF_BYCOMMAND
            | if snapshot.window.click_through {
                MF_CHECKED
            } else {
                MF_UNCHECKED
            })
        .0,
    );
    CheckMenuItem(
        menu,
        MENU_TOPMOST as u32,
        (MF_BYCOMMAND
            | if snapshot.window.always_on_top {
                MF_CHECKED
            } else {
                MF_UNCHECKED
            })
        .0,
    );
    let mut cursor = POINT::default();
    let _ = GetCursorPos(&mut cursor);
    let _ = SetForegroundWindow(context.video_hwnd);
    let command = TrackPopupMenu(
        menu,
        TPM_LEFTALIGN | TPM_BOTTOMALIGN | TPM_RIGHTBUTTON | TPM_RETURNCMD,
        cursor.x,
        cursor.y,
        Some(0),
        context.video_hwnd,
        None,
    );
    DestroyMenu(menu).ok();
    if command.0 != 0 {
        handle_menu(context, command.0 as u16);
    }
}

unsafe fn handle_menu(context: *mut AppContext, command: u16) {
    let Some(context) = context.as_mut() else {
        return;
    };
    let Some(snapshot) = context.runtime.ui_snapshot() else {
        return;
    };
    match command {
        MENU_SHOW => toggle_visibility(context),
        MENU_CLICK_THROUGH => {
            let _ = context.runtime.update_window(SetWindowRequest {
                visible: None,
                click_through: Some(!snapshot.window.click_through),
                always_on_top: None,
            });
        }
        MENU_TOPMOST => {
            let _ = context.runtime.update_window(SetWindowRequest {
                visible: None,
                click_through: None,
                always_on_top: Some(!snapshot.window.always_on_top),
            });
        }
        MENU_SIZE_SMALL => set_size_preset(context, WindowSizePreset::Small),
        MENU_SIZE_MEDIUM => set_size_preset(context, WindowSizePreset::Medium),
        MENU_SIZE_LARGE => set_size_preset(context, WindowSizePreset::Large),
        MENU_QUIT => {
            let _ = DestroyWindow(context.video_hwnd);
        }
        _ => {}
    }
}

fn size_menu_command(preset: WindowSizePreset) -> u16 {
    match preset {
        WindowSizePreset::Small => MENU_SIZE_SMALL,
        WindowSizePreset::Medium => MENU_SIZE_MEDIUM,
        WindowSizePreset::Large => MENU_SIZE_LARGE,
    }
}

fn resized_video_rect(current: RECT, work_area: RECT, width: i32, height: i32) -> RECT {
    let max_left = (work_area.right - width).max(work_area.left);
    let max_top = (work_area.bottom - height).max(work_area.top);
    let left = (current.right - width).clamp(work_area.left, max_left);
    let top = (current.bottom - height).clamp(work_area.top, max_top);
    RECT {
        left,
        top,
        right: left + width,
        bottom: top + height,
    }
}

unsafe fn set_size_preset(context: &mut AppContext, preset: WindowSizePreset) {
    if context.size_preset == preset {
        return;
    }
    if let Err(error) = apply_size_preset(context, preset) {
        context.runtime.set_ui_error(Some(error));
        context.runtime.notify_state();
    }
}

unsafe fn apply_size_preset(
    context: &mut AppContext,
    preset: WindowSizePreset,
) -> Result<(), String> {
    let mut current = RECT::default();
    GetWindowRect(context.video_hwnd, &mut current)
        .map_err(|error| format!("unable to read the video window size: {error}"))?;
    let monitor = MonitorFromWindow(context.video_hwnd, MONITOR_DEFAULTTONEAREST);
    let mut monitor_info = MONITORINFO {
        cbSize: size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    if !GetMonitorInfoW(monitor, &mut monitor_info).as_bool() {
        return Err("unable to read the current monitor work area".to_owned());
    }
    let (width, height) = preset_dimensions(preset);
    let next = resized_video_rect(current, monitor_info.rcWork, width, height);
    let old_width = current.right - current.left;
    let old_height = current.bottom - current.top;
    context.runtime.resize_player(width, height)?;
    if let Err(error) = SetWindowPos(
        context.video_hwnd,
        None,
        next.left,
        next.top,
        width,
        height,
        SWP_NOACTIVATE | SWP_NOOWNERZORDER,
    ) {
        let _ = context.runtime.resize_player(old_width, old_height);
        return Err(format!("unable to resize the video window: {error}"));
    }
    if let Err(error) = settings::set_window_size_preset(preset) {
        let _ = context.runtime.resize_player(old_width, old_height);
        let _ = SetWindowPos(
            context.video_hwnd,
            None,
            current.left,
            current.top,
            old_width,
            old_height,
            SWP_NOACTIVATE | SWP_NOOWNERZORDER,
        );
        return Err(error);
    }
    context.size_preset = preset;
    sync_overlay(context);
    Ok(())
}

unsafe fn toggle_visibility(context: *mut AppContext) {
    let Some(context) = context.as_mut() else {
        return;
    };
    if let Some(snapshot) = context.runtime.ui_snapshot() {
        let _ = context.runtime.update_window(SetWindowRequest {
            visible: Some(!snapshot.window.visible),
            click_through: None,
            always_on_top: None,
        });
    }
}

fn create_surface(factory: &ID2D1Factory, width: i32, height: i32) -> Result<Surface, String> {
    let dc = unsafe { CreateCompatibleDC(None) };
    if dc.is_invalid() {
        return Err("unable to create overlay DC".to_owned());
    }
    let info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: width,
            biHeight: -height,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut bits = ptr::null_mut();
    let bitmap = match unsafe { CreateDIBSection(None, &info, DIB_RGB_COLORS, &mut bits, None, 0) }
    {
        Ok(bitmap) => bitmap,
        Err(error) => {
            let _ = unsafe { DeleteDC(dc) };
            return Err(win_error(error));
        }
    };
    let previous = unsafe { SelectObject(dc, HGDIOBJ(bitmap.0)) };
    let properties = D2D1_RENDER_TARGET_PROPERTIES {
        r#type: D2D1_RENDER_TARGET_TYPE_DEFAULT,
        pixelFormat: D2D1_PIXEL_FORMAT {
            format: DXGI_FORMAT_B8G8R8A8_UNORM,
            alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
        },
        dpiX: 96.0 * VISUAL_SCALE,
        dpiY: 96.0 * VISUAL_SCALE,
        usage: D2D1_RENDER_TARGET_USAGE_NONE,
        minLevel: D2D1_FEATURE_LEVEL_DEFAULT,
    };
    let target = match unsafe { factory.CreateDCRenderTarget(&properties) } {
        Ok(target) => target,
        Err(error) => {
            unsafe {
                SelectObject(dc, previous);
                let _ = DeleteObject(HGDIOBJ(bitmap.0));
                let _ = DeleteDC(dc);
            }
            return Err(win_error(error));
        }
    };
    Ok(Surface {
        dc,
        bitmap,
        previous,
        bits: bits.cast(),
        target,
        width,
        height,
    })
}

fn update_layered(hwnd: HWND, surface: &Surface) -> Result<(), String> {
    let mut window = RECT::default();
    unsafe { GetWindowRect(hwnd, &mut window) }.map_err(win_error)?;
    let destination = POINT {
        x: window.left,
        y: window.top,
    };
    let source = POINT { x: 0, y: 0 };
    let size = SIZE {
        cx: surface.width,
        cy: surface.height,
    };
    let blend = BLENDFUNCTION {
        BlendOp: AC_SRC_OVER as u8,
        BlendFlags: 0,
        SourceConstantAlpha: 255,
        AlphaFormat: AC_SRC_ALPHA as u8,
    };
    unsafe {
        UpdateLayeredWindow(
            hwnd,
            None,
            Some(&destination),
            Some(&size),
            Some(surface.dc),
            Some(&source),
            COLORREF(0),
            Some(&blend),
            UPDATE_LAYERED_WINDOW_FLAGS(ULW_ALPHA.0),
        )
    }
    .map_err(win_error)
}

fn create_format(
    factory: &IDWriteFactory,
    size: f32,
    weight: windows::Win32::Graphics::DirectWrite::DWRITE_FONT_WEIGHT,
) -> Result<IDWriteTextFormat, String> {
    let format = unsafe {
        factory.CreateTextFormat(
            w!("Segoe UI Variable"),
            None::<&IDWriteFontCollection>,
            weight,
            DWRITE_FONT_STYLE_NORMAL,
            DWRITE_FONT_STRETCH_NORMAL,
            size,
            w!("zh-CN"),
        )
    }
    .map_err(win_error)?;
    unsafe {
        format
            .SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP)
            .map_err(win_error)?;
        format
            .SetTextAlignment(DWRITE_TEXT_ALIGNMENT_LEADING)
            .map_err(win_error)?;
        let sign = factory
            .CreateEllipsisTrimmingSign(&format)
            .map_err(win_error)?;
        let trimming = DWRITE_TRIMMING {
            granularity: DWRITE_TRIMMING_GRANULARITY_CHARACTER,
            delimiter: 0,
            delimiterCount: 0,
        };
        format.SetTrimming(&trimming, &sign).map_err(win_error)?;
    }
    Ok(format)
}

fn brush(
    target: &ID2D1DCRenderTarget,
    value: D2D1_COLOR_F,
) -> Result<ID2D1SolidColorBrush, String> {
    unsafe { target.CreateSolidColorBrush(&value, None) }.map_err(win_error)
}

fn gradient_brush(
    target: &ID2D1DCRenderTarget,
    start: D2D1_COLOR_F,
    end: D2D1_COLOR_F,
    start_x: f32,
    start_y: f32,
    end_x: f32,
    end_y: f32,
) -> Result<ID2D1LinearGradientBrush, String> {
    let stops = [
        D2D1_GRADIENT_STOP {
            position: 0.0,
            color: start,
        },
        D2D1_GRADIENT_STOP {
            position: 1.0,
            color: end,
        },
    ];
    let collection = unsafe {
        target.CreateGradientStopCollection(&stops, D2D1_GAMMA_2_2, D2D1_EXTEND_MODE_CLAMP)
    }
    .map_err(win_error)?;
    let mut properties = D2D1_LINEAR_GRADIENT_BRUSH_PROPERTIES::default();
    properties.startPoint.X = start_x;
    properties.startPoint.Y = start_y;
    properties.endPoint.X = end_x;
    properties.endPoint.Y = end_y;
    unsafe { target.CreateLinearGradientBrush(&properties, None, &collection) }.map_err(win_error)
}

#[allow(clippy::too_many_arguments)]
fn draw_text(
    target: &ID2D1DCRenderTarget,
    value: &str,
    format: &IDWriteTextFormat,
    brush: &ID2D1SolidColorBrush,
    left: f32,
    top: f32,
    right: f32,
    bottom: f32,
) {
    let text = value.encode_utf16().collect::<Vec<_>>();
    let rect = D2D_RECT_F {
        left,
        top,
        right,
        bottom,
    };
    unsafe {
        target.DrawText(
            &text,
            format,
            &rect,
            brush,
            D2D1_DRAW_TEXT_OPTIONS_CLIP,
            DWRITE_MEASURING_MODE_NATURAL,
        )
    };
}

fn rounded(left: f32, top: f32, right: f32, bottom: f32, radius: f32) -> D2D1_ROUNDED_RECT {
    D2D1_ROUNDED_RECT {
        rect: D2D_RECT_F {
            left,
            top,
            right,
            bottom,
        },
        radiusX: radius,
        radiusY: radius,
    }
}

fn color(red: u8, green: u8, blue: u8, alpha: f32) -> D2D1_COLOR_F {
    D2D1_COLOR_F {
        r: red as f32 / 255.0,
        g: green as f32 / 255.0,
        b: blue as f32 / 255.0,
        a: alpha,
    }
}

fn theme_color(value: UiColor, alpha: f32) -> D2D1_COLOR_F {
    color(value.red, value.green, value.blue, alpha)
}

fn mix_color(left: UiColor, right: UiColor, amount: f32) -> UiColor {
    let mix = |left: u8, right: u8| {
        (left as f32 + (right as f32 - left as f32) * amount.clamp(0.0, 1.0)).round() as u8
    };
    UiColor::new(
        mix(left.red, right.red),
        mix(left.green, right.green),
        mix(left.blue, right.blue),
    )
}

fn scale_color(value: UiColor, amount: f32) -> UiColor {
    let scale = |channel: u8| (channel as f32 * amount.clamp(0.0, 1.0)).round() as u8;
    UiColor::new(scale(value.red), scale(value.green), scale(value.blue))
}

fn state_color(state: &str, theme: &UiTheme) -> D2D1_COLOR_F {
    match state {
        "failed" => color(255, 114, 133, 1.0),
        "waiting" => color(255, 208, 105, 1.0),
        "review" => color(138, 240, 178, 1.0),
        _ => theme_color(theme.accent, 1.0),
    }
}

fn selected_detail(snapshot: &UiSnapshot) -> String {
    if let Some(selected) = snapshot
        .sessions
        .selected_session_id
        .as_deref()
        .and_then(|id| {
            snapshot
                .sessions
                .sessions
                .iter()
                .find(|session| session.id == id)
        })
    {
        return format!(
            "{} · {}",
            selected.title,
            activity_label(&selected.activity)
        );
    }
    match snapshot.state.state.as_str() {
        "idle" => "等待 Codex Session",
        "running-left" | "running-right" => "正在移动 Codex Pet",
        "waving" => "Codex Pet 向你打招呼",
        "jumping" => "鼠标悬停互动",
        "failed" => "任务遇到阻塞",
        "waiting" => "需要你的输入或授权",
        "running" => "Codex 正在工作",
        "review" => "任务已完成，等待查看",
        _ => "状态已更新",
    }
    .to_owned()
}

fn activity_label(value: &str) -> &'static str {
    match value {
        "starting" => "Session 已连接",
        "working" => "正在处理任务",
        "command" => "正在运行命令",
        "reading" => "正在读取文件",
        "editing" => "正在编辑文件",
        "searching" => "正在搜索信息",
        "waiting" => "等待输入或授权",
        "recovering" => "遇到错误，正在处理",
        "ready" => "已完成，点击查看",
        "subagent" => "Subagent 正在工作",
        "idle" => "当前空闲",
        _ => "状态已更新",
    }
}

fn open_session(
    runtime: &Arc<RuntimeData>,
    session_id: &str,
    thread_id: &str,
    revision: u64,
) -> Result<(), String> {
    if !runtime.is_verified_session_route(session_id, thread_id, revision) {
        return Err("主体任务尚未验证".to_owned());
    }
    runtime.set_session_error(None);
    let runtime = runtime.clone();
    let session_id = session_id.to_owned();
    let thread_id = thread_id.to_owned();
    thread::Builder::new()
        .name("codex-pet-open-session".to_owned())
        .spawn(move || {
            let result = open_session_with_launcher(
                &runtime,
                &session_id,
                &thread_id,
                revision,
                || runtime.refresh_active_routes(false),
                metadata::fetch_thread_metadata,
                launch_codex_uri,
            );
            if !runtime.is_shutting_down() {
                match result {
                    Ok(()) => runtime.set_session_error(None),
                    Err(error) => runtime.set_session_error(Some(error)),
                }
            }
        })
        .map(|_| ())
        .map_err(|_| "无法启动任务跳转验证".to_owned())
}

fn open_session_with_launcher<R, V, F>(
    runtime: &Arc<RuntimeData>,
    session_id: &str,
    thread_id: &str,
    revision: u64,
    route_refresher: R,
    verifier: V,
    launcher: F,
) -> Result<(), String>
where
    R: FnOnce() -> Result<(), String>,
    V: FnOnce(&str) -> Result<crate::core::ThreadMetadata, String>,
    F: FnOnce(&str) -> Result<(), String>,
{
    if !runtime.is_verified_session_route(session_id, thread_id, revision) {
        return Err("主体任务尚未验证".to_owned());
    }
    if runtime.is_shutting_down() {
        return Err("Codex Pet 正在退出".to_owned());
    }
    route_refresher()?;
    if !runtime.is_verified_session_route(session_id, thread_id, revision) {
        return Err("任务已归档或不再位于活动列表".to_owned());
    }
    let metadata = verifier(thread_id)?;
    if metadata.thread_id.as_deref() != Some(thread_id) {
        return Err("主体任务验证不一致".to_owned());
    }
    runtime.submit_verified_session_route(session_id, thread_id, revision, launcher)
}

fn launch_codex_uri(uri: &str) -> Result<(), String> {
    let Some(system_root) = std::env::var_os("SystemRoot").map(PathBuf::from) else {
        return Err("SystemRoot 不可用".to_owned());
    };
    let explorer = system_root.join("explorer.exe");
    if !explorer.is_file() {
        return Err("explorer.exe 不可用".to_owned());
    }
    let mut command = Command::new(explorer);
    command.arg(uri);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    command
        .spawn()
        .map(|_| ())
        .map_err(|_| "无法打开 Codex Session".to_owned())
}

fn clamp_session_offset(offset: usize, total: usize) -> usize {
    offset.min(total.saturating_sub(VISIBLE_SESSION_ROWS))
}

fn scroll_session_offset(offset: usize, total: usize, wheel_delta: i32) -> usize {
    let max_offset = total.saturating_sub(VISIBLE_SESSION_ROWS);
    if wheel_delta < 0 {
        offset.saturating_add(1).min(max_offset)
    } else if wheel_delta > 0 {
        offset.saturating_sub(1)
    } else {
        clamp_session_offset(offset, total)
    }
}

fn point_from_lparam(lparam: LPARAM) -> POINT {
    POINT {
        x: signed_low_word(lparam.0),
        y: signed_high_word(lparam.0),
    }
}

fn signed_low_word(value: isize) -> i32 {
    (value as u16 as i16) as i32
}

fn signed_high_word(value: isize) -> i32 {
    ((value >> 16) as u16 as i16) as i32
}

fn copy_wide_fixed<const N: usize>(value: &str, output: &mut [u16; N]) {
    let encoded = OsStr::new(value)
        .encode_wide()
        .take(N.saturating_sub(1))
        .collect::<Vec<_>>();
    output[..encoded.len()].copy_from_slice(&encoded);
    output[encoded.len()] = 0;
}

fn win_error(error: Error) -> String {
    error.to_string()
}

use std::os::windows::ffi::OsStrExt;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drag_state_changes_only_after_threshold_or_direction_change() {
        let mut interaction = InteractionState::default();
        interaction.begin_drag(100);
        assert_eq!(interaction.update_drag(103), None);
        assert_eq!(interaction.update_drag(104), Some("running-right"));
        assert_eq!(interaction.update_drag(112), None);
        assert_eq!(interaction.update_drag(108), Some("running-left"));
        interaction.end_drag();
        assert_eq!(interaction.update_drag(120), None);
    }

    #[test]
    fn session_scroll_reaches_every_row_and_clamps_after_shrink() {
        assert_eq!(scroll_session_offset(0, 3, -120), 0);
        assert_eq!(scroll_session_offset(0, 4, -120), 1);
        assert_eq!(scroll_session_offset(0, 32, -120), 1);
        assert_eq!(scroll_session_offset(29, 32, -120), 29);
        assert_eq!(scroll_session_offset(29, 32, 120), 28);
        assert_eq!(clamp_session_offset(29, 2), 0);
    }

    #[test]
    fn ui_theme_blending_preserves_semantic_attention_colors() {
        let theme = UiTheme {
            base: UiColor::new(9, 7, 15),
            accent: UiColor::new(111, 91, 183),
            light: UiColor::new(222, 227, 243),
        };
        let mixed = mix_color(theme.base, theme.accent, 0.5);
        assert_eq!(mixed, UiColor::new(60, 49, 99));
        assert_eq!(
            state_color("running", &theme),
            theme_color(theme.accent, 1.0)
        );
        assert_eq!(state_color("failed", &theme), color(255, 114, 133, 1.0));
        assert_eq!(state_color("waiting", &theme), color(255, 208, 105, 1.0));
        assert_eq!(state_color("review", &theme), color(138, 240, 178, 1.0));
    }

    #[test]
    fn session_cards_and_status_panel_stay_outside_video_bounds() {
        let (_, medium_height) = preset_dimensions(WindowSizePreset::Medium);
        let video_height = medium_height as f32 / VISUAL_SCALE;
        let total_height = INFO_TOP_EXTENT + video_height + INFO_BOTTOM_EXTENT;
        let video_bottom = total_height - INFO_BOTTOM_EXTENT;
        let layout = vertical_layout(total_height, SESSION_MAX_HEIGHT);

        assert_eq!(SESSION_MAX_HEIGHT, 141.0);
        assert!(layout.tray_top >= PANEL_MARGIN);
        assert!(layout.tray_top + SESSION_MAX_HEIGHT <= layout.video_top - PANEL_GAP);
        assert!(layout.status_top >= video_bottom + PANEL_GAP);
        assert!(layout.progress_bottom <= total_height - PANEL_MARGIN);
    }

    #[test]
    fn initial_video_position_leaves_room_for_the_session_cards() {
        let work_area = RECT {
            left: 0,
            top: 0,
            right: 2560,
            bottom: 1400,
        };
        let (medium_width, _) = preset_dimensions(WindowSizePreset::Medium);
        let (x, y) = video_position_in_work_area(work_area, medium_width);

        assert_eq!(
            x + medium_width + layout_to_pixels(PANEL_MARGIN),
            work_area.right
        );
        assert!(y - layout_to_pixels(INFO_TOP_EXTENT) >= work_area.top);
    }

    #[test]
    fn size_presets_are_exact_nine_by_sixteen_compositions() {
        for preset in [
            WindowSizePreset::Small,
            WindowSizePreset::Medium,
            WindowSizePreset::Large,
        ] {
            let (width, height) = preset_dimensions(preset);
            assert!(width >= MIN_WIDTH);
            assert!(height >= MIN_HEIGHT);
            assert_eq!(width * 16, height * 9);
        }
        assert_eq!(size_menu_command(WindowSizePreset::Small), MENU_SIZE_SMALL);
        assert_eq!(
            size_menu_command(WindowSizePreset::Medium),
            MENU_SIZE_MEDIUM
        );
        assert_eq!(size_menu_command(WindowSizePreset::Large), MENU_SIZE_LARGE);
    }

    #[test]
    fn resizing_preserves_the_bottom_right_anchor_inside_the_work_area() {
        let current = RECT {
            left: 800,
            top: 300,
            right: 1_160,
            bottom: 940,
        };
        let work_area = RECT {
            left: 0,
            top: 0,
            right: 1_920,
            bottom: 1_040,
        };
        let (width, height) = preset_dimensions(WindowSizePreset::Large);
        let resized = resized_video_rect(current, work_area, width, height);

        assert_eq!(resized.right, current.right);
        assert_eq!(resized.bottom, current.bottom);
        assert_eq!(resized.right - resized.left, width);
        assert_eq!(resized.bottom - resized.top, height);
    }

    #[test]
    fn resizing_clamps_a_partly_offscreen_window_back_to_the_work_area() {
        let current = RECT {
            left: 1_800,
            top: 900,
            right: 2_160,
            bottom: 1_540,
        };
        let work_area = RECT {
            left: 0,
            top: 0,
            right: 1_920,
            bottom: 1_040,
        };
        let (width, height) = preset_dimensions(WindowSizePreset::Medium);
        let resized = resized_video_rect(current, work_area, width, height);

        assert_eq!(resized.right, work_area.right);
        assert_eq!(resized.bottom, work_area.bottom);
        assert_eq!(resized.left, work_area.right - width);
        assert_eq!(resized.top, work_area.bottom - height);
    }

    #[test]
    fn only_verified_threads_launch_and_successful_submission_marks_review_read() {
        let states = crate::core::VALID_STATES
            .iter()
            .map(|state| {
                (
                    (*state).to_owned(),
                    crate::video_config::StateVideoConfig {
                        selection: crate::video_config::SelectionMode::WeightedRandom,
                        transition_policy: crate::video_config::TransitionPolicy::Immediate,
                        clips: vec![crate::video_config::VideoClipConfig {
                            file: format!("{state}.mp4"),
                            enabled: true,
                            weight: 1,
                            loop_range: None,
                        }],
                    },
                )
            })
            .collect();
        let config = crate::video_config::VideoConfig {
            schema_version: crate::video_config::VIDEO_CONFIG_SCHEMA_VERSION,
            states,
        };
        let root = std::path::PathBuf::from("test-catalog");
        let resolved_paths = crate::core::VALID_STATES
            .iter()
            .map(|state| ((*state).to_owned(), vec![format!("{state}.mp4")]))
            .collect();
        let runtime = RuntimeData::new_with_store(
            crate::video_config::LoadedVideoConfig {
                root: root.clone(),
                config_path: root.join(crate::video_config::VIDEO_CONFIG_FILENAME),
                source: crate::video_config::VideoConfigSource::LegacyScan,
                fingerprint: crate::video_config::config_fingerprint(&config),
                config,
                resolved_paths,
            },
            crate::storage::SessionStore::disabled(),
        );
        let session_id = "019fcc02-569f-7790-92ff-209aaca4275c";
        let now = crate::core::now_ms();
        runtime
            .snapshot
            .lock()
            .unwrap()
            .core
            .apply_event(crate::core::CodexEventRequest {
                schema_version: 1,
                event_id: "a".repeat(64),
                session_id: session_id.to_owned(),
                turn_id: None,
                hook_event_name: "Stop".to_owned(),
                state: "review".to_owned(),
                activity: "ready".to_owned(),
                emitted_at_ms: now,
            })
            .unwrap();
        let revision = 1;

        assert!(open_session_with_launcher(
            &runtime,
            session_id,
            session_id,
            revision,
            || Ok(()),
            |_| Ok(crate::core::ThreadMetadata::default()),
            |_| Ok(())
        )
        .is_err());

        {
            let mut snapshot = runtime.snapshot.lock().unwrap();
            snapshot.core.apply_metadata(
                session_id,
                revision,
                crate::core::ThreadMetadata {
                    thread_id: Some(session_id.to_owned()),
                    title: Some("已验证任务".to_owned()),
                    ..Default::default()
                },
            );
        }

        let mut refresh_failure_launched = false;
        assert!(open_session_with_launcher(
            &runtime,
            session_id,
            session_id,
            revision,
            || Err("active route unavailable".to_owned()),
            |_| panic!("metadata must not run when active-route refresh fails"),
            |_| {
                refresh_failure_launched = true;
                Ok(())
            }
        )
        .is_err());
        assert!(!refresh_failure_launched);

        assert!(open_session_with_launcher(
            &runtime,
            session_id,
            session_id,
            revision,
            || Ok(()),
            |_| Ok(crate::core::ThreadMetadata {
                thread_id: Some(session_id.to_owned()),
                ..Default::default()
            }),
            |_| Err("blocked".to_owned())
        )
        .is_err());
        let blocked = runtime.snapshot.lock().unwrap().core.sessions_snapshot();
        assert_eq!(blocked.sessions[0].state, "review");
        assert!(blocked.sessions[0].unread);

        let mut launched_uri = None;
        open_session_with_launcher(
            &runtime,
            session_id,
            session_id,
            revision,
            || Ok(()),
            |_| {
                Ok(crate::core::ThreadMetadata {
                    thread_id: Some(session_id.to_owned()),
                    ..Default::default()
                })
            },
            |uri| {
                launched_uri = Some(uri.to_owned());
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(
            launched_uri.as_deref(),
            Some("codex://threads/019fcc02-569f-7790-92ff-209aaca4275c")
        );
        let opened = runtime.snapshot.lock().unwrap().core.sessions_snapshot();
        assert_eq!(opened.session_count, 0);
        runtime.stop();
    }
}
