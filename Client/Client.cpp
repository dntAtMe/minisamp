#include "RakPeerInterface.h"
#include "MessageIdentifiers.h"
#include "BitStream.h"
#include "RakNetTypes.h"
#include "../MiniSamp/Common/MessageTypes.cpp"

#include <iostream>
#include <windows.h>

bool ReadPlayerHealth(DWORD processID, DWORD& health) {
	HANDLE hProcess = OpenProcess(PROCESS_VM_READ, FALSE, processID);
	if (!hProcess) {
		std::cerr << "Failed to open process" << std::endl;
		return false;
	}

	uintptr_t healthAddr = 0xBA18FC;

	ReadProcessMemory(hProcess, (LPCVOID)healthAddr, &health, sizeof(health), NULL);

	CloseHandle(hProcess);
	return true;
}


int main()
{
	DWORD processID = 40824;
	DWORD x;

	if (!ReadPlayerHealth(processID, x)) {
		std::cerr << "Failed to read player position" << std::endl;
		return 1;
	}

	std::cout << "Player health: " << x << std::endl;

	std::cin.get();

	return 0;

    std::cout << "Starting the client" << std::endl;

    RakNet::RakPeerInterface* rakPeer = RakNet::RakPeerInterface::GetInstance();
    RakNet::SocketDescriptor sd;
    rakPeer->Startup(10, &sd, 1);

    std::cout << "Connecting to the server..." << std::endl;

	RakNet::ConnectionAttemptResult car = rakPeer->Connect("127.0.0.1", 6000, 0, 0);
	if (car != RakNet::CONNECTION_ATTEMPT_STARTED) {
		std::cout << "Failed to start connection attempt." << std::endl;
		return 1;
	}

	RakNet::BitStream bsOut;
    RakNet::Packet* packet;

    while (true)
    {
        for (packet = rakPeer->Receive(); packet; rakPeer->DeallocatePacket(packet), packet = rakPeer->Receive())
        {
			std::cout << "Processing a received message" << std::endl;
            switch (packet->data[0])
            {
			case ID_CONNECTION_REQUEST_ACCEPTED:
				std::cout << "Connected to the server" << std::endl;

				bsOut.Write((RakNet::MessageID)ID_GAME_MESSAGE_1);
				bsOut.Write("Hello server");
				rakPeer->Send(&bsOut, HIGH_PRIORITY, RELIABLE_ORDERED, 0, packet->systemAddress, false);
				std::cout << "Sent a message to the server" << std::endl;

				break;
			case ID_CONNECTION_ATTEMPT_FAILED:
				std::cout << "Failed to connect to the server" << std::endl;
				break;
			case ID_NO_FREE_INCOMING_CONNECTIONS:
				std::cout << "Server is full" << std::endl;
				break;
			case ID_DISCONNECTION_NOTIFICATION:
				std::cout << "Disconnected from the server" << std::endl;
				break;
			case ID_CONNECTION_LOST:
				std::cout << "Lost connection to the server" << std::endl;
				break;
			default:
				std::cout << "Received a message with an unknown id: " << packet->data[0] << std::endl;
				break;
			}
		}
	}

	RakNet::RakPeerInterface::DestroyInstance(rakPeer);

	return 0;
}
