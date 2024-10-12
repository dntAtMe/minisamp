#include "pch.h"

#include <windows.h>
#include <iostream>

#define PLAYER_HEALTH_ADDR 0xBA18FC

void AccessMemory(DWORD address)
{
    int value = 0;

    __asm
    {
        mov eax, address
        mov eax, [eax]
        mov value, eax
    }

    std::cout << "Value at address " << std::hex << address << " is: " << value << std::endl;
}

void WriteMemory(DWORD address, float newValue)
{
    __asm
    {
        mov eax, address
        mov ebx, newValue
        mov[eax], ebx
    }

    std::cout << "New value " << newValue << " written to address " << std::hex << address << std::endl;
}

BOOL APIENTRY DllMain(HMODULE hModule, DWORD  ul_reason_for_call, LPVOID lpReserved)
{
    switch (ul_reason_for_call)
    {
    case DLL_PROCESS_ATTACH:
        AccessMemory(PLAYER_HEALTH_ADDR);
        WriteMemory(PLAYER_HEALTH_ADDR, 0);
        break;

    case DLL_PROCESS_DETACH:
        break;
    }
    return TRUE;
}
