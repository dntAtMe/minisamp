use std::ptr;

use windows::{
    core::*,
    Win32::Foundation::*,
    Win32::Graphics::Direct3D9::*,
    Win32::Graphics::Gdi::UpdateWindow,
    Win32::System::LibraryLoader::GetModuleHandleW,
    Win32::UI::WindowsAndMessaging::*,
};

static mut G_PD3D: Option<IDirect3D9> = None;
static mut G_DEVICE: Option<IDirect3DDevice9> = None;

fn main() {
    unsafe { run().expect("Failed to run client-target") };
}

unsafe fn run() -> Result<()> {
    let module = GetModuleHandleW(None)?;
    let instance: HINSTANCE = std::mem::transmute(module);
    let class_name = w!("Direct3D Hook Test");

    let wc = WNDCLASSEXW {
        cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
        style: CS_CLASSDC,
        lpfnWndProc: Some(wnd_proc),
        hInstance: instance,
        lpszClassName: class_name,
        ..Default::default()
    };
    RegisterClassExW(&wc);

    let hwnd = CreateWindowExW(
        WINDOW_EX_STYLE::default(),
        class_name,
        w!("DirectX 9 Hook Test App"),
        WS_OVERLAPPEDWINDOW,
        100,
        100,
        800,
        600,
        None,
        None,
        instance,
        None,
    )?;

    init_d3d(hwnd)?;

    let _ = ShowWindow(hwnd, SW_SHOWDEFAULT);
    let _ = UpdateWindow(hwnd);

    let mut msg = MSG::default();
    loop {
        if PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
            if msg.message == WM_QUIT {
                break;
            }
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        } else {
            render();
        }
    }

    cleanup();
    let _ = UnregisterClassW(class_name, instance);
    Ok(())
}

unsafe fn init_d3d(hwnd: HWND) -> Result<()> {
    let d3d = Direct3DCreate9(D3D_SDK_VERSION).ok_or_else(|| Error::from(E_FAIL))?;

    let mut d3dpp = D3DPRESENT_PARAMETERS {
        Windowed: BOOL::from(true),
        SwapEffect: D3DSWAPEFFECT_DISCARD,
        BackBufferFormat: D3DFMT_UNKNOWN,
        PresentationInterval: D3DPRESENT_INTERVAL_ONE as u32,
        ..Default::default()
    };

    let mut device: Option<IDirect3DDevice9> = None;
    d3d.CreateDevice(
        D3DADAPTER_DEFAULT,
        D3DDEVTYPE_HAL,
        hwnd,
        D3DCREATE_SOFTWARE_VERTEXPROCESSING as u32,
        &mut d3dpp,
        &mut device,
    )?;

    G_PD3D = Some(d3d);
    G_DEVICE = device;
    Ok(())
}

unsafe fn cleanup() {
    G_DEVICE = None;
    G_PD3D = None;
}

unsafe fn render() {
    if let Some(ref device) = G_DEVICE {
        let _ = device.Clear(0, ptr::null(), D3DCLEAR_TARGET as u32, 0xFFFFFFFF, 1.0, 0);
        if device.BeginScene().is_ok() {
            let _ = device.EndScene();
        }
        let _ = device.Present(ptr::null(), ptr::null(), None, ptr::null());
    }
}

unsafe extern "system" fn wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_DESTROY => {
            PostQuitMessage(0);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}
