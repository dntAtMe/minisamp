#include <winsock2.h>
#include <Ws2tcpip.h>  // For additional TCP/IP functions
#include <windows.h>

#include "RakPeerInterface.h"
#include "MessageIdentifiers.h"
#include "BitStream.h"
#include "RakNetTypes.h"

#include <iostream>

RakNet::RakPeerInterface* rakPeer;

DWORD WINAPI MainThread(LPVOID param) {
    // Set up RakNet client
    rakPeer = RakNet::RakPeerInterface::GetInstance();
    RakNet::SocketDescriptor sd;
    rakPeer->Startup(1, &sd, 1);

    std::cout << "Injecting RakNet client into GTA: SA..." << std::endl;

    RakNet::ConnectionAttemptResult car = rakPeer->Connect("127.0.0.1", 6000, 0, 0);
    if (car != RakNet::CONNECTION_ATTEMPT_STARTED) {
        std::cout << "Failed to start connection attempt." << std::endl;
        return 1;
    }

    // Run the RakNet client in the GTA:SA process
    RakNet::Packet* packet;
    while (true) {
        for (packet = rakPeer->Receive(); packet; rakPeer->DeallocatePacket(packet), packet = rakPeer->Receive()) {
            switch (packet->data[0]) {
            case ID_CONNECTION_REQUEST_ACCEPTED:
                std::cout << "Connected to the server!" << std::endl;
                break;
            case ID_CONNECTION_ATTEMPT_FAILED:
                std::cout << "Failed to connect to the server!" << std::endl;
                break;
            default:
                std::cout << "Received a packet with id: " << packet->data[0] << std::endl;
                break;
            }
        }
        Sleep(100);  // Sleep to reduce CPU usage
    }

    RakNet::RakPeerInterface::DestroyInstance(rakPeer);
    return 0;
}

BOOL APIENTRY DllMain(HMODULE hModule, DWORD  ul_reason_for_call, LPVOID lpReserved) {
    switch (ul_reason_for_call) {
    case DLL_PROCESS_ATTACH:
        CreateThread(NULL, 0, MainThread, NULL, 0, NULL);  // Start the RakNet client in a separate thread
        break;
    case DLL_PROCESS_DETACH:
        // Clean up RakNet resources if needed
        RakNet::RakPeerInterface::DestroyInstance(rakPeer);
        break;
    }
    return TRUE;
}
