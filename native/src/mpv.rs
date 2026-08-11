use crate::video_config::{TransitionPolicy, VideoLoop};
use std::{
    ffi::{c_char, c_int, c_ulong, c_void, CStr, CString},
    path::{Path, PathBuf},
    ptr,
    sync::mpsc::{self, Receiver, RecvTimeoutError, Sender},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};
use windows::{
    core::{IUnknown, Interface, PCSTR, PCWSTR},
    Win32::{
        Foundation::{FreeLibrary, HMODULE, HWND},
        Graphics::{
            DirectComposition::{
                DCompositionCreateDevice, IDCompositionDevice, IDCompositionEffectGroup,
                IDCompositionTarget, IDCompositionVisual,
            },
            Dxgi::IDXGIDevice,
        },
        System::Com::{CoInitializeEx, CoUninitialize, COINIT_MULTITHREADED},
        System::LibraryLoader::{
            GetProcAddress, LoadLibraryExW, LOAD_LIBRARY_SEARCH_DEFAULT_DIRS,
            LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR,
        },
    },
};

const MPV_EVENT_NONE: i32 = 0;
const MPV_EVENT_SHUTDOWN: i32 = 1;
const MPV_EVENT_END_FILE: i32 = 7;
const MPV_EVENT_FILE_LOADED: i32 = 8;
const MPV_END_FILE_REASON_EOF: i32 = 0;
const MPV_END_FILE_REASON_ERROR: i32 = 4;
const EXPECTED_API_MAJOR: u32 = 2;
const EXPECTED_API_MINOR: u32 = 5;
pub const CROSSFADE_FRAMES: u32 = 5;
pub const NOMINAL_VIDEO_FPS: u32 = 24;
pub const CROSSFADE_DURATION_MS: u64 =
    (CROSSFADE_FRAMES as u64 * 1_000 + NOMINAL_VIDEO_FPS as u64 / 2) / NOMINAL_VIDEO_FPS as u64;
const PRELOAD_LEAD_MS: u64 = CROSSFADE_DURATION_MS + 450;
const MAX_EVENTS_PER_SLOT_TICK: usize = 64;
const MAX_COMMANDS_PER_TICK: usize = 4_096;
const LOAD_COALESCE_WINDOW_MS: u64 = 48;
const MAX_LOADS_PER_HANDLE: u32 = 64;

#[repr(C)]
struct MpvHandle {
    _private: [u8; 0],
}

#[repr(C)]
struct MpvEvent {
    event_id: c_int,
    error: c_int,
    reply_userdata: u64,
    data: *mut c_void,
}

#[repr(C)]
struct MpvEventEndFile {
    reason: c_int,
    error: c_int,
    playlist_entry_id: i64,
    playlist_insert_id: i64,
    playlist_insert_num_entries: c_int,
}

type ClientApiVersion = unsafe extern "C" fn() -> c_ulong;
type Create = unsafe extern "C" fn() -> *mut MpvHandle;
type Initialize = unsafe extern "C" fn(*mut MpvHandle) -> c_int;
type SetOptionString = unsafe extern "C" fn(*mut MpvHandle, *const c_char, *const c_char) -> c_int;
type SetPropertyString =
    unsafe extern "C" fn(*mut MpvHandle, *const c_char, *const c_char) -> c_int;
type Command = unsafe extern "C" fn(*mut MpvHandle, *const *const c_char) -> c_int;
type WaitEvent = unsafe extern "C" fn(*mut MpvHandle, f64) -> *const MpvEvent;
type GetPropertyString = unsafe extern "C" fn(*mut MpvHandle, *const c_char) -> *mut c_char;
type MpvFree = unsafe extern "C" fn(*mut c_void);
type TerminateDestroy = unsafe extern "C" fn(*mut MpvHandle);
type ErrorString = unsafe extern "C" fn(c_int) -> *const c_char;

struct MpvLibrary {
    module: HMODULE,
    client_api_version: ClientApiVersion,
    create: Create,
    initialize: Initialize,
    set_option_string: SetOptionString,
    set_property_string: SetPropertyString,
    command: Command,
    wait_event: WaitEvent,
    get_property_string: GetPropertyString,
    mpv_free: MpvFree,
    terminate_destroy: TerminateDestroy,
    error_string: ErrorString,
}

impl MpvLibrary {
    fn load(path: &Path) -> Result<Self, String> {
        let wide = to_wide(path.as_os_str());
        let module = unsafe {
            LoadLibraryExW(
                PCWSTR(wide.as_ptr()),
                None,
                LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_DEFAULT_DIRS,
            )
        }
        .map_err(|error| format!("unable to load {}: {error}", path.display()))?;

        let loaded = unsafe {
            (|| -> Result<Self, String> {
                Ok(Self {
                    module,
                    client_api_version: load_symbol(module, b"mpv_client_api_version\0")?,
                    create: load_symbol(module, b"mpv_create\0")?,
                    initialize: load_symbol(module, b"mpv_initialize\0")?,
                    set_option_string: load_symbol(module, b"mpv_set_option_string\0")?,
                    set_property_string: load_symbol(module, b"mpv_set_property_string\0")?,
                    command: load_symbol(module, b"mpv_command\0")?,
                    wait_event: load_symbol(module, b"mpv_wait_event\0")?,
                    get_property_string: load_symbol(module, b"mpv_get_property_string\0")?,
                    mpv_free: load_symbol(module, b"mpv_free\0")?,
                    terminate_destroy: load_symbol(module, b"mpv_terminate_destroy\0")?,
                    error_string: load_symbol(module, b"mpv_error_string\0")?,
                })
            })()
        };
        if loaded.is_err() {
            let _ = unsafe { FreeLibrary(module) };
        }
        loaded
    }

    fn api_version(&self) -> (u32, u32) {
        let raw = unsafe { (self.client_api_version)() };
        (raw >> 16, raw & 0xffff)
    }

    fn error(&self, code: i32) -> String {
        let pointer = unsafe { (self.error_string)(code) };
        if pointer.is_null() {
            return format!("libmpv error {code}");
        }
        unsafe { CStr::from_ptr(pointer) }
            .to_string_lossy()
            .into_owned()
    }
}

impl Drop for MpvLibrary {
    fn drop(&mut self) {
        let _ = unsafe { FreeLibrary(self.module) };
    }
}

unsafe fn load_symbol<T: Copy>(module: HMODULE, name: &'static [u8]) -> Result<T, String> {
    let symbol = GetProcAddress(module, PCSTR(name.as_ptr())).ok_or_else(|| {
        format!(
            "libmpv is missing {}",
            String::from_utf8_lossy(&name[..name.len() - 1])
        )
    })?;
    Ok(std::mem::transmute_copy(&symbol))
}

#[derive(Clone, Debug)]
pub enum PlayerEvent {
    LibraryReady {
        dll_path: String,
        api_major: u32,
        api_minor: u32,
        version: String,
        configuration: String,
    },
    HandleCount(u32),
    SourceSwitched {
        generation: u64,
        filename: String,
    },
    MediaReady {
        generation: u64,
        filename: String,
        hwdec: String,
    },
    MediaEnded {
        generation: u64,
    },
    MediaEnding {
        generation: u64,
    },
    CrossfadeStarted {
        generation: u64,
    },
    CrossfadeCompleted {
        generation: u64,
    },
    Playing(bool),
    Released,
    MediaError {
        generation: u64,
        message: String,
    },
    Error(String),
}

enum PlayerCommand {
    Load {
        path: String,
        generation: u64,
        loop_range: Option<VideoLoop>,
        at_boundary: bool,
        transition_policy: TransitionPolicy,
        force_interrupt: bool,
    },
    Visible(bool),
    Resize {
        width: i32,
        height: i32,
        response: Sender<Result<(), String>>,
    },
    Shutdown,
}

struct DesiredMedia {
    path: String,
    generation: u64,
    loop_range: Option<VideoLoop>,
    playlist_entry_id: Option<i64>,
    ready: bool,
    at_boundary: bool,
    report_events: bool,
    ending_notified: bool,
    seek_ms: u64,
    transition_policy: TransitionPolicy,
}

#[derive(Clone)]
struct LoadRequest {
    path: String,
    generation: u64,
    loop_range: Option<VideoLoop>,
    at_boundary: bool,
    transition_policy: TransitionPolicy,
    force_interrupt: bool,
}

struct PlayerEventInfo {
    event_id: i32,
    end_file_reason: i32,
    end_file_error: i32,
    end_file_entry_id: i64,
}

pub struct PlayerController {
    sender: Sender<PlayerCommand>,
    join: Option<JoinHandle<()>>,
}

impl PlayerController {
    pub fn start(
        video_hwnd: isize,
        composition_width: i32,
        composition_height: i32,
        release_after: Duration,
        event_sender: Sender<PlayerEvent>,
    ) -> Result<Self, String> {
        let dll_path = resolve_mpv_dll()?;
        let (sender, receiver) = mpsc::channel();
        let join = thread::Builder::new()
            .name("codex-pet-mpv".to_owned())
            .spawn(move || {
                run_player(
                    video_hwnd,
                    composition_width,
                    composition_height,
                    dll_path,
                    release_after,
                    receiver,
                    event_sender,
                )
            })
            .map_err(|error| format!("unable to start libmpv thread: {error}"))?;
        Ok(Self {
            sender,
            join: Some(join),
        })
    }

    pub fn load(
        &self,
        path: String,
        generation: u64,
        loop_range: Option<VideoLoop>,
        at_boundary: bool,
        transition_policy: TransitionPolicy,
        force_interrupt: bool,
    ) -> Result<(), String> {
        self.sender
            .send(PlayerCommand::Load {
                path,
                generation,
                loop_range,
                at_boundary,
                transition_policy,
                force_interrupt,
            })
            .map_err(|_| "libmpv command channel is unavailable".to_owned())
    }

    pub fn set_visible(&self, visible: bool) {
        let _ = self.sender.send(PlayerCommand::Visible(visible));
    }

    pub fn resize(&self, width: i32, height: i32) -> Result<(), String> {
        if width <= 0 || height <= 0 {
            return Err("video composition size must be positive".to_owned());
        }
        let (response, receiver) = mpsc::channel();
        self.sender
            .send(PlayerCommand::Resize {
                width,
                height,
                response,
            })
            .map_err(|_| "libmpv command channel is unavailable".to_owned())?;
        receiver
            .recv_timeout(Duration::from_secs(2))
            .map_err(|_| "libmpv resize did not complete in time".to_owned())?
    }

    pub fn shutdown(&mut self) {
        let _ = self.sender.send(PlayerCommand::Shutdown);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

impl Drop for PlayerController {
    fn drop(&mut self) {
        self.shutdown();
    }
}

struct InitializedPlayer {
    library: MpvLibrary,
    handle: *mut MpvHandle,
    load_count: u32,
}

impl InitializedPlayer {
    fn create(
        dll_path: &Path,
        composition_width: i32,
        composition_height: i32,
    ) -> Result<Self, String> {
        let library = MpvLibrary::load(dll_path)?;
        let (major, minor) = library.api_version();
        if major != EXPECTED_API_MAJOR || minor < EXPECTED_API_MINOR {
            return Err(format!(
                "unsupported libmpv client API {major}.{minor}; need {EXPECTED_API_MAJOR}.{EXPECTED_API_MINOR}+"
            ));
        }
        let handle = unsafe { (library.create)() };
        if handle.is_null() {
            return Err("mpv_create returned null".to_owned());
        }
        let mut player = Self {
            library,
            handle,
            load_count: 0,
        };
        for (name, value) in [
            ("terminal", "no"),
            ("msg-level", "all=no"),
            ("input-default-bindings", "no"),
            ("input-vo-keyboard", "no"),
            ("osc", "no"),
            ("audio", "no"),
            ("idle", "yes"),
            ("force-window", "immediate"),
            ("keep-open", "always"),
            ("loop-file", "no"),
            ("hwdec", "auto"),
            ("vo", "gpu-next"),
            ("gpu-api", "d3d11"),
            ("gpu-context", "d3d11"),
            ("d3d11-output-mode", "composition"),
            ("keepaspect", "yes"),
            ("panscan", "1.0"),
            ("border", "no"),
        ] {
            player.set_option(name, value)?;
        }
        player.set_option(
            "d3d11-composition-size",
            &format!("{composition_width}x{composition_height}"),
        )?;
        let result = unsafe { (player.library.initialize)(player.handle) };
        if result < 0 {
            let error = player.library.error(result);
            return Err(format!("mpv_initialize failed: {error}"));
        }
        Ok(player)
    }

    fn set_option(&mut self, name: &str, value: &str) -> Result<(), String> {
        let name = CString::new(name).map_err(|_| "invalid option name".to_owned())?;
        let value = CString::new(value).map_err(|_| "invalid option value".to_owned())?;
        let result =
            unsafe { (self.library.set_option_string)(self.handle, name.as_ptr(), value.as_ptr()) };
        if result < 0 {
            Err(self.library.error(result))
        } else {
            Ok(())
        }
    }

    fn set_property(&mut self, name: &str, value: &str) -> Result<(), String> {
        let name = CString::new(name).map_err(|_| "invalid property name".to_owned())?;
        let value = CString::new(value).map_err(|_| "invalid property value".to_owned())?;
        let result = unsafe {
            (self.library.set_property_string)(self.handle, name.as_ptr(), value.as_ptr())
        };
        if result < 0 {
            Err(self.library.error(result))
        } else {
            Ok(())
        }
    }

    fn property(&self, name: &str) -> Option<String> {
        let name = CString::new(name).ok()?;
        let pointer = unsafe { (self.library.get_property_string)(self.handle, name.as_ptr()) };
        if pointer.is_null() {
            return None;
        }
        let value = unsafe { CStr::from_ptr(pointer) }
            .to_string_lossy()
            .into_owned();
        unsafe { (self.library.mpv_free)(pointer.cast()) };
        Some(value)
    }

    fn load(&mut self, path: &str) -> Result<i64, String> {
        let command = CString::new("loadfile").unwrap();
        let path = CString::new(path).map_err(|_| "video path contains NUL".to_owned())?;
        let replace = CString::new("replace").unwrap();
        let args = [
            command.as_ptr(),
            path.as_ptr(),
            replace.as_ptr(),
            ptr::null(),
        ];
        let result = unsafe { (self.library.command)(self.handle, args.as_ptr()) };
        if result < 0 {
            Err(self.library.error(result))
        } else {
            self.load_count = self.load_count.saturating_add(1);
            self.property("playlist/0/id")
                .or_else(|| self.property("playlist-entry-id"))
                .and_then(|value| value.parse::<i64>().ok())
                .ok_or_else(|| "libmpv did not expose the current playlist entry id".to_owned())
        }
    }

    fn apply_loop(&mut self, loop_range: Option<&VideoLoop>) -> Result<(), String> {
        for (name, value) in loop_property_values(loop_range) {
            self.set_property(name, &value)?;
        }
        Ok(())
    }

    fn wait_event(&mut self, timeout: f64) -> Option<PlayerEventInfo> {
        let event = unsafe { (self.library.wait_event)(self.handle, timeout) };
        if event.is_null() {
            return None;
        }
        let event = unsafe { &*event };
        let (end_file_reason, end_file_error, end_file_entry_id) =
            if event.event_id == MPV_EVENT_END_FILE && !event.data.is_null() {
                let data = unsafe { &*(event.data.cast::<MpvEventEndFile>()) };
                (data.reason, data.error, data.playlist_entry_id)
            } else {
                (-1, 0, -1)
            };
        Some(PlayerEventInfo {
            event_id: event.event_id,
            end_file_reason,
            end_file_error,
            end_file_entry_id,
        })
    }

    fn terminate(mut self) {
        if !self.handle.is_null() {
            unsafe { (self.library.terminate_destroy)(self.handle) };
            self.handle = ptr::null_mut();
        }
    }
}

impl Drop for InitializedPlayer {
    fn drop(&mut self) {
        if !self.handle.is_null() {
            unsafe { (self.library.terminate_destroy)(self.handle) };
            self.handle = ptr::null_mut();
        }
    }
}

struct VideoCompositor {
    device: IDCompositionDevice,
    _target: IDCompositionTarget,
    root: IDCompositionVisual,
    visuals: [IDCompositionVisual; 2],
    effects: [IDCompositionEffectGroup; 2],
}

impl VideoCompositor {
    fn create(video_hwnd: isize) -> Result<Self, String> {
        let device: IDCompositionDevice = unsafe {
            DCompositionCreateDevice(None::<&IDXGIDevice>)
                .map_err(|error| format!("unable to create DirectComposition device: {error}"))?
        };
        let target = unsafe {
            device
                .CreateTargetForHwnd(HWND(video_hwnd as *mut _), true)
                .map_err(|error| format!("unable to create DirectComposition target: {error}"))?
        };
        let root = unsafe {
            device
                .CreateVisual()
                .map_err(|error| format!("unable to create DirectComposition root: {error}"))?
        };
        let visuals = [
            unsafe { device.CreateVisual() }
                .map_err(|error| format!("unable to create DirectComposition visual A: {error}"))?,
            unsafe { device.CreateVisual() }
                .map_err(|error| format!("unable to create DirectComposition visual B: {error}"))?,
        ];
        let effects = [
            unsafe { device.CreateEffectGroup() }
                .map_err(|error| format!("unable to create DirectComposition effect A: {error}"))?,
            unsafe { device.CreateEffectGroup() }
                .map_err(|error| format!("unable to create DirectComposition effect B: {error}"))?,
        ];
        unsafe {
            visuals[0]
                .SetEffect(&effects[0])
                .map_err(|error| format!("unable to bind DirectComposition effect A: {error}"))?;
            visuals[1]
                .SetEffect(&effects[1])
                .map_err(|error| format!("unable to bind DirectComposition effect B: {error}"))?;
            effects[0].SetOpacity2(0.0).map_err(|error| {
                format!("unable to initialize DirectComposition opacity A: {error}")
            })?;
            effects[1].SetOpacity2(0.0).map_err(|error| {
                format!("unable to initialize DirectComposition opacity B: {error}")
            })?;
            root.AddVisual(&visuals[0], false, None::<&IDCompositionVisual>)
                .map_err(|error| format!("unable to add DirectComposition visual A: {error}"))?;
            root.AddVisual(&visuals[1], true, &visuals[0])
                .map_err(|error| format!("unable to add DirectComposition visual B: {error}"))?;
            target
                .SetRoot(&root)
                .map_err(|error| format!("unable to set DirectComposition root: {error}"))?;
            device
                .Commit()
                .map_err(|error| format!("unable to commit DirectComposition tree: {error}"))?;
        }
        Ok(Self {
            device,
            _target: target,
            root,
            visuals,
            effects,
        })
    }

    fn bind_player(&self, index: usize, player: &InitializedPlayer) -> Result<(), String> {
        let pointer = player
            .property("display-swapchain")
            .and_then(|value| parse_pointer(&value))
            .ok_or_else(|| "libmpv did not expose its DirectComposition swapchain".to_owned())?;
        let raw = pointer as *mut c_void;
        let content = unsafe { IUnknown::from_raw_borrowed(&raw) }
            .ok_or_else(|| "libmpv exposed an invalid DirectComposition swapchain".to_owned())?;
        unsafe {
            self.visuals[index]
                .SetContent(content)
                .map_err(|error| format!("unable to bind video swapchain: {error}"))?;
            self.device
                .Commit()
                .map_err(|error| format!("unable to commit video swapchain: {error}"))
        }
    }

    fn clear_player(&self, index: usize) {
        unsafe {
            let _ = self.visuals[index].SetContent(None::<&IUnknown>);
            let _ = self.effects[index].SetOpacity2(0.0);
            let _ = self.device.Commit();
        }
    }

    fn bring_to_front(&self, index: usize) -> Result<(), String> {
        unsafe {
            self.root
                .RemoveVisual(&self.visuals[index])
                .map_err(|error| format!("unable to reorder video visual: {error}"))?;
            self.root
                // With a null reference visual, insertAbove=false places the child above every
                // sibling. insertAbove=true does the opposite and hid the fading slot below the
                // still-opaque outgoing video.
                .AddVisual(&self.visuals[index], false, None::<&IDCompositionVisual>)
                .map_err(|error| format!("unable to foreground video visual: {error}"))?;
            self.device
                .Commit()
                .map_err(|error| format!("unable to commit video z-order: {error}"))
        }
    }

    fn set_opacity(&self, index: usize, opacity: f32) -> Result<(), String> {
        unsafe {
            self.effects[index]
                .SetOpacity2(opacity.clamp(0.0, 1.0))
                .map_err(|error| format!("unable to set video blend opacity: {error}"))?;
            self.device
                .Commit()
                .map_err(|error| format!("unable to commit video blend opacity: {error}"))
        }
    }
}

struct PlayerSlot {
    player: Option<InitializedPlayer>,
    media: Option<DesiredMedia>,
}

struct Crossfade {
    outgoing: Option<usize>,
    incoming: usize,
    generation: u64,
    started_at: Instant,
}

fn should_defer_load(
    slots: &[PlayerSlot; 2],
    front: Option<usize>,
    pending: Option<usize>,
    fade: Option<&Crossfade>,
    request: &LoadRequest,
) -> bool {
    !request.force_interrupt
        && effective_transition_policy(slots, front, pending, fade)
            == TransitionPolicy::FinishCurrentClip
}

fn effective_transition_policy(
    slots: &[PlayerSlot; 2],
    front: Option<usize>,
    pending: Option<usize>,
    fade: Option<&Crossfade>,
) -> TransitionPolicy {
    if let Some(active_fade) = fade {
        let incoming = slots[active_fade.incoming].media.as_ref();
        if incoming.is_some_and(|media| !media.at_boundary) {
            return incoming
                .map(|media| media.transition_policy)
                .unwrap_or_default();
        }
        return active_fade
            .outgoing
            .and_then(|index| slots[index].media.as_ref())
            .map(|media| media.transition_policy)
            .unwrap_or_default();
    }
    if let Some(index) = pending {
        let incoming = slots[index].media.as_ref();
        if incoming.is_some_and(|media| !media.at_boundary) {
            return incoming
                .map(|media| media.transition_policy)
                .unwrap_or_default();
        }
    }
    front
        .and_then(|index| slots[index].media.as_ref())
        .map(|media| media.transition_policy)
        .unwrap_or_default()
}

fn run_player(
    video_hwnd: isize,
    composition_width: i32,
    composition_height: i32,
    dll_path: PathBuf,
    release_after: Duration,
    receiver: Receiver<PlayerCommand>,
    event_sender: Sender<PlayerEvent>,
) {
    if unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.is_err() {
        let _ = event_sender.send(PlayerEvent::Error(
            "unable to initialize COM for DirectComposition".to_owned(),
        ));
        return;
    }
    let compositor = match VideoCompositor::create(video_hwnd) {
        Ok(compositor) => compositor,
        Err(error) => {
            let _ = event_sender.send(PlayerEvent::Error(error));
            unsafe { CoUninitialize() };
            return;
        }
    };
    let mut slots = [
        PlayerSlot {
            player: None,
            media: None,
        },
        PlayerSlot {
            player: None,
            media: None,
        },
    ];
    let mut front: Option<usize> = None;
    let mut pending: Option<usize> = None;
    let mut fade: Option<Crossfade> = None;
    let mut visible = true;
    let mut hidden_since: Option<Instant> = None;
    let mut last_desired: Option<LoadRequest> = None;
    let mut deferred_load: Option<LoadRequest> = None;
    let mut composition_size = (composition_width, composition_height);
    let mut running = true;

    for index in 0..slots.len() {
        let _ = compositor.set_opacity(index, 0.0);
    }

    while running {
        let first_command = match receiver.recv_timeout(Duration::from_millis(16)) {
            Ok(command) => Some(command),
            Err(RecvTimeoutError::Timeout) => None,
            Err(RecvTimeoutError::Disconnected) => {
                running = false;
                None
            }
        };
        let mut latest_load = None;
        let mut requested_visibility = None;
        let mut shutdown_requested = false;
        if let Some(first_command) = first_command {
            let coalesce_loads = matches!(&first_command, PlayerCommand::Load { .. });
            let mut commands = vec![first_command];
            if coalesce_loads {
                let deadline = Instant::now() + Duration::from_millis(LOAD_COALESCE_WINDOW_MS);
                while commands.len() < MAX_COMMANDS_PER_TICK {
                    let remaining = deadline.saturating_duration_since(Instant::now());
                    if remaining.is_zero() {
                        break;
                    }
                    match receiver.recv_timeout(remaining) {
                        Ok(command) => {
                            let urgent = matches!(
                                &command,
                                PlayerCommand::Visible(_)
                                    | PlayerCommand::Resize { .. }
                                    | PlayerCommand::Shutdown
                            );
                            commands.push(command);
                            if urgent {
                                break;
                            }
                        }
                        Err(RecvTimeoutError::Timeout) => break,
                        Err(RecvTimeoutError::Disconnected) => {
                            shutdown_requested = true;
                            break;
                        }
                    }
                }
            }
            commands.extend(
                receiver
                    .try_iter()
                    .take(MAX_COMMANDS_PER_TICK - commands.len()),
            );
            for command in commands {
                match command {
                    PlayerCommand::Load {
                        path,
                        generation,
                        loop_range,
                        at_boundary,
                        transition_policy,
                        force_interrupt,
                    } => {
                        latest_load = Some(LoadRequest {
                            path,
                            generation,
                            loop_range,
                            at_boundary,
                            transition_policy,
                            force_interrupt,
                        })
                    }
                    PlayerCommand::Visible(next_visible) => {
                        requested_visibility = Some(next_visible)
                    }
                    PlayerCommand::Resize {
                        width,
                        height,
                        response,
                    } => {
                        let result = resize_players(&mut slots, composition_size, (width, height));
                        if result.is_ok() {
                            composition_size = (width, height);
                        }
                        let _ = response.send(result);
                    }
                    PlayerCommand::Shutdown => shutdown_requested = true,
                }
            }
        }
        if shutdown_requested {
            running = false;
            continue;
        }

        if let Some(next_visible) = requested_visibility {
            let was_visible = visible;
            visible = next_visible;
            if visible {
                hidden_since = None;
                if latest_load.is_none() && slots.iter().all(|slot| slot.player.is_none()) {
                    if let Some(request) = last_desired.clone() {
                        deferred_load = None;
                        if let Err(message) = prepare_media(
                            &mut slots,
                            &mut front,
                            &mut pending,
                            &mut fade,
                            &compositor,
                            &dll_path,
                            composition_size,
                            request.path,
                            request.generation,
                            request.loop_range,
                            0,
                            false,
                            true,
                            request.transition_policy,
                            &event_sender,
                        ) {
                            let _ = event_sender.send(PlayerEvent::MediaError {
                                generation: request.generation,
                                message,
                            });
                        }
                    }
                } else if !was_visible {
                    for slot in &mut slots {
                        if let Some(player) = slot.player.as_mut() {
                            let _ = player.set_property("pause", "no");
                        }
                    }
                    let _ = event_sender.send(PlayerEvent::Playing(true));
                }
            } else if was_visible {
                hidden_since = Some(Instant::now());
                for slot in &mut slots {
                    if let Some(player) = slot.player.as_mut() {
                        let _ = player.set_property("pause", "yes");
                    }
                }
                let _ = event_sender.send(PlayerEvent::Playing(false));
            }
        }

        if let Some(mut request) = latest_load {
            last_desired = Some(request.clone());
            if visible {
                let replace_boundary_pending = fade.is_none()
                    && pending.is_some_and(|index| {
                        slots[index]
                            .media
                            .as_ref()
                            .is_some_and(|media| media.at_boundary)
                    });
                if should_defer_load(&slots, front, pending, fade.as_ref(), &request)
                    && !replace_boundary_pending
                {
                    deferred_load = Some(request);
                } else {
                    deferred_load = None;
                    if replace_boundary_pending {
                        request.at_boundary = true;
                    }
                    if let Err(message) = prepare_media(
                        &mut slots,
                        &mut front,
                        &mut pending,
                        &mut fade,
                        &compositor,
                        &dll_path,
                        composition_size,
                        request.path,
                        request.generation,
                        request.loop_range,
                        0,
                        request.at_boundary,
                        true,
                        request.transition_policy,
                        &event_sender,
                    ) {
                        let _ = event_sender.send(PlayerEvent::MediaError {
                            generation: request.generation,
                            message,
                        });
                    }
                }
            } else {
                deferred_load = Some(request);
            }
        }

        if !visible
            && hidden_since.is_some_and(|since| since.elapsed() >= release_after)
            && slots.iter().any(|slot| slot.player.is_some())
        {
            release_slots(&mut slots, &compositor);
            front = None;
            pending = None;
            fade = None;
            let _ = event_sender.send(PlayerEvent::HandleCount(0));
            let _ = event_sender.send(PlayerEvent::Released);
        }

        let mut force_boundary_start = false;
        let mut force_deferred_start = false;
        for index in 0..slots.len() {
            let mut events = Vec::new();
            if let Some(player) = slots[index].player.as_mut() {
                for _ in 0..MAX_EVENTS_PER_SLOT_TICK {
                    let Some(event) = player.wait_event(0.0) else {
                        break;
                    };
                    if event.event_id == MPV_EVENT_NONE {
                        break;
                    }
                    events.push(event);
                }
            }
            for event in events {
                match event.event_id {
                    MPV_EVENT_FILE_LOADED => {
                        let slot = &mut slots[index];
                        let (Some(player), Some(media)) =
                            (slot.player.as_mut(), slot.media.as_mut())
                        else {
                            continue;
                        };
                        let current_entry_id = player
                            .property("playlist/0/id")
                            .or_else(|| player.property("playlist-entry-id"))
                            .and_then(|value| value.parse::<i64>().ok());
                        let current_path = player.property("path");
                        if current_entry_id != media.playlist_entry_id
                            || current_path
                                .as_deref()
                                .is_none_or(|path| !same_path(path, &media.path))
                        {
                            continue;
                        }
                        if let Err(error) = player.apply_loop(None) {
                            let _ = event_sender.send(PlayerEvent::MediaError {
                                generation: media.generation,
                                message: format!("unable to clear native video loop: {error}"),
                            });
                            continue;
                        }
                        let _ = player.set_property("pause", "yes");
                        if media.seek_ms > 0 {
                            let _ = player.set_property("time-pos", &format_seconds(media.seek_ms));
                        }
                        if let Err(message) = compositor.bind_player(index, player) {
                            let _ = event_sender.send(PlayerEvent::MediaError {
                                generation: media.generation,
                                message,
                            });
                            continue;
                        }
                        media.ready = true;
                        if media.report_events {
                            let hwdec = player
                                .property("hwdec-current")
                                .unwrap_or_else(|| "unavailable".to_owned());
                            let _ = event_sender.send(PlayerEvent::MediaReady {
                                generation: media.generation,
                                filename: filename(&media.path),
                                hwdec,
                            });
                        }
                    }
                    MPV_EVENT_END_FILE => {
                        let Some(media) = slots[index].media.as_ref() else {
                            continue;
                        };
                        if media.playlist_entry_id != Some(event.end_file_entry_id) {
                            continue;
                        }
                        if event.end_file_reason == MPV_END_FILE_REASON_ERROR
                            && event.end_file_error < 0
                        {
                            let message = slots[index]
                                .player
                                .as_ref()
                                .map(|player| player.library.error(event.end_file_error))
                                .unwrap_or_else(|| "video playback failed".to_owned());
                            let _ = event_sender.send(PlayerEvent::MediaError {
                                generation: media.generation,
                                message,
                            });
                        } else if event.end_file_reason == MPV_END_FILE_REASON_EOF
                            && front == Some(index)
                        {
                            if pending.is_some_and(|next| {
                                slots[next].media.as_ref().is_some_and(|media| media.ready)
                            }) {
                                force_boundary_start = true;
                            } else if deferred_load.is_some() {
                                force_deferred_start = true;
                            } else if media.loop_range.is_some() {
                                let _ = event_sender.send(PlayerEvent::MediaError {
                                    generation: media.generation,
                                    message: "media ended before the blended loop was ready"
                                        .to_owned(),
                                });
                            } else {
                                let _ = event_sender.send(PlayerEvent::MediaEnded {
                                    generation: media.generation,
                                });
                            }
                        }
                    }
                    MPV_EVENT_SHUTDOWN => {
                        compositor.clear_player(index);
                        if let Some(player) = slots[index].player.take() {
                            player.terminate();
                        }
                        slots[index].media = None;
                        if front == Some(index) {
                            front = None;
                        }
                        if pending == Some(index) {
                            pending = None;
                        }
                        fade = None;
                        let _ = event_sender.send(PlayerEvent::HandleCount(handle_count(&slots)));
                        let _ = event_sender.send(PlayerEvent::Error(
                            "libmpv requested shutdown; the slot will be recreated".to_owned(),
                        ));
                    }
                    _ => {}
                }
            }
        }

        if visible && force_deferred_start && pending.is_none() && fade.is_none() {
            if let Some(mut request) = deferred_load.take() {
                request.at_boundary = true;
                if let Err(message) = prepare_media(
                    &mut slots,
                    &mut front,
                    &mut pending,
                    &mut fade,
                    &compositor,
                    &dll_path,
                    composition_size,
                    request.path,
                    request.generation,
                    request.loop_range,
                    0,
                    true,
                    true,
                    request.transition_policy,
                    &event_sender,
                ) {
                    let _ = event_sender.send(PlayerEvent::MediaError {
                        generation: request.generation,
                        message,
                    });
                } else {
                    force_boundary_start = true;
                }
            }
        }

        if visible && fade.is_none() && pending.is_none() {
            let should_start_now = deferred_load.as_ref().is_some_and(|request| {
                !should_defer_load(&slots, front, pending, fade.as_ref(), request)
            });
            if should_start_now {
                if let Some(request) = deferred_load.take() {
                    if let Err(message) = prepare_media(
                        &mut slots,
                        &mut front,
                        &mut pending,
                        &mut fade,
                        &compositor,
                        &dll_path,
                        composition_size,
                        request.path,
                        request.generation,
                        request.loop_range,
                        0,
                        request.at_boundary,
                        true,
                        request.transition_policy,
                        &event_sender,
                    ) {
                        let _ = event_sender.send(PlayerEvent::MediaError {
                            generation: request.generation,
                            message,
                        });
                    }
                }
            }
        }

        if visible && fade.is_none() && pending.is_none() {
            if let Some(front_index) = front {
                let boundary = playback_boundary_ms(&slots[front_index]);
                let position = playback_position_ms(&slots[front_index]);
                if let (Some(boundary), Some(position)) = (boundary, position) {
                    if boundary.saturating_sub(position) <= PRELOAD_LEAD_MS {
                        if let Some(mut request) = deferred_load.take() {
                            request.at_boundary = true;
                            if let Err(message) = prepare_media(
                                &mut slots,
                                &mut front,
                                &mut pending,
                                &mut fade,
                                &compositor,
                                &dll_path,
                                composition_size,
                                request.path,
                                request.generation,
                                request.loop_range,
                                0,
                                true,
                                true,
                                request.transition_policy,
                                &event_sender,
                            ) {
                                let _ = event_sender.send(PlayerEvent::MediaError {
                                    generation: request.generation,
                                    message,
                                });
                            }
                        } else if let Some(media) = slots[front_index].media.as_mut() {
                            if let Some(loop_range) = media.loop_range.clone() {
                                let path = media.path.clone();
                                let generation = media.generation;
                                let transition_policy = media.transition_policy;
                                if let Err(message) = prepare_media(
                                    &mut slots,
                                    &mut front,
                                    &mut pending,
                                    &mut fade,
                                    &compositor,
                                    &dll_path,
                                    composition_size,
                                    path,
                                    generation,
                                    Some(loop_range.clone()),
                                    loop_range.start_ms,
                                    true,
                                    false,
                                    transition_policy,
                                    &event_sender,
                                ) {
                                    let _ = event_sender.send(PlayerEvent::MediaError {
                                        generation,
                                        message,
                                    });
                                }
                            } else if !media.ending_notified {
                                media.ending_notified = true;
                                let _ = event_sender.send(PlayerEvent::MediaEnding {
                                    generation: media.generation,
                                });
                            }
                        }
                    }
                }
            }
        }

        if visible && fade.is_none() {
            if let Some(incoming) = pending {
                let ready = slots[incoming]
                    .media
                    .as_ref()
                    .is_some_and(|media| media.ready);
                let at_boundary = slots[incoming]
                    .media
                    .as_ref()
                    .is_some_and(|media| media.at_boundary);
                let boundary_due = front.is_none()
                    || !at_boundary
                    || force_boundary_start
                    || front.is_some_and(|index| {
                        match (
                            playback_boundary_ms(&slots[index]),
                            playback_position_ms(&slots[index]),
                        ) {
                            (Some(boundary), Some(position)) => {
                                boundary.saturating_sub(position) <= CROSSFADE_DURATION_MS
                            }
                            _ => false,
                        }
                    });
                if ready && boundary_due {
                    fade = start_crossfade(&mut slots, front, incoming, &compositor, &event_sender);
                }
            }
        }

        if visible {
            if let Some(active_fade) = fade.as_ref() {
                let progress = (active_fade.started_at.elapsed().as_secs_f64()
                    / (CROSSFADE_DURATION_MS as f64 / 1_000.0))
                    .clamp(0.0, 1.0);
                let incoming_alpha = fade_alpha(progress);
                let _ = compositor
                    .set_opacity(active_fade.incoming, incoming_alpha as f32 / u8::MAX as f32);
                if let Some(outgoing) = active_fade.outgoing {
                    // The incoming slot is above the outgoing slot. Keeping the old slot opaque
                    // while the new slot fades in produces the intended A*(1-t)+B*t contribution
                    // without the dark mid-fade dip caused by attenuating both layered windows.
                    let _ = compositor.set_opacity(outgoing, 1.0);
                }
                if progress >= 1.0 {
                    let completed = fade.take().unwrap();
                    let _ = compositor.set_opacity(completed.incoming, 1.0);
                    if let Some(outgoing) = completed.outgoing {
                        compositor.clear_player(outgoing);
                        if let Some(player) = slots[outgoing].player.take() {
                            player.terminate();
                        }
                        slots[outgoing].media = None;
                    }
                    front = Some(completed.incoming);
                    pending = None;
                    let _ = event_sender.send(PlayerEvent::HandleCount(handle_count(&slots)));
                    let _ = event_sender.send(PlayerEvent::CrossfadeCompleted {
                        generation: completed.generation,
                    });
                }
            }
        }
    }

    release_slots(&mut slots, &compositor);
    let _ = event_sender.send(PlayerEvent::HandleCount(0));
    unsafe { CoUninitialize() };
}

fn resize_players(
    slots: &mut [PlayerSlot; 2],
    current_size: (i32, i32),
    next_size: (i32, i32),
) -> Result<(), String> {
    if next_size.0 <= 0 || next_size.1 <= 0 {
        return Err("video composition size must be positive".to_owned());
    }
    if current_size == next_size {
        return Ok(());
    }
    let current = format!("{}x{}", current_size.0, current_size.1);
    let next = format!("{}x{}", next_size.0, next_size.1);
    let mut updated: Vec<usize> = Vec::new();
    for index in 0..slots.len() {
        let Some(player) = slots[index].player.as_mut() else {
            continue;
        };
        if let Err(error) = player.set_property("d3d11-composition-size", &next) {
            for updated_index in updated {
                if let Some(player) = slots[updated_index].player.as_mut() {
                    let _ = player.set_property("d3d11-composition-size", &current);
                }
            }
            return Err(format!("unable to resize video composition: {error}"));
        }
        updated.push(index);
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn prepare_media(
    slots: &mut [PlayerSlot; 2],
    front: &mut Option<usize>,
    pending: &mut Option<usize>,
    fade: &mut Option<Crossfade>,
    compositor: &VideoCompositor,
    dll_path: &Path,
    composition_size: (i32, i32),
    path: String,
    generation: u64,
    loop_range: Option<VideoLoop>,
    seek_ms: u64,
    at_boundary: bool,
    report_events: bool,
    transition_policy: TransitionPolicy,
    event_sender: &Sender<PlayerEvent>,
) -> Result<(), String> {
    if let Some(cancelled) = fade.take() {
        if let Some(outgoing) = cancelled.outgoing {
            let _ = compositor.set_opacity(outgoing, 1.0);
            *front = Some(outgoing);
        }
        compositor.clear_player(cancelled.incoming);
        if let Some(player) = slots[cancelled.incoming].player.as_mut() {
            let _ = player.set_property("pause", "yes");
        }
        slots[cancelled.incoming].media = None;
    }
    let target = front.map_or(0, |index| 1 - index);
    compositor.clear_player(target);
    if let Some(player) = slots[target].player.as_mut() {
        let _ = player.set_property("pause", "yes");
    }
    slots[target].media = None;
    let _ = compositor.set_opacity(target, 0.0);
    if slots[target]
        .player
        .as_ref()
        .is_some_and(|player| player.load_count >= MAX_LOADS_PER_HANDLE)
    {
        if let Some(player) = slots[target].player.take() {
            player.terminate();
        }
    }
    if slots[target].player.is_none() {
        slots[target].player = Some(create_player(
            dll_path,
            composition_size.0,
            composition_size.1,
            event_sender,
        )?);
    }
    let entry_id = slots[target]
        .player
        .as_mut()
        .ok_or_else(|| "video slot was not initialized".to_owned())?
        .load(&path)?;
    slots[target].media = Some(DesiredMedia {
        path: path.clone(),
        generation,
        loop_range,
        playlist_entry_id: Some(entry_id),
        ready: false,
        at_boundary,
        report_events,
        ending_notified: false,
        seek_ms,
        transition_policy,
    });
    *pending = Some(target);
    if report_events {
        let _ = event_sender.send(PlayerEvent::SourceSwitched {
            generation,
            filename: filename(&path),
        });
    }
    let _ = event_sender.send(PlayerEvent::HandleCount(handle_count(slots)));
    Ok(())
}

fn start_crossfade(
    slots: &mut [PlayerSlot; 2],
    outgoing: Option<usize>,
    incoming: usize,
    compositor: &VideoCompositor,
    event_sender: &Sender<PlayerEvent>,
) -> Option<Crossfade> {
    let generation = slots[incoming].media.as_ref()?.generation;
    let player = slots[incoming].player.as_mut()?;
    if let Err(error) = player.set_property("pause", "no") {
        let _ = event_sender.send(PlayerEvent::MediaError {
            generation,
            message: error,
        });
        return None;
    }
    if let Err(message) = compositor.bring_to_front(incoming) {
        let _ = event_sender.send(PlayerEvent::MediaError {
            generation,
            message,
        });
        return None;
    }
    let _ = event_sender.send(PlayerEvent::CrossfadeStarted { generation });
    let _ = event_sender.send(PlayerEvent::Playing(true));
    Some(Crossfade {
        outgoing,
        incoming,
        generation,
        started_at: Instant::now(),
    })
}

fn playback_position_ms(slot: &PlayerSlot) -> Option<u64> {
    property_milliseconds(slot.player.as_ref()?, "time-pos")
}

fn playback_boundary_ms(slot: &PlayerSlot) -> Option<u64> {
    if let Some(end_ms) = slot
        .media
        .as_ref()
        .and_then(|media| media.loop_range.as_ref().map(|range| range.end_ms))
    {
        return Some(end_ms);
    }
    property_milliseconds(slot.player.as_ref()?, "duration")
}

fn property_milliseconds(player: &InitializedPlayer, name: &str) -> Option<u64> {
    let seconds = player.property(name)?.parse::<f64>().ok()?;
    seconds
        .is_finite()
        .then(|| (seconds.max(0.0) * 1_000.0).round() as u64)
}

fn fade_alpha(progress: f64) -> u8 {
    let progress = progress.clamp(0.0, 1.0);
    let smooth = progress * progress * (3.0 - 2.0 * progress);
    (smooth * u8::MAX as f64).round() as u8
}

fn handle_count(slots: &[PlayerSlot; 2]) -> u32 {
    slots.iter().filter(|slot| slot.player.is_some()).count() as u32
}

fn release_slots(slots: &mut [PlayerSlot; 2], compositor: &VideoCompositor) {
    for (index, slot) in slots.iter_mut().enumerate() {
        compositor.clear_player(index);
        if let Some(player) = slot.player.take() {
            player.terminate();
        }
        slot.media = None;
    }
}

fn create_player(
    dll_path: &Path,
    composition_width: i32,
    composition_height: i32,
    event_sender: &Sender<PlayerEvent>,
) -> Result<InitializedPlayer, String> {
    let player = InitializedPlayer::create(dll_path, composition_width, composition_height)?;
    let (major, minor) = player.library.api_version();
    let version = player
        .property("mpv-version")
        .unwrap_or_else(|| "unknown".to_owned());
    let configuration = player
        .property("mpv-configuration")
        .unwrap_or_else(|| "unknown".to_owned());
    let _ = event_sender.send(PlayerEvent::LibraryReady {
        dll_path: dll_path.display().to_string(),
        api_major: major,
        api_minor: minor,
        version,
        configuration,
    });
    Ok(player)
}

fn resolve_mpv_dll() -> Result<PathBuf, String> {
    let mut candidates = Vec::new();
    if let Some(configured) = std::env::var_os("CODEX_PET_MPV_DLL") {
        let configured = PathBuf::from(configured);
        if configured.is_absolute() {
            candidates.push(configured);
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            candidates.push(parent.join("libmpv-2.dll"));
            candidates.push(parent.join("mpv-2.dll"));
        }
    }
    let project = Path::new(env!("CARGO_MANIFEST_DIR"));
    candidates.push(project.join("third_party").join("mpv").join("libmpv-2.dll"));
    candidates.push(
        project
            .parent()
            .unwrap_or(project)
            .join("third_party")
            .join("mpv")
            .join("libmpv-2.dll"),
    );
    for path in &candidates {
        if path.is_file() {
            return path
                .canonicalize()
                .map_err(|error| format!("unable to resolve libmpv path: {error}"));
        }
    }
    Err(format!(
        "libmpv-2.dll was not found; checked {}",
        candidates
            .iter()
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>()
            .join("; ")
    ))
}

fn filename(path: &str) -> String {
    Path::new(path)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("video")
        .to_owned()
}

fn same_path(left: &str, right: &str) -> bool {
    fn normalize(value: &str) -> String {
        value
            .strip_prefix(r"\\?\")
            .unwrap_or(value)
            .replace('/', "\\")
    }
    normalize(left).eq_ignore_ascii_case(&normalize(right))
}

fn parse_pointer(value: &str) -> Option<usize> {
    let value = value.trim();
    if let Some(hex) = value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
    {
        usize::from_str_radix(hex, 16).ok()
    } else {
        value.parse::<usize>().ok()
    }
    .filter(|pointer| *pointer != 0)
}

fn loop_property_values(loop_range: Option<&VideoLoop>) -> Vec<(&'static str, String)> {
    let mut values = vec![
        ("ab-loop-a", "no".to_owned()),
        ("ab-loop-b", "no".to_owned()),
    ];
    if let Some(loop_range) = loop_range {
        values.push(("loop-file", "no".to_owned()));
        values.push(("ab-loop-a", format_seconds(loop_range.start_ms)));
        values.push(("ab-loop-b", format_seconds(loop_range.end_ms)));
    } else {
        values.push(("loop-file", "no".to_owned()));
    }
    values
}

fn format_seconds(milliseconds: u64) -> String {
    format!("{}.{:03}", milliseconds / 1000, milliseconds % 1000)
}

fn to_wide(value: &std::ffi::OsStr) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    value.encode_wide().chain(Some(0)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_media(transition_policy: TransitionPolicy, at_boundary: bool) -> DesiredMedia {
        DesiredMedia {
            path: "test.mp4".to_owned(),
            generation: 1,
            loop_range: None,
            playlist_entry_id: None,
            ready: true,
            at_boundary,
            report_events: true,
            ending_notified: false,
            seek_ms: 0,
            transition_policy,
        }
    }

    fn test_request(force_interrupt: bool) -> LoadRequest {
        LoadRequest {
            path: "next.mp4".to_owned(),
            generation: 2,
            loop_range: None,
            at_boundary: false,
            transition_policy: TransitionPolicy::Immediate,
            force_interrupt,
        }
    }

    #[test]
    fn filename_is_bounded_to_the_leaf() {
        assert_eq!(filename(r"C:\videos\idle-1.mp4"), "idle-1.mp4");
    }

    #[test]
    fn windows_paths_compare_case_insensitively() {
        assert!(same_path(r"\\?\C:\Videos\Idle.mp4", r"c:/videos/idle.mp4"));
    }

    #[test]
    fn composition_swapchain_pointer_accepts_decimal_and_hex() {
        assert_eq!(parse_pointer("4660"), Some(0x1234));
        assert_eq!(parse_pointer("0x1234"), Some(0x1234));
        assert_eq!(parse_pointer("0"), None);
    }

    #[test]
    fn loop_properties_clear_previous_points_before_applying_new_ones() {
        let values = loop_property_values(Some(&VideoLoop {
            start_ms: 1250,
            end_ms: 3675,
        }));
        assert_eq!(values[0], ("ab-loop-a", "no".to_owned()));
        assert_eq!(values[1], ("ab-loop-b", "no".to_owned()));
        assert_eq!(values[2], ("loop-file", "no".to_owned()));
        assert_eq!(values[3], ("ab-loop-a", "1.250".to_owned()));
        assert_eq!(values[4], ("ab-loop-b", "3.675".to_owned()));
        assert_eq!(
            loop_property_values(None),
            vec![
                ("ab-loop-a", "no".to_owned()),
                ("ab-loop-b", "no".to_owned()),
                ("loop-file", "no".to_owned())
            ]
        );
    }

    #[test]
    fn five_frame_crossfade_uses_the_nominal_asset_cadence() {
        assert_eq!(CROSSFADE_FRAMES, 5);
        assert_eq!(NOMINAL_VIDEO_FPS, 24);
        assert_eq!(CROSSFADE_DURATION_MS, 208);
    }

    #[test]
    fn crossfade_alpha_is_bounded_monotonic_and_eased() {
        let samples = [0.0, 0.2, 0.4, 0.6, 0.8, 1.0].map(fade_alpha);
        assert_eq!(samples[0], 0);
        assert_eq!(samples[5], 255);
        assert!(samples.windows(2).all(|pair| pair[0] <= pair[1]));
        assert!(samples[1] < 51);
        assert!(samples[4] > 204);
    }

    #[test]
    fn protected_clip_defers_normal_load_but_attention_can_preempt() {
        let slots = [
            PlayerSlot {
                player: None,
                media: Some(test_media(TransitionPolicy::FinishCurrentClip, false)),
            },
            PlayerSlot {
                player: None,
                media: None,
            },
        ];
        assert!(should_defer_load(
            &slots,
            Some(0),
            None,
            None,
            &test_request(false)
        ));
        assert!(!should_defer_load(
            &slots,
            Some(0),
            None,
            None,
            &test_request(true)
        ));
    }

    #[test]
    fn boundary_preload_keeps_the_outgoing_clip_policy_authoritative() {
        let slots = [
            PlayerSlot {
                player: None,
                media: Some(test_media(TransitionPolicy::FinishCurrentClip, false)),
            },
            PlayerSlot {
                player: None,
                media: Some(test_media(TransitionPolicy::Immediate, true)),
            },
        ];
        assert!(should_defer_load(
            &slots,
            Some(0),
            Some(1),
            None,
            &test_request(false)
        ));
    }
}
