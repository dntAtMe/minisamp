#include <d3d9.h>
#include <windows.h>
#include <tchar.h>

LPDIRECT3D9 g_pD3D = NULL;
LPDIRECT3DDEVICE9 g_pD3DDevice = NULL;
HWND g_hWnd = NULL;

LRESULT WINAPI MsgProc(HWND hWnd, UINT msg, WPARAM wParam, LPARAM lParam);
HRESULT InitD3D(HWND hWnd);
VOID Cleanup();
VOID Render();

int APIENTRY _tWinMain(HINSTANCE hInst, HINSTANCE, LPTSTR, int) {
    WNDCLASSEX wc = { sizeof(WNDCLASSEX), CS_CLASSDC, MsgProc, 0L, 0L, GetModuleHandle(NULL), NULL, NULL, NULL, NULL, _T("Direct3D Hook Test"), NULL };
    RegisterClassEx(&wc);

    g_hWnd = CreateWindow(_T("Direct3D Hook Test"), _T("DirectX 9 Hook Test App"), WS_OVERLAPPEDWINDOW, 100, 100, 800, 600, NULL, NULL, wc.hInstance, NULL);

    if (SUCCEEDED(InitD3D(g_hWnd))) {
        ShowWindow(g_hWnd, SW_SHOWDEFAULT);
        UpdateWindow(g_hWnd);

        MSG msg;
        ZeroMemory(&msg, sizeof(msg));
        while (msg.message != WM_QUIT) {
            if (PeekMessage(&msg, NULL, 0U, 0U, PM_REMOVE)) {
                TranslateMessage(&msg);
                DispatchMessage(&msg);
            }
            else {
                Render();
            }
        }
    }

    Cleanup();
    UnregisterClass(_T("Direct3D Hook Test"), wc.hInstance);
    return 0;
}

LRESULT WINAPI MsgProc(HWND hWnd, UINT msg, WPARAM wParam, LPARAM lParam) {
    switch (msg) {
    case WM_DESTROY:
        PostQuitMessage(0);
        return 0;
    }
    return DefWindowProc(hWnd, msg, wParam, lParam);
}

HRESULT InitD3D(HWND hWnd) {
    if ((g_pD3D = Direct3DCreate9(D3D_SDK_VERSION)) == NULL)
        return E_FAIL;

    D3DPRESENT_PARAMETERS d3dpp;
    ZeroMemory(&d3dpp, sizeof(d3dpp));
    d3dpp.Windowed = TRUE;
    d3dpp.SwapEffect = D3DSWAPEFFECT_DISCARD;
    d3dpp.BackBufferFormat = D3DFMT_UNKNOWN;
    d3dpp.PresentationInterval = D3DPRESENT_INTERVAL_ONE;

    if (FAILED(g_pD3D->CreateDevice(D3DADAPTER_DEFAULT, D3DDEVTYPE_HAL, hWnd,
        D3DCREATE_SOFTWARE_VERTEXPROCESSING,
        &d3dpp, &g_pD3DDevice))) {
        return E_FAIL;
    }

    return S_OK;
}

VOID Cleanup() {
    if (g_pD3DDevice != NULL)
        g_pD3DDevice->Release();

    if (g_pD3D != NULL)
        g_pD3D->Release();
}

VOID Render() {
    g_pD3DDevice->Clear(0, NULL, D3DCLEAR_TARGET, D3DCOLOR_XRGB(255, 255, 255), 1.0f, 0);

    if (SUCCEEDED(g_pD3DDevice->BeginScene())) {

        g_pD3DDevice->EndScene();
    }

    g_pD3DDevice->Present(NULL, NULL, NULL, NULL);
}
