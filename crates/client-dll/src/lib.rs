use std::ffi::c_void;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::ptr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use minhook::MinHook;
use once_cell::sync::OnceCell;
use windows::core::HRESULT;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct3D9::*;
use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
use windows::Win32::System::SystemServices::DLL_PROCESS_ATTACH;
use windows::Win32::System::Threading::CreateThread;
use windows::Win32::UI::WindowsAndMessaging::*;

use shared::{self, Message, DEFAULT_PORT};

/// IDirect3DDevice9::EndScene (vtable index 42)
type EndSceneFn = unsafe extern "system" fn(this: *mut c_void) -> HRESULT;

/// IDirect3DDevice9::Clear (vtable index 43)
type ClearFn = unsafe extern "system" fn(
    this: *mut c_void,
    count: u32,
    rects: *const D3DRECT,
    flags: u32,
    color: u32,
    z: f32,
    stencil: u32,
) -> HRESULT;

static ORIGINAL_END_SCENE: OnceCell<EndSceneFn> = OnceCell::new();
static HOOKED: AtomicBool = AtomicBool::new(false);
static CONNECTION: OnceCell<Mutex<TcpStream>> = OnceCell::new();

// --- Hooked EndScene ---

unsafe extern "system" fn hk_end_scene(this: *mut c_void) -> HRESULT {
    if !HOOKED.load(Ordering::SeqCst) {
        HOOKED.store(true, Ordering::SeqCst);
    }

    // Visual proof the hook is working: tint the screen blue
    // Read Clear from device vtable (index 43)
    let vtable = *(this as *const *const *const c_void);
    let clear_fn: ClearFn = std::mem::transmute(*vtable.add(43));
    // D3DCOLOR_ARGB(100, 0, 0, 255) — semi-transparent blue overlay
    let _ = clear_fn(this, 0, ptr::null(), D3DCLEAR_TARGET as u32, 0x640000FF, 1.0, 0);

    if let Some(&original) = ORIGINAL_END_SCENE.get() {
        original(this)
    } else {
        HRESULT(0)
    }
}

// --- Hook EndScene directly using a dummy device ---

unsafe fn hook_end_scene() {
    let d3d9_module = match GetModuleHandleW(windows::core::w!("d3d9.dll")) {
        Ok(h) if !h.is_invalid() => h,
        _ => return,
    };

    let create9_addr = match GetProcAddress(d3d9_module, windows::core::s!("Direct3DCreate9")) {
        Some(addr) => addr,
        None => return,
    };

    let direct3d_create9: unsafe extern "system" fn(u32) -> *mut c_void =
        std::mem::transmute(create9_addr);

    let d3d_ptr = direct3d_create9(D3D_SDK_VERSION);
    if d3d_ptr.is_null() {
        return;
    }

    unsafe extern "system" fn dummy_wnd_proc(
        hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM,
    ) -> LRESULT {
        DefWindowProcW(hwnd, msg, wparam, lparam)
    }

    let class_name = windows::core::w!("MiniSampDummy");
    let wc = WNDCLASSEXW {
        cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
        lpfnWndProc: Some(dummy_wnd_proc),
        lpszClassName: class_name,
        ..Default::default()
    };
    RegisterClassExW(&wc);

    let dummy_hwnd = CreateWindowExW(
        WINDOW_EX_STYLE::default(),
        class_name,
        windows::core::w!(""),
        WS_OVERLAPPEDWINDOW,
        0, 0, 1, 1,
        None,
        None,
        HINSTANCE::default(),
        None,
    );
    let dummy_hwnd = match dummy_hwnd {
        Ok(h) => h,
        Err(_) => return,
    };

    let d3d_vtable = *(d3d_ptr as *const *const *const c_void);

    type CreateDeviceRaw = unsafe extern "system" fn(
        this: *mut c_void,
        adapter: u32,
        device_type: D3DDEVTYPE,
        focus_window: HWND,
        behavior_flags: u32,
        pp: *mut D3DPRESENT_PARAMETERS,
        out_device: *mut *mut c_void,
    ) -> HRESULT;

    let create_device_fn: CreateDeviceRaw = std::mem::transmute(*d3d_vtable.add(16));

    let mut d3dpp = D3DPRESENT_PARAMETERS {
        Windowed: BOOL::from(true),
        SwapEffect: D3DSWAPEFFECT_DISCARD,
        BackBufferFormat: D3DFMT_UNKNOWN,
        ..Default::default()
    };

    let mut dummy_device: *mut c_void = ptr::null_mut();
    let hr = create_device_fn(
        d3d_ptr,
        D3DADAPTER_DEFAULT,
        D3DDEVTYPE_HAL,
        dummy_hwnd,
        D3DCREATE_SOFTWARE_VERTEXPROCESSING as u32,
        &mut d3dpp,
        &mut dummy_device,
    );

    if hr.is_err() || dummy_device.is_null() {
        let release: unsafe extern "system" fn(*mut c_void) -> u32 =
            std::mem::transmute(*d3d_vtable.add(2));
        release(d3d_ptr);
        let _ = DestroyWindow(dummy_hwnd);
        return;
    }

    // Read EndScene address from device vtable (index 42)
    let device_vtable = *(dummy_device as *const *const *const c_void);
    let end_scene_addr = *device_vtable.add(42) as *mut c_void;

    if let Ok(trampoline) =
        MinHook::create_hook(end_scene_addr, hk_end_scene as *mut c_void)
    {
        let original_fn: EndSceneFn = std::mem::transmute(trampoline);
        let _ = ORIGINAL_END_SCENE.set(original_fn);
        let _ = MinHook::enable_hook(end_scene_addr);
    }

    // Release dummy device and D3D9 object
    let release: unsafe extern "system" fn(*mut c_void) -> u32 = {
        let vtable = *(dummy_device as *const *const *const c_void);
        std::mem::transmute(*vtable.add(2))
    };
    release(dummy_device);

    let release_d3d: unsafe extern "system" fn(*mut c_void) -> u32 =
        std::mem::transmute(*d3d_vtable.add(2));
    release_d3d(d3d_ptr);

    let _ = DestroyWindow(dummy_hwnd);
}

// --- Networking ---

fn send_message(stream: &mut TcpStream, msg: &Message) {
    let payload = shared::serialize(msg);
    let len = (payload.len() as u32).to_le_bytes();
    let _ = stream.write_all(&len);
    let _ = stream.write_all(&payload);
    let _ = stream.flush();
}

fn network_loop() {
    let addr = format!("127.0.0.1:{DEFAULT_PORT}");
    let mut stream = match TcpStream::connect(&addr) {
        Ok(s) => s,
        Err(_) => return,
    };

    // Send hello while still in blocking mode
    let msg = Message::GameMessage("Hello server from Rust client-dll!".to_string());
    send_message(&mut stream, &msg);

    // Now switch to nonblocking for the poll loop
    stream.set_nonblocking(true).ok();

    let _ = CONNECTION.set(Mutex::new(stream));

    // Keep the thread alive, poll for incoming messages
    let mut buf = [0u8; 4096];
    loop {
        if let Some(conn) = CONNECTION.get() {
            if let Ok(mut stream) = conn.lock() {
                match stream.read(&mut buf) {
                    Ok(0) => break,
                    Ok(_) => {}
                    Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                    Err(_) => break,
                }
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(16));
    }
}

// --- DLL entry point ---

#[no_mangle]
unsafe extern "system" fn DllMain(
    _module: HMODULE,
    call_reason: u32,
    _reserved: *mut c_void,
) -> BOOL {
    if call_reason == DLL_PROCESS_ATTACH {
        let _ = CreateThread(None, 0, Some(main_thread), None, Default::default(), None);
    }
    TRUE
}

unsafe extern "system" fn main_thread(_param: *mut c_void) -> u32 {
    hook_end_scene();
    network_loop();
    0
}
