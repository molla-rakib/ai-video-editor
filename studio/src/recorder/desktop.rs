//! The desktop as the system sees it: the monitors and the windows a recording can show, bringing a
//! window to the front, and keeping this app out of its own recordings.
//!
//! A window is recorded as the part of the screen it covers (how Camtasia does it): Windows' own
//! window capture only delivers pictures when the window changes, so a still window would never
//! start a recording.

/// A rectangle in desktop pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Rect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl Rect {
    pub fn width(&self) -> i32 {
        self.right - self.left
    }

    pub fn height(&self) -> i32 {
        self.bottom - self.top
    }

    pub fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.left && x < self.right && y >= self.top && y < self.bottom
    }

    fn intersect(&self, other: &Rect) -> Rect {
        Rect { left: self.left.max(other.left), top: self.top.max(other.top), right: self.right.min(other.right), bottom: self.bottom.min(other.bottom) }
    }
}

/// A window that can be recorded.
#[derive(Debug, Clone, PartialEq)]
pub struct Window {
    /// The system's handle for it.
    pub handle: u64,
    /// The program it belongs to ("Chrome").
    pub app: String,
    pub title: String,
    pub rect: Rect,
}

/// The part of `window` inside `monitor`, relative to the monitor's top-left corner, with even sides
/// (video encoders need them): `(x, y, width, height)`. `None` when too little of it is on that monitor.
pub fn area_in(window: Rect, monitor: Rect) -> Option<(i32, i32, u32, u32)> {
    let r = window.intersect(&monitor);
    let (w, h) = (r.width() & !1, r.height() & !1);
    (w >= 64 && h >= 64).then(|| (r.left - monitor.left, r.top - monitor.top, w as u32, h as u32))
}

/// The monitor a window belongs to: the one holding its centre (else the first it overlaps).
pub fn monitor_of(window: Rect, monitors: &[Rect]) -> Option<usize> {
    let (cx, cy) = ((window.left + window.right) / 2, (window.top + window.bottom) / 2);
    monitors.iter().position(|m| m.contains(cx, cy)).or_else(|| monitors.iter().position(|m| area_in(window, *m).is_some()))
}

/// What the window list calls it: "Chrome: Pricing page", not too long.
pub fn window_label(app: &str, title: &str) -> String {
    let title = title.trim();
    let mut label = if app.is_empty() || title.to_lowercase().contains(&app.to_lowercase()) { title.to_string() } else { format!("{app}: {title}") };
    if label.chars().count() > 64 {
        label = label.chars().take(62).collect::<String>().trim_end().to_string() + "…";
    }
    label
}

/// "C:\\…\\chrome.exe" → "Chrome". (Split by hand: on Linux, where it is tested, `Path` doesn't take a
/// backslash for a separator.)
#[cfg_attr(not(any(windows, test)), allow(dead_code))]
pub fn app_name(exe: &str) -> String {
    let file = exe.rsplit(['\\', '/']).next().unwrap_or(exe);
    let stem = file.rsplit_once('.').map_or(file, |(stem, _)| stem).to_string();
    let mut chars = stem.chars();
    chars.next().map(|c| c.to_uppercase().collect::<String>() + chars.as_str()).unwrap_or_default()
}

/// A shortcut as the app writes it ("Ctrl+Shift+R", "F10") in the system's terms: the modifier bits
/// of `RegisterHotKey` (Alt 1, Ctrl 2, Shift 4) and the virtual-key code. `None` for keys a global
/// shortcut can't use (arrows, symbols: they differ between keyboard layouts).
pub fn hotkey_of(chord: &str) -> Option<(u32, u32)> {
    let mut parts: Vec<&str> = chord.split('+').collect();
    let key = parts.pop()?;
    let mut mods = 0;
    for m in parts {
        mods |= match m {
            "Alt" => 1,
            "Ctrl" => 2,
            "Shift" => 4,
            _ => return None,
        };
    }
    let vk = match key {
        k if k.len() == 1 && k.chars().all(|c| c.is_ascii_alphanumeric()) => k.to_ascii_uppercase().as_bytes()[0] as u32,
        k => 0x6F + k.strip_prefix('F')?.parse::<u32>().ok().filter(|n| (1..=24).contains(n))?,
    };
    Some((mods, vk))
}

/// Calls `pressed` whenever the shortcut is pressed, whichever program is in front, until dropped:
/// a screen recording is stopped with it while another app is being shown. `None` when the system
/// doesn't offer it (another program owns that shortcut, or not on Windows).
pub fn global_shortcut(chord: &str, pressed: impl Fn() + Send + 'static) -> Option<GlobalShortcut> {
    let (mods, vk) = hotkey_of(chord)?;
    register(mods, vk, Box::new(pressed))
}

#[cfg(windows)]
pub use self::windows_impl::*;

#[cfg(windows)]
mod windows_impl {
    use super::*;
    use windows::core::PWSTR;
    use windows::Win32::Foundation::{CloseHandle, BOOL, HWND, LPARAM, RECT};
    use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS};
    use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, IDXGIFactory1};
    use windows::Win32::System::Threading::{OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION};
    use windows::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetWindow, GetWindowLongW, GetWindowRect, GetWindowTextW, GetWindowThreadProcessId, IsIconic, IsWindowVisible, SetForegroundWindow, SetWindowDisplayAffinity, ShowWindow, GWL_EXSTYLE, GW_OWNER, SW_RESTORE, WDA_EXCLUDEFROMCAPTURE, WDA_NONE,
        WS_EX_TOOLWINDOW,
    };

    fn rect(r: RECT) -> Rect {
        Rect { left: r.left, top: r.top, right: r.right, bottom: r.bottom }
    }

    /// Asks for real pixels on this thread. Without it, a program Windows thinks doesn't know about
    /// display scaling gets monitors in scaled sizes (1829x1143 for a 3200x2000 panel at 175%) while
    /// window frames come in real pixels, and every window area would be wrong. (The app's own
    /// window thread is already this; the lookups run on other threads.)
    fn real_pixels() {
        use windows::Win32::UI::HiDpi::{SetThreadDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2};
        // SAFETY: changes only how this thread is told sizes.
        unsafe {
            SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        }
    }

    /// The monitors, in the order the desktop capture (`ddagrab=output_idx=N`) numbers them: the
    /// outputs of the first graphics adapter, as DXGI lists them.
    pub fn monitors() -> Vec<Rect> {
        real_pixels();
        // SAFETY: plain COM calls; each interface is released when dropped.
        unsafe {
            let Ok(factory) = CreateDXGIFactory1::<IDXGIFactory1>() else { return Vec::new() };
            let Ok(adapter) = factory.EnumAdapters1(0) else { return Vec::new() };
            (0..16).map_while(|i| adapter.EnumOutputs(i).ok()).filter_map(|o| o.GetDesc().ok()).filter(|d| d.AttachedToDesktop.as_bool()).map(|d| rect(d.DesktopCoordinates)).collect()
        }
    }

    /// Where the window really is: its visible frame (without the invisible resize border).
    pub fn window_rect(handle: u64) -> Option<Rect> {
        real_pixels();
        let hwnd = HWND(handle as isize as *mut core::ffi::c_void);
        let mut r = RECT::default();
        // SAFETY: the attribute is a RECT, of the size given.
        unsafe {
            if DwmGetWindowAttribute(hwnd, DWMWA_EXTENDED_FRAME_BOUNDS, &mut r as *mut RECT as *mut _, std::mem::size_of::<RECT>() as u32).is_err() {
                GetWindowRect(hwnd, &mut r).ok()?;
            }
        }
        Some(rect(r)).filter(|r| r.width() > 0 && r.height() > 0)
    }

    fn exe_of(pid: u32) -> String {
        // SAFETY: a handle opened and closed here; the buffer's size is passed with it.
        unsafe {
            let Ok(process) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, BOOL(0), pid) else { return String::new() };
            let mut buf = [0u16; 512];
            let mut len = buf.len() as u32;
            let ok = QueryFullProcessImageNameW(process, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len).is_ok();
            let _ = CloseHandle(process);
            if ok { String::from_utf16_lossy(&buf[..len as usize]) } else { String::new() }
        }
    }

    unsafe extern "system" fn collect(hwnd: HWND, found: LPARAM) -> BOOL {
        // SAFETY: `found` is the `Vec` passed to `EnumWindows` below, alive for the whole call.
        (*(found.0 as *mut Vec<HWND>)).push(hwnd);
        BOOL(1)
    }

    /// The windows a person would call windows: visible, not minimized, titled, not a tool window
    /// or a hidden (cloaked) app, not owned by another window, and not this app's own. Front first.
    pub fn windows() -> Vec<Window> {
        real_pixels();
        let mut handles: Vec<HWND> = Vec::new();
        // SAFETY: the callback only pushes into `handles`, which outlives the call.
        unsafe {
            let _ = EnumWindows(Some(collect), LPARAM(&mut handles as *mut Vec<HWND> as isize));
        }
        let own = std::process::id();
        handles
            .into_iter()
            .filter_map(|hwnd| {
                // SAFETY: read-only queries about a window handle the system just gave us.
                unsafe {
                    if !IsWindowVisible(hwnd).as_bool() || IsIconic(hwnd).as_bool() {
                        return None;
                    }
                    if GetWindow(hwnd, GW_OWNER).is_ok_and(|o| !o.is_invalid()) || GetWindowLongW(hwnd, GWL_EXSTYLE) as u32 & WS_EX_TOOLWINDOW.0 != 0 {
                        return None;
                    }
                    let mut cloaked = 0u32;
                    if DwmGetWindowAttribute(hwnd, DWMWA_CLOAKED, &mut cloaked as *mut u32 as *mut _, 4).is_ok() && cloaked != 0 {
                        return None;
                    }
                    let mut title = [0u16; 512];
                    let len = GetWindowTextW(hwnd, &mut title).max(0) as usize;
                    let title = String::from_utf16_lossy(&title[..len]);
                    let mut pid = 0u32;
                    GetWindowThreadProcessId(hwnd, Some(&mut pid));
                    if title.trim().is_empty() || pid == own || title == "Program Manager" {
                        return None;
                    }
                    let handle = hwnd.0 as isize as u64;
                    let rect = window_rect(handle)?;
                    (rect.width() >= 120 && rect.height() >= 80).then(|| Window { handle, app: app_name(&exe_of(pid)), title, rect })
                }
            })
            .collect()
    }

    /// Brings a window to the front (restoring it if it was minimized), so it is what gets recorded.
    pub fn bring_to_front(handle: u64) {
        let hwnd = HWND(handle as isize as *mut core::ffi::c_void);
        // SAFETY: plain window calls; a handle that went away makes them fail harmlessly.
        unsafe {
            if IsIconic(hwnd).as_bool() {
                let _ = ShowWindow(hwnd, SW_RESTORE);
            }
            let _ = SetForegroundWindow(hwnd);
        }
    }

    /// A global shortcut held by a thread of its own (the system sends it there as a message), let
    /// go of when dropped.
    pub struct GlobalShortcut {
        thread: u32,
    }

    impl Drop for GlobalShortcut {
        fn drop(&mut self) {
            use windows::Win32::Foundation::{LPARAM, WPARAM};
            use windows::Win32::UI::WindowsAndMessaging::{PostThreadMessageW, WM_QUIT};
            // SAFETY: a message to the shortcut's own thread, which unregisters it and ends.
            unsafe {
                let _ = PostThreadMessageW(self.thread, WM_QUIT, WPARAM(0), LPARAM(0));
            }
        }
    }

    pub(super) fn register(mods: u32, vk: u32, pressed: Box<dyn Fn() + Send>) -> Option<GlobalShortcut> {
        use windows::Win32::System::Threading::GetCurrentThreadId;
        use windows::Win32::UI::Input::KeyboardAndMouse::{RegisterHotKey, UnregisterHotKey, HOT_KEY_MODIFIERS, MOD_NOREPEAT};
        use windows::Win32::UI::WindowsAndMessaging::{GetMessageW, PeekMessageW, MSG, PM_NOREMOVE, WM_HOTKEY, WM_USER};
        let (ready, answer) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            // SAFETY: the shortcut belongs to this thread, which waits for its messages and lets go
            // of it before it ends.
            unsafe {
                let mut msg = MSG::default();
                // (Gives the thread its message queue before anyone can post to it.)
                let _ = PeekMessageW(&mut msg, HWND::default(), WM_USER, WM_USER, PM_NOREMOVE);
                if RegisterHotKey(HWND::default(), 1, HOT_KEY_MODIFIERS(mods) | MOD_NOREPEAT, vk).is_err() {
                    let _ = ready.send(None);
                    return;
                }
                let _ = ready.send(Some(GetCurrentThreadId()));
                // Ends at WM_QUIT (0) or on an error (-1).
                while GetMessageW(&mut msg, HWND::default(), 0, 0).0 > 0 {
                    if msg.message == WM_HOTKEY {
                        pressed();
                    }
                }
                let _ = UnregisterHotKey(HWND::default(), 1);
            }
        });
        answer.recv().ok().flatten().map(|thread| GlobalShortcut { thread })
    }

    /// Keeps one of this app's windows out of screen captures (it stays visible on the screen):
    /// the Record screen dialog isn't recorded. Windows 10 2004 and later; older systems ignore it.
    pub fn hide_from_capture(handle: u64, hide: bool) {
        let hwnd = HWND(handle as isize as *mut core::ffi::c_void);
        // SAFETY: a window of this process; failure leaves it as it was.
        unsafe {
            let _ = SetWindowDisplayAffinity(hwnd, if hide { WDA_EXCLUDEFROMCAPTURE } else { WDA_NONE });
        }
    }
}

/// On other systems windows aren't listed yet, and the app can't be hidden from a capture.
#[cfg(not(windows))]
mod other_impl {
    use super::*;

    pub fn windows() -> Vec<Window> {
        Vec::new()
    }

    pub fn window_rect(_: u64) -> Option<Rect> {
        None
    }

    pub fn bring_to_front(_: u64) {}

    pub fn hide_from_capture(_: u64, _: bool) {}

    /// (X11 would need a key grab, Wayland doesn't allow one: the bar's Stop button and the app's
    /// own shortcut work there.)
    pub struct GlobalShortcut;

    pub(super) fn register(_: u32, _: u32, _: Box<dyn Fn() + Send>) -> Option<GlobalShortcut> {
        None
    }
}

#[cfg(not(windows))]
pub use self::other_impl::*;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shortcuts_are_turned_into_system_keys() {
        assert_eq!(hotkey_of("Ctrl+Shift+R"), Some((2 | 4, b'R' as u32)));
        assert_eq!(hotkey_of("F10"), Some((0, 0x79)));
        assert_eq!(hotkey_of("Alt+F1"), Some((1, 0x70)));
        assert_eq!(hotkey_of("Ctrl+7"), Some((2, b'7' as u32)));
        for odd in ["", "Ctrl+", "Ctrl+←", "Ctrl+/", "F25", "Hyper+R", "Space"] {
            assert_eq!(hotkey_of(odd), None, "{odd}");
        }
    }

    #[test]
    fn a_window_is_recorded_as_the_even_sized_part_of_its_monitor_it_covers() {
        let monitor = Rect { left: 0, top: 0, right: 3200, bottom: 2000 };
        // Inside: offsets from the monitor, odd sides made even.
        assert_eq!(area_in(Rect { left: 101, top: 51, right: 1102, bottom: 852 }, monitor), Some((101, 51, 1000, 800)));
        // Hanging off the right edge: only what is on the monitor.
        assert_eq!(area_in(Rect { left: 3000, top: 0, right: 3600, bottom: 500 }, monitor), Some((3000, 0, 200, 500)));
        // Off it, or a sliver: nothing.
        assert_eq!(area_in(Rect { left: 3300, top: 0, right: 3600, bottom: 500 }, monitor), None);
        assert_eq!(area_in(Rect { left: 0, top: 0, right: 40, bottom: 500 }, monitor), None);
        // A second monitor to the left: offsets are from its own corner.
        let left = Rect { left: -1920, top: 0, right: 0, bottom: 1080 };
        assert_eq!(area_in(Rect { left: -1000, top: 100, right: -200, bottom: 700 }, left), Some((920, 100, 800, 600)));
        // Which monitor: the one with the window's centre.
        let monitors = [monitor, left];
        assert_eq!(monitor_of(Rect { left: -1000, top: 100, right: -200, bottom: 700 }, &monitors), Some(1));
        assert_eq!(monitor_of(Rect { left: -100, top: 100, right: 500, bottom: 700 }, &monitors), Some(0));
        assert_eq!(monitor_of(Rect { left: 9000, top: 9000, right: 9500, bottom: 9500 }, &monitors), None);
    }

    #[test]
    fn windows_are_named_by_program_and_title() {
        assert_eq!(app_name(r"C:\Program Files\Google\Chrome\Application\chrome.exe"), "Chrome");
        assert_eq!(app_name("/usr/bin/code"), "Code");
        assert_eq!(app_name(""), "");
        assert_eq!(window_label("Chrome", "Pricing page"), "Chrome: Pricing page");
        // A title that already names its program isn't repeated.
        assert_eq!(window_label("Code", "main.rs - Visual Studio Code"), "main.rs - Visual Studio Code");
        assert_eq!(window_label("Notepad", "notes.txt - Notepad"), "notes.txt - Notepad");
        let long = window_label("Chrome", &"word ".repeat(40));
        assert!(long.chars().count() <= 64 && long.ends_with('…'), "{long}");
    }

    /// Developer tool: what this computer would offer (titles stay on this machine).
    /// `cargo test -p studio lists_this_desktop -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn lists_this_desktop() {
        let began = std::time::Instant::now();
        #[cfg(windows)]
        println!("monitors: {:?}", monitors());
        let found = windows();
        println!("{} windows in {:.0} ms", found.len(), began.elapsed().as_secs_f64() * 1000.0);
        for w in &found {
            println!("  {} {}x{}", app_name(&w.app), w.rect.width(), w.rect.height());
        }
    }
}
