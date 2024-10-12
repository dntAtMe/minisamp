#include <windows.h>
#include <d3d9.h>
#include <detours.h>
#include "imgui.h"
#include "backends/imgui_impl_dx9.h"
#include "backends/imgui_impl_win32.h"
#include <iostream>

typedef HRESULT(APIENTRY* EndScene_t)(LPDIRECT3DDEVICE9 pDevice);
EndScene_t oEndScene = nullptr;

LPDIRECT3DDEVICE9 g_pD3DDevice = nullptr;
bool g_bImGuiInitialized = false;

HRESULT APIENTRY hkEndScene(LPDIRECT3DDEVICE9 pDevice) {
    if (!g_bImGuiInitialized) {
        ImGui::CreateContext();
        ImGui_ImplDX9_Init(pDevice);
        ImGui_ImplWin32_Init(FindWindowA(NULL, "DirectX 9 Hook Test App"));
        g_bImGuiInitialized = true;
    }

    ImGui_ImplDX9_NewFrame();
    ImGui_ImplWin32_NewFrame();
    ImGui::NewFrame();

    ImGui::Begin("ImGui Window");
    ImGui::Text("Hello from ImGui in GTA: San Andreas!");
    if (ImGui::Button("Click Me")) {
        std::cout << "Button clicked!" << std::endl;
    }
    ImGui::End();

    ImGui::Render();
    ImGui_ImplDX9_RenderDrawData(ImGui::GetDrawData());

    return oEndScene(pDevice);
}

typedef HRESULT(APIENTRY* CreateDevice_t)(
    IDirect3D9* pDirect3D9,
    UINT Adapter,
    D3DDEVTYPE DeviceType,
    HWND hFocusWindow,
    DWORD BehaviorFlags,
    D3DPRESENT_PARAMETERS* pPresentationParameters,
    IDirect3DDevice9** ppReturnedDeviceInterface);

CreateDevice_t oCreateDevice = nullptr;

HRESULT APIENTRY hkCreateDevice(
    IDirect3D9* pDirect3D9,
    UINT Adapter,
    D3DDEVTYPE DeviceType,
    HWND hFocusWindow,
    DWORD BehaviorFlags,
    D3DPRESENT_PARAMETERS* pPresentationParameters,
    IDirect3DDevice9** ppReturnedDeviceInterface) {

    HRESULT result = oCreateDevice(pDirect3D9, Adapter, DeviceType, hFocusWindow, BehaviorFlags, pPresentationParameters, ppReturnedDeviceInterface);

    if (SUCCEEDED(result)) {
        g_pD3DDevice = *ppReturnedDeviceInterface;

        void** vtable = *(void***)g_pD3DDevice;
        oEndScene = (EndScene_t)vtable[42];
        DetourTransactionBegin();
        DetourUpdateThread(GetCurrentThread());
        DetourAttach(&(PVOID&)oEndScene, hkEndScene);
        DetourTransactionCommit();
    }

    return result;
}

void HookDirect3D() {
    HMODULE hD3D9 = GetModuleHandle(L"d3d9.dll");
    if (hD3D9 == NULL) {
        std::cerr << "Failed to get d3d9.dll handle" << std::endl;
        return;
    }

    FARPROC pDirect3DCreate9 = GetProcAddress(hD3D9, "Direct3DCreate9");
    if (pDirect3DCreate9 == NULL) {
        std::cerr << "Failed to get Direct3DCreate9 address" << std::endl;
        return;
    }

    IDirect3D9* pD3D9 = ((IDirect3D9 * (WINAPI*)(UINT))pDirect3DCreate9)(D3D_SDK_VERSION);
    if (pD3D9 == NULL) {
        std::cerr << "Failed to create IDirect3D9 interface" << std::endl;
        return;
    }

    void** vtable = *(void***)pD3D9;
    oCreateDevice = (CreateDevice_t)vtable[16];

    DetourTransactionBegin();
    DetourUpdateThread(GetCurrentThread());
    DetourAttach(&(PVOID&)oCreateDevice, hkCreateDevice);
    DetourTransactionCommit();
}

void Unhook() {
    if (oEndScene) {
        DetourTransactionBegin();
        DetourUpdateThread(GetCurrentThread());
        DetourDetach(&(PVOID&)oEndScene, hkEndScene);
        DetourTransactionCommit();
        oEndScene = nullptr;
    }
    if (oCreateDevice) {
        DetourTransactionBegin();
        DetourUpdateThread(GetCurrentThread());
        DetourDetach(&(PVOID&)oCreateDevice, hkCreateDevice);
        DetourTransactionCommit();
        oCreateDevice = nullptr;
    }

    if (g_bImGuiInitialized) {
        ImGui_ImplDX9_Shutdown();
        ImGui_ImplWin32_Shutdown();
        ImGui::DestroyContext();
    }
}

DWORD WINAPI MainThread(LPVOID lpReserved) {
    HookDirect3D();
    return 0;
}

// DLL entry point
BOOL APIENTRY DllMain(HMODULE hModule, DWORD ul_reason_for_call, LPVOID lpReserved) {
    switch (ul_reason_for_call) {
    case DLL_PROCESS_ATTACH:
        CreateThread(NULL, 0, MainThread, hModule, 0, NULL);
        break;
    case DLL_PROCESS_DETACH:
        Unhook();
        break;
    }
    return TRUE;
}
